-- Bytes cleanup or the pending sweep is deleting. The claim is written in
-- one transaction, the store's delete runs outside any transaction, and a
-- second transaction finishes the claim. While a key is claimed, staging
-- the same content writes bytes under a key of its own instead, so a
-- delete in flight never takes bytes a new row holds.
ALTER TABLE file_objects_pending ADD COLUMN deleting_since TIMESTAMPTZ;

-- The size of bytes staged before their row exists, so the room checks
-- count them until a row does (or the sweep takes them away). Null where
-- a row already held the bytes when they were pinned.
ALTER TABLE file_objects_pending ADD COLUMN byte_size BIGINT CHECK (byte_size >= 0);
