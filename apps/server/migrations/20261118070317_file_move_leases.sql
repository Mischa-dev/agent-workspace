-- K5.13 follow-up (Lane B chunk 3): two moves into the same store (a
-- server-wide one and a workspace's) wrote the same destination key, and
-- the one that lost the switch deleted the object the winner's row named.
-- A move, and the prune of a moved file's old copy, now holds a lease on
-- the file's row first: only the holder writes or deletes its bytes, and a
-- stopped holder's lease runs out.
ALTER TABLE files
    ADD COLUMN move_lease UUID,
    ADD COLUMN move_lease_until TIMESTAMPTZ,
    ADD CONSTRAINT files_move_lease_whole
        CHECK ((move_lease IS NULL) = (move_lease_until IS NULL));
