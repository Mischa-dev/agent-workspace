-- A2.27 follow-up (Lane C, from #325's review): an allowed review that was
-- used remembers the request key of the call that used it (as a SHA-256 hex
-- digest, never the key), so the same keyed call retried after a lost answer
-- reaches its command's own replay instead of asking the reviewers again.
ALTER TABLE guard_holds
    ADD COLUMN key_hash TEXT CHECK (key_hash IS NULL OR key_hash ~ '^[0-9a-f]{64}$');

CREATE INDEX guard_holds_used_key
    ON guard_holds (account_id, operation, key_hash)
    WHERE state = 'used' AND key_hash IS NOT NULL;
