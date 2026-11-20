-- M1.10 (Lane C): package upgrades.
--
-- package_objects.local_hash: a fingerprint of the object as it stood right
-- after its package put it there (or last updated it). An upgrade compares
-- the object now with it to tell "changed here" from "untouched". NULL on
-- rows from before upgrades existed, which an upgrade treats as changed
-- here, so nothing is overwritten that might have been edited.
--
-- action 'detached': an object a newer version no longer carries. It stays
-- where it is, as it is; removing the package leaves it alone.
--
-- package_installs.upgraded_from: the install a newer version replaced
-- (that one is marked superseded).

ALTER TABLE package_objects
    ADD COLUMN local_hash TEXT CHECK (local_hash IS NULL OR char_length(local_hash) <= 100);

ALTER TABLE package_objects DROP CONSTRAINT IF EXISTS package_objects_action_check;
ALTER TABLE package_objects
    ADD CONSTRAINT package_objects_action_check
    CHECK (action IN ('created', 'reused', 'updated', 'detached'));

ALTER TABLE package_installs
    ADD COLUMN upgraded_from UUID REFERENCES package_installs(id) ON DELETE SET NULL;

CREATE INDEX package_installs_upgraded_from ON package_installs (upgraded_from)
    WHERE upgraded_from IS NOT NULL;
