-- Adopt retained pre-M3 mirrors into the UUID-scoped M3 layout without
-- moving or deleting the legacy mirror that owns historical dirty worktrees.

CREATE TABLE IF NOT EXISTS m3_repository_path_upgrades (
    repository_id UUID PRIMARY KEY,
    legacy_path TEXT NOT NULL,
    canonical_path TEXT NOT NULL
);

LOCK TABLE repositories IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE repositories
    ADD COLUMN legacy_mirror_path TEXT,
    ADD CONSTRAINT repositories_legacy_mirror_path_check CHECK (
        legacy_mirror_path IS NULL
        OR (
            char_length(legacy_mirror_path) BETWEEN 1 AND 4096
            AND left(legacy_mirror_path, 1) = '/'
            AND legacy_mirror_path <> mirror_path
        )
    );

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM repositories repository
        WHERE repository.mirror_path IS DISTINCT FROM (
            SELECT upgrade.legacy_path
            FROM m3_repository_path_upgrades upgrade
            WHERE upgrade.repository_id = repository.id
        )
        AND EXISTS (
            SELECT 1
            FROM m3_repository_path_upgrades upgrade
            WHERE upgrade.repository_id = repository.id
        )
    ) THEN
        RAISE EXCEPTION 'Repository mirror changed during canonical-path preflight'
            USING ERRCODE = '23514';
    END IF;
END;
$$;

DROP TRIGGER repositories_enforce_identity ON repositories;

UPDATE repositories repository
SET legacy_mirror_path = upgrade.legacy_path,
    mirror_path = upgrade.canonical_path,
    version = version + 1,
    updated_at = now()
FROM m3_repository_path_upgrades upgrade
WHERE upgrade.repository_id = repository.id;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM repositories
        WHERE mirror_path !~ (
            '/repositories/' || id::text || '/mirror[.]git$'
        )
    ) THEN
        RAISE EXCEPTION 'Repository mirror is not in the canonical UUID-scoped layout'
            USING ERRCODE = '23514';
    END IF;
END;
$$;

CREATE OR REPLACE FUNCTION enforce_repository_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.github_repository_id IS DISTINCT FROM OLD.github_repository_id
       OR NEW.owner IS DISTINCT FROM OLD.owner
       OR NEW.name IS DISTINCT FROM OLD.name
       OR NEW.clone_url IS DISTINCT FROM OLD.clone_url
       OR NEW.mirror_path IS DISTINCT FROM OLD.mirror_path
       OR NEW.legacy_mirror_path IS DISTINCT FROM OLD.legacy_mirror_path THEN
        RAISE EXCEPTION 'Repository identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER repositories_enforce_identity
BEFORE UPDATE
ON repositories
FOR EACH ROW
EXECUTE FUNCTION enforce_repository_identity();

DROP TABLE m3_repository_path_upgrades;
