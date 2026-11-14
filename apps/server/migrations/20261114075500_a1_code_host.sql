-- A1.18: the code-host seam, part 1. A repository names the code host it
-- lives on and its id there, so code that isn't GitHub's can be added
-- behind `code_host::CodeHost` without new columns per host.
--
-- * code_host: 'github' for every repository today.
-- * external_id: the repository's id on its code host. For GitHub it is
--   github_repository_id, computed here so every existing writer keeps it
--   right with no trigger; the expression is dropped (ALTER COLUMN ... DROP
--   EXPRESSION) when a second code host writes its own ids.

ALTER TABLE repositories
    ADD COLUMN code_host TEXT NOT NULL DEFAULT 'github'
        CHECK (code_host ~ '^[a-z][a-z0-9_]{0,39}$'),
    ADD COLUMN external_id TEXT
        GENERATED ALWAYS AS (github_repository_id::text) STORED;
