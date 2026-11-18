-- H1.14: first run in the browser. While no person has an account, the
-- server keeps one setup code: its SHA-256 here, the code itself in a file
-- in the data root of the process that issued it. Whoever enters it makes
-- the first account and either a workspace or an import; until the import
-- is uploaded, the row names that account. The row goes once setup is
-- done, or as soon as anyone has an account another way.
CREATE TABLE server_setup (
    id SMALLINT PRIMARY KEY CHECK (id = 1),
    code_digest BYTEA NOT NULL CHECK (octet_length(code_digest) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    claimed_by_actor_id UUID REFERENCES actors(id) ON DELETE CASCADE,
    claimed_at TIMESTAMPTZ,
    CHECK ((claimed_by_actor_id IS NULL) = (claimed_at IS NULL))
);
