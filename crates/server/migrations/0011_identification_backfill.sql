-- Data migration: best-effort backfill of the old two-stage
-- triage_results + deepdive_results history into the new single
-- identification_results shape, so sightings identified before this
-- release still show up in the new API. triage_results/deepdive_results are
-- intentionally NOT dropped (this app has real production data; the old
-- tables just stop being written to going forward).
--
-- For each sighting, this takes its latest triage_results row (there must
-- be at least one for a row to be produced at all) and, if one exists, its
-- latest deepdive_results row, and maps them as follows:
--   * status: 'complete' if a deepdive row exists, else the triage status
--     ('insufficient' stays 'insufficient'; 'genus_candidate'/
--     'species_candidate' without a deepdive become 'partial' since no
--     facts/risks enrichment ever ran for them).
--   * model: the deepdive row's model if present, else the triage row's.
--   * candidates_json:
--       - with a deepdive row: a single-element array for
--         `best_match_species` (deepdive never stored more than one best
--         match), carrying its confidence/wikipedia_url, a risk_note
--         derived from the old `safety_notes` free text (truncated to 140
--         chars), and `confusants` converted from the old
--         {species, common_name, danger_level, distinguishing_features,
--         notes} shape into the new condensed
--         {species, danger_level, note, wikipedia_url} shape (old
--         DangerLevel values 'safe'/'caution' both map to the new
--         'mild' level, since the new confusant contract only has
--         unknown|mild|toxic|deadly_toxic — see the identification module's
--         report for this deviation). edible/medicinal/psychoactive/
--         poisonous are left NULL: the old pipeline never captured them.
--       - without a deepdive row: every triage candidate_species entry,
--         carried over as-is with every new-only field NULL/empty.
--   * missing_info_json / photos_considered: copied verbatim from the
--     triage row — both columns already match the new shape exactly.
INSERT INTO identification_results (
    id, sighting_id, created_at, updated_at, status, model,
    candidates_json, missing_info_json, photos_considered
)
SELECT
    lower(hex(randomblob(16))),
    t.sighting_id,
    COALESCE(d.created_at, t.created_at),
    COALESCE(d.created_at, t.created_at),
    CASE
        WHEN d.id IS NOT NULL THEN 'complete'
        WHEN t.status = 'insufficient' THEN 'insufficient'
        ELSE 'partial'
    END,
    COALESCE(d.model, t.model),
    CASE
        WHEN d.id IS NOT NULL THEN
            json_array(
                json_object(
                    'species', d.best_match_species,
                    'common_name', (
                        SELECT json_extract(tc.value, '$.common_name')
                        FROM json_each(t.candidate_species_json) tc
                        WHERE json_extract(tc.value, '$.species') = d.best_match_species
                        LIMIT 1
                    ),
                    'confidence', d.confidence,
                    'edible', NULL,
                    'medicinal', NULL,
                    'psychoactive', NULL,
                    'poisonous', NULL,
                    'wikipedia_url', d.wikipedia_url,
                    'risk_note', substr(COALESCE(d.safety_notes, ''), 1, 140),
                    'confusants', (
                        SELECT COALESCE(json_group_array(
                            json_object(
                                'species', json_extract(dc.value, '$.species'),
                                'danger_level', CASE json_extract(dc.value, '$.danger_level')
                                    WHEN 'safe' THEN 'mild'
                                    WHEN 'caution' THEN 'mild'
                                    WHEN 'toxic' THEN 'toxic'
                                    WHEN 'deadly_toxic' THEN 'deadly_toxic'
                                    ELSE 'unknown'
                                END,
                                'note', substr(COALESCE(json_extract(dc.value, '$.notes'), ''), 1, 100),
                                'wikipedia_url', NULL
                            )
                        ), '[]')
                        FROM json_each(d.confusants_json) dc
                    )
                )
            )
        ELSE
            (
                SELECT COALESCE(json_group_array(
                    json_object(
                        'species', json_extract(tc.value, '$.species'),
                        'common_name', json_extract(tc.value, '$.common_name'),
                        'confidence', json_extract(tc.value, '$.confidence'),
                        'edible', NULL,
                        'medicinal', NULL,
                        'psychoactive', NULL,
                        'poisonous', NULL,
                        'wikipedia_url', NULL,
                        'risk_note', NULL,
                        'confusants', json('[]')
                    )
                ), '[]')
                FROM json_each(t.candidate_species_json) tc
            )
    END,
    t.missing_info_json,
    t.photos_considered
FROM triage_results t
LEFT JOIN deepdive_results d
    ON d.sighting_id = t.sighting_id
   AND d.created_at = (
        SELECT MAX(d2.created_at) FROM deepdive_results d2 WHERE d2.sighting_id = t.sighting_id
   )
WHERE t.created_at = (
    SELECT MAX(t2.created_at) FROM triage_results t2 WHERE t2.sighting_id = t.sighting_id
);
