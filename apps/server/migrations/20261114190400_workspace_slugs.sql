-- K4.5: workspace slugs (K4 plan, sections 2.6 and 3.10 point 8).
--
-- `workspace_slugs` keeps every slug a workspace has had. The current one
-- is also `organizations.slug`; a retired one keeps pointing at its
-- workspace and can never be taken by another, so an old link never opens
-- someone else's workspace. The rules for new slugs (3 to 40 characters,
-- not a reserved word such as `aw-direct-scope`) are checked in Rust
-- (`workspace::slug`); existing longer slugs stay valid.
--
-- Triggers added (ruling R16): `organizations_slug_history` on
-- `organizations`, the only writer of `workspace_slugs`, so every path that
-- creates or renames a workspace keeps the history in the same
-- transaction. It stays: the history is the product, not a stopgap.
--
-- The direct scope (K4.23) has no organizations row yet, so the backfill
-- has nothing to skip; K4.23 leaves its row out of the history.

CREATE TABLE workspace_slugs (
    slug TEXT PRIMARY KEY CHECK (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$' AND char_length(slug) <= 63),
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    current BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    retired_at TIMESTAMPTZ,
    CHECK (current = (retired_at IS NULL))
);

CREATE UNIQUE INDEX workspace_slugs_one_current ON workspace_slugs (workspace_id) WHERE current;
CREATE INDEX workspace_slugs_workspace ON workspace_slugs (workspace_id);

INSERT INTO workspace_slugs (slug, workspace_id, created_at)
SELECT slug, id, created_at FROM organizations
WHERE slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$' AND char_length(slug) <= 63
ON CONFLICT (slug) DO NOTHING;

CREATE FUNCTION organizations_slug_history()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND NEW.slug = OLD.slug THEN
        RETURN NULL;
    END IF;
    UPDATE workspace_slugs
    SET current = false, retired_at = now()
    WHERE workspace_id = NEW.id AND current AND slug <> NEW.slug;
    -- A slug longer than a DNS label (old test and import data only) is
    -- still the current slug in `organizations`, with no history row.
    IF char_length(NEW.slug) > 63 THEN
        RETURN NULL;
    END IF;
    INSERT INTO workspace_slugs (slug, workspace_id)
    VALUES (NEW.slug, NEW.id)
    ON CONFLICT (slug) DO UPDATE SET current = true, retired_at = NULL
    WHERE workspace_slugs.workspace_id = EXCLUDED.workspace_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'workspace slug % belonged to another workspace', NEW.slug
            USING ERRCODE = '23505', CONSTRAINT = 'workspace_slugs_pkey';
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER organizations_slug_history
AFTER INSERT OR UPDATE OF slug ON organizations
FOR EACH ROW EXECUTE FUNCTION organizations_slug_history();
