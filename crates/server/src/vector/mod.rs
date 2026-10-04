//! Brute-force cosine-similarity vector store, backed by a plain SQLite
//! `BLOB` column (migration `0007_vectors.sql`).
//!
//! Same design as `ai_buddy`'s `vector` module: no vector database, no
//! external index. Embeddings are stored as little-endian `f32` bytes and
//! compared with an in-process cosine similarity scan over every row of the
//! requested `kind`. This is plenty fast at the scale of a personal foraging
//! log (a handful of Wikipedia articles' worth of chunks, not millions of
//! rows), and it keeps the whole backend to one SQLite file with zero extra
//! services to run.
//!
//! Shared by two independent corpora, distinguished by `kind`:
//! `"wikipedia"` (article chunks, populated by the `deepdive` module after
//! calling `wikipedia::fetch_or_cache`) and `"confusant"` (reserved for
//! curated-note embeddings, currently unused but kept as a stable `kind`
//! value per the architecture doc).

use sqlx::SqlitePool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

/// Encode a vector as little-endian `f32` bytes for the `vec` BLOB column.
fn encode_vec(vec: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vec.len() * 4);
    for v in vec {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

/// Decode little-endian `f32` bytes back into a vector. Any trailing bytes
/// that don't make up a full 4-byte float are silently dropped (defensive
/// against a corrupt/truncated row rather than panicking).
#[allow(clippy::chunks_exact_to_as_chunks)] // chunks_exact(4) is clearer here than as_chunks::<4>()
fn decode_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            // chunks_exact(4) guarantees 4-byte chunks, so this conversion
            // never actually fails; `unwrap_or_default` avoids a panic path
            // regardless.
            let arr: [u8; 4] = chunk.try_into().unwrap_or_default();
            f32::from_le_bytes(arr)
        })
        .collect()
}

/// Cosine similarity between two equal-length vectors. Returns `f32::MIN`
/// for mismatched/empty inputs so they always sort last, and `0.0` for a
/// zero vector (rather than dividing by zero).
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return f32::MIN;
    }

    let mut dot = 0f32;
    let mut norm_a = 0f32;
    let mut norm_b = 0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

/// Store one embedded chunk. A given `(kind, source_id)` pair may have many
/// rows (one per chunk of a longer source document) — this always inserts a
/// new row rather than replacing anything, so callers that re-embed a source
/// should accept some duplication is possible; search ranks by similarity so
/// stale/duplicate chunks just don't surface unless they're actually close.
pub async fn upsert(
    db: &SqlitePool,
    kind: &str,
    source_id: &str,
    model: &str,
    text: &str,
    vec: &[f32],
) -> AppResult<()> {
    let id = Uuid::new_v4().to_string();
    let blob = encode_vec(vec);

    sqlx::query(
        "INSERT INTO embeddings (id, kind, source_id, model, dim, vec, text, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now'))",
    )
    .bind(id)
    .bind(kind)
    .bind(source_id)
    .bind(model)
    .bind(vec.len() as i64)
    .bind(blob)
    .bind(text)
    .execute(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    Ok(())
}

/// Return the `top_k` chunks of `kind` most similar to `query_vec`, as
/// `(source_id, text, score)`, highest score first.
pub async fn search(
    db: &SqlitePool,
    kind: &str,
    query_vec: &[f32],
    top_k: usize,
) -> AppResult<Vec<(String, String, f32)>> {
    let rows: Vec<(String, String, Vec<u8>)> =
        sqlx::query_as("SELECT source_id, text, vec FROM embeddings WHERE kind = ?")
            .bind(kind)
            .fetch_all(db)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;

    let mut scored: Vec<(String, String, f32)> = rows
        .into_iter()
        .map(|(source_id, text, blob)| {
            let vec = decode_vec(&blob);
            let score = cosine_similarity(query_vec, &vec);
            (source_id, text, score)
        })
        .collect();

    scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k);

    Ok(scored)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = crate::db::init_pool(db_path.to_str().unwrap())
            .await
            .unwrap();
        (dir, pool)
    }

    #[test]
    fn encode_decode_roundtrip() {
        let original = vec![1.0_f32, -2.5, 0.0, 3.25];
        let bytes = encode_vec(&original);
        assert_eq!(bytes.len(), 16);
        assert_eq!(decode_vec(&bytes), original);
    }

    #[test]
    fn cosine_similarity_identical_vectors_is_one() {
        let v = vec![1.0_f32, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors_is_zero() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![0.0_f32, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_mismatched_lengths_sorts_last() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![1.0_f32];
        assert_eq!(cosine_similarity(&a, &b), f32::MIN);
    }

    #[tokio::test]
    async fn search_ranks_by_similarity_and_respects_top_k() {
        let (_dir, pool) = test_pool().await;

        upsert(
            &pool,
            "wikipedia",
            "src-a",
            "test-model",
            "chunk about death caps",
            &[1.0, 0.0, 0.0],
        )
        .await
        .unwrap();
        upsert(
            &pool,
            "wikipedia",
            "src-b",
            "test-model",
            "chunk about button mushrooms",
            &[0.0, 1.0, 0.0],
        )
        .await
        .unwrap();
        upsert(
            &pool,
            "wikipedia",
            "src-c",
            "test-model",
            "unrelated chunk",
            &[0.0, 0.0, 1.0],
        )
        .await
        .unwrap();
        // Different `kind` — must never show up in a "wikipedia" search.
        upsert(
            &pool,
            "confusant",
            "src-d",
            "test-model",
            "a confusant note",
            &[1.0, 0.0, 0.0],
        )
        .await
        .unwrap();

        let results = search(&pool, "wikipedia", &[1.0, 0.0, 0.0], 2)
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        let first = results.first().unwrap();
        let second = results.get(1).unwrap();
        assert_eq!(first.0, "src-a");
        assert!(first.2 > second.2);
        assert!(results.iter().all(|(id, _, _)| id != "src-d"));
    }
}
