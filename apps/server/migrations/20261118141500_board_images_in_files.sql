-- C1.29 (lane D2, documents): a board's images move into `files` and the
-- file store, as files of the board (thing_kind 'whiteboard', external_key
-- the id the board's elements name them by). New images go there at once;
-- the `whiteboards.move_images` job moves the ones in `whiteboard_files`,
-- and reads fall back to that table until it is empty. A later migration
-- drops `whiteboard_files` once the job has run on the live server.
--
-- Board images are stored under a key named by their content, so a
-- duplicated board adds rows over the same bytes. The cleanup keeps bytes
-- while any row still holds them, and finds those rows by key.

ALTER TABLE files DROP CONSTRAINT files_storage_key_key;
CREATE INDEX files_storage_key ON files (storage_key) WHERE blob_deleted_at IS NULL;

INSERT INTO background_jobs (id, kind, payload, unique_key, max_attempts)
SELECT gen_random_uuid(), 'whiteboards.move_images', '{}'::jsonb, 'whiteboards.move_images', 50
WHERE EXISTS (SELECT 1 FROM whiteboard_files);
