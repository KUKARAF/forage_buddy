-- Caches the Wikipedia reference-photo URL alongside the rest of a
-- species's cached facts, so `identification::verify_visual_match` doesn't
-- need a second Wikipedia fetch on a facts-cache hit. Nullable: a species
-- may have no photo, and every existing cached row predates this column.

ALTER TABLE species_facts_cache ADD COLUMN wikipedia_image_url TEXT;
