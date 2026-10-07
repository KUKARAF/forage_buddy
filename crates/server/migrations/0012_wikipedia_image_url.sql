-- Adds a representative-photo URL to the Wikipedia cache, used by
-- `identification::verify_visual_match` to compare a real Wikipedia photo
-- of the identified species against the forager's own photo. Nullable: a
-- page may have no thumbnail/original image, and every existing cached row
-- predates this column.

ALTER TABLE wikipedia_pages ADD COLUMN image_url TEXT;
