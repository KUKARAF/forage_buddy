//! Curated, hand-checked offline foraging-safety reference data
//! (migration `0005_species_reference.sql`) — **the safety-critical part of
//! this module**.
//!
//! `species_reference` and `confusant_pairs` are seeded once, in the
//! migration itself, with well-established, widely-published facts (see
//! that file's header comment). This module only ever *reads* that data; it
//! never writes to it, and nothing here lets an LLM response overwrite or
//! shadow a curated row. The `deepdive` module treats this as the
//! trustworthy floor that model output is layered on top of, never a
//! replacement for.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use sqlx::SqlitePool;

use crate::error::{AppError, AppResult};

/// A curated species_reference row, as looked up by [`lookup`]. Public API
/// of this module: the `deepdive` module only reads `wikipedia_title` and
/// `danger_level` today, but the full curated row is exposed here for any
/// future caller (e.g. a `/api/species/lookup` endpoint) rather than
/// trimming it down to just what's used right now.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SpeciesReferenceDto {
    pub id: String,
    pub genus: String,
    pub species: Option<String>,
    pub common_names: Vec<String>,
    pub danger_level: String,
    pub wikipedia_title: Option<String>,
    pub summary: String,
}

/// One curated dangerous-lookalike entry, as returned by
/// [`curated_confusants_for`]: the *other* species in a confusant pair,
/// plus the checklist for telling it apart from the one being looked up.
#[derive(Debug, Clone, Serialize)]
pub struct ConfusantDto {
    pub species_ref_id: String,
    pub genus: String,
    pub species: Option<String>,
    pub common_names: Vec<String>,
    pub danger_level: String,
    pub distinguishing_features: Vec<String>,
    pub notes: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct SpeciesRow {
    id: String,
    genus: String,
    species: Option<String>,
    common_names_json: String,
    danger_level: String,
    wikipedia_title: Option<String>,
    summary: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct PairRow {
    id: String,
    species_ref_a: String,
    species_ref_b: String,
    danger_level: String,
    distinguishing_features_json: String,
    notes: String,
}

fn parse_common_names(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

/// Fuzzy, case-insensitive match of `query` against a curated row: exact or
/// substring match on genus, "Genus species", or any common name. Loose on
/// purpose — the caller (deep-dive) is matching free-text LLM output like
/// "Amanita phalloides (death cap)" or just a bare genus.
fn row_matches(row: &SpeciesRow, query_lower: &str) -> bool {
    if row.genus.to_lowercase() == query_lower || query_lower.contains(&row.genus.to_lowercase()) {
        return true;
    }

    if let Some(species) = &row.species {
        let full = format!("{} {}", row.genus, species).to_lowercase();
        if full == query_lower || query_lower.contains(&full) || full.contains(query_lower) {
            return true;
        }
    }

    parse_common_names(&row.common_names_json)
        .iter()
        .any(|name| {
            let name_lower = name.to_lowercase();
            name_lower == query_lower
                || query_lower.contains(&name_lower)
                || name_lower.contains(query_lower)
        })
}

fn to_species_reference_dto(row: &SpeciesRow) -> SpeciesReferenceDto {
    SpeciesReferenceDto {
        id: row.id.clone(),
        genus: row.genus.clone(),
        species: row.species.clone(),
        common_names: parse_common_names(&row.common_names_json),
        danger_level: row.danger_level.clone(),
        wikipedia_title: row.wikipedia_title.clone(),
        summary: row.summary.clone(),
    }
}

async fn all_species_rows(db: &SqlitePool) -> AppResult<Vec<SpeciesRow>> {
    sqlx::query_as(
        "SELECT id, genus, species, common_names_json, danger_level, wikipedia_title, summary \
         FROM species_reference",
    )
    .fetch_all(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))
}

/// Look up the curated row (if any) matching `species_or_genus`. Returns the
/// first match when more than one curated row matches (e.g. a bare genus
/// query against a genus with several seeded species) — callers that need
/// the most specific match should pass a full "Genus species" string.
pub async fn lookup(
    db: &SqlitePool,
    species_or_genus: &str,
) -> AppResult<Option<SpeciesReferenceDto>> {
    let query = species_or_genus.trim();
    if query.is_empty() {
        return Ok(None);
    }
    let query_lower = query.to_lowercase();

    let rows = all_species_rows(db).await?;
    Ok(rows
        .iter()
        .find(|row| row_matches(row, &query_lower))
        .map(to_species_reference_dto))
}

/// Curated dangerous-lookalike lookup: matches `species_or_genus` against
/// `species_reference` (by genus, by "Genus species", or by a common name),
/// then joins `confusant_pairs` in both directions (as `species_ref_a` or
/// as `species_ref_b`) and returns the *other* side of each matching pair.
///
/// Always backed by the curated migration data — never the output of an LLM
/// call. Returns an empty list (not an error) when nothing matches, so
/// callers can safely treat "no curated confusants known" the same as "none
/// found".
pub async fn curated_confusants_for(
    db: &SqlitePool,
    species_or_genus: &str,
) -> AppResult<Vec<ConfusantDto>> {
    let query = species_or_genus.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let query_lower = query.to_lowercase();

    let species_rows = all_species_rows(db).await?;
    let matched_ids: HashSet<String> = species_rows
        .iter()
        .filter(|row| row_matches(row, &query_lower))
        .map(|row| row.id.clone())
        .collect();

    if matched_ids.is_empty() {
        return Ok(Vec::new());
    }

    let by_id: HashMap<&str, &SpeciesRow> =
        species_rows.iter().map(|r| (r.id.as_str(), r)).collect();

    let pair_rows: Vec<PairRow> = sqlx::query_as(
        "SELECT id, species_ref_a, species_ref_b, danger_level, distinguishing_features_json, notes \
         FROM confusant_pairs",
    )
    .fetch_all(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    let mut seen_pairs = HashSet::new();
    let mut out = Vec::new();

    for pair in &pair_rows {
        let counterpart_id = if matched_ids.contains(&pair.species_ref_a) {
            pair.species_ref_b.as_str()
        } else if matched_ids.contains(&pair.species_ref_b) {
            pair.species_ref_a.as_str()
        } else {
            continue;
        };

        if !seen_pairs.insert(pair.id.clone()) {
            continue;
        }

        let Some(counterpart) = by_id.get(counterpart_id) else {
            continue;
        };

        out.push(ConfusantDto {
            species_ref_id: counterpart.id.clone(),
            genus: counterpart.genus.clone(),
            species: counterpart.species.clone(),
            common_names: parse_common_names(&counterpart.common_names_json),
            danger_level: pair.danger_level.clone(),
            distinguishing_features: serde_json::from_str(&pair.distinguishing_features_json)
                .unwrap_or_default(),
            notes: pair.notes.clone(),
        });
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh temp-file sqlite pool with the full migration chain applied
    /// (including the seeded `species_reference`/`confusant_pairs` data from
    /// `0005_species_reference.sql`) — this is the integrity check for the
    /// safety-critical seed data itself.
    async fn test_pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = crate::db::init_pool(db_path.to_str().unwrap())
            .await
            .unwrap();
        (dir, pool)
    }

    #[tokio::test]
    async fn death_cap_confusant_with_button_mushroom_is_seeded() {
        let (_dir, pool) = test_pool().await;

        let confusants = curated_confusants_for(&pool, "Amanita phalloides")
            .await
            .unwrap();

        let button = confusants
            .iter()
            .find(|c| c.genus == "Agaricus" && c.species.as_deref() == Some("bisporus"));
        let button =
            button.expect("death cap must list the button mushroom as a curated confusant");

        assert_eq!(button.danger_level, "deadly_toxic");
        assert!(!button.distinguishing_features.is_empty());
        assert!(button
            .distinguishing_features
            .iter()
            .any(|f| f.to_lowercase().contains("volva")));
    }

    #[tokio::test]
    async fn lookup_is_bidirectional() {
        let (_dir, pool) = test_pool().await;

        // Looking up the *other* side of the pair must return the same
        // relationship, just from the opposite direction.
        let confusants = curated_confusants_for(&pool, "button mushroom")
            .await
            .unwrap();
        let death_cap = confusants
            .iter()
            .find(|c| c.genus == "Amanita" && c.species.as_deref() == Some("phalloides"));
        assert!(
            death_cap.is_some(),
            "button mushroom must list death cap as a confusant"
        );
    }

    #[tokio::test]
    async fn genus_level_query_matches_all_species_in_genus() {
        let (_dir, pool) = test_pool().await;

        // Querying the bare genus "Amanita" should surface confusants for
        // every seeded Amanita species (phalloides, virosa, bisporigera).
        let confusants = curated_confusants_for(&pool, "Amanita").await.unwrap();
        assert!(!confusants.is_empty());
        assert!(confusants.iter().any(|c| c.genus == "Agaricus"));
    }

    #[tokio::test]
    async fn common_name_query_matches() {
        let (_dir, pool) = test_pool().await;

        let confusants = curated_confusants_for(&pool, "false morel").await.unwrap();
        let true_morel = confusants
            .iter()
            .find(|c| c.genus == "Morchella" && c.species.as_deref() == Some("esculenta"));
        assert!(
            true_morel.is_some(),
            "false morel must list true morel as a confusant"
        );
        assert_eq!(true_morel.unwrap().danger_level, "deadly_toxic");
    }

    #[tokio::test]
    async fn unknown_species_returns_empty_not_error() {
        let (_dir, pool) = test_pool().await;

        let confusants = curated_confusants_for(&pool, "Totally Fictional Species")
            .await
            .unwrap();
        assert!(confusants.is_empty());
    }

    #[tokio::test]
    async fn lookup_returns_curated_wikipedia_title_and_danger_level() {
        let (_dir, pool) = test_pool().await;

        let found = lookup(&pool, "death cap").await.unwrap();
        let found = found.expect("death cap should resolve to the curated row");
        assert_eq!(found.genus, "Amanita");
        assert_eq!(found.danger_level, "deadly_toxic");
        assert_eq!(
            found.wikipedia_title,
            Some("Amanita phalloides".to_string())
        );
    }

    #[tokio::test]
    async fn every_confusant_pair_references_a_real_species_reference_row() {
        let (_dir, pool) = test_pool().await;

        let species_ids: HashSet<String> = all_species_rows(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();

        let pairs: Vec<PairRow> = sqlx::query_as(
            "SELECT id, species_ref_a, species_ref_b, danger_level, distinguishing_features_json, notes \
             FROM confusant_pairs",
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        assert_eq!(
            pairs.len(),
            10,
            "expected exactly the 10 seeded confusant pairs"
        );
        for pair in &pairs {
            assert!(
                species_ids.contains(&pair.species_ref_a),
                "{} -> dangling species_ref_a",
                pair.id
            );
            assert!(
                species_ids.contains(&pair.species_ref_b),
                "{} -> dangling species_ref_b",
                pair.id
            );
            assert!(!pair.distinguishing_features_json.is_empty());
        }
    }
}
