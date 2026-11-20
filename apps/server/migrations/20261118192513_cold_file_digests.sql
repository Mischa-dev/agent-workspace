-- K3.25 follow-up (review of #299): a cold log or history file is read
-- back only as it was written. Each file records its compressed size and
-- SHA-256; a reader takes exactly that many bytes from the store, checks
-- the digest, and unpacks under a fixed limit, so a store that serves
-- something else (a bigger object, a gzip bomb) is refused, not parsed.
-- Nothing is deployed with these tables yet, so they hold no rows.
ALTER TABLE event_archives
    ADD COLUMN byte_size BIGINT NOT NULL CHECK (byte_size > 0),
    ADD COLUMN sha256 BYTEA NOT NULL CHECK (octet_length(sha256) = 32);
ALTER TABLE history_archives
    ADD COLUMN byte_size BIGINT NOT NULL CHECK (byte_size > 0),
    ADD COLUMN sha256 BYTEA NOT NULL CHECK (octet_length(sha256) = 32);
