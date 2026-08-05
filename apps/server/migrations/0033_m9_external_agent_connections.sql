-- M9 externally owned agent identity and digest-only connection credentials.

CREATE TABLE external_agent_connections (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    actor_id UUID NOT NULL REFERENCES actors(id),
    connection_name TEXT NOT NULL CHECK (
        char_length(connection_name) BETWEEN 1 AND 80
        AND connection_name ~ '^[A-Za-z0-9][A-Za-z0-9._ -]*$'
    ),
    runtime_kind TEXT NOT NULL CHECK (runtime_kind = 'hermes'),
    runtime_version TEXT NOT NULL CHECK (runtime_version = '0.20.0'),
    scopes TEXT[] NOT NULL CHECK (
        cardinality(scopes) BETWEEN 1 AND 5
        AND scopes <@ ARRAY[
            'read', 'write', 'events', 'approval_respond', 'worker_request'
        ]::TEXT[]
        AND scopes @> ARRAY['read']::TEXT[]
    ),
    status TEXT NOT NULL DEFAULT 'active' CHECK (
        status IN ('active', 'disabled', 'removed')
    ),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_by_actor_id UUID NOT NULL,
    updated_by_actor_id UUID NOT NULL,
    last_used_at TIMESTAMPTZ,
    last_error_category TEXT CHECK (
        last_error_category IS NULL
        OR (
            char_length(last_error_category) BETWEEN 1 AND 64
            AND last_error_category ~ '^[a-z][a-z0-9_]*$'
        )
    ),
    disabled_at TIMESTAMPTZ,
    removed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, actor_id),
    UNIQUE (project_id, connection_name),
    FOREIGN KEY (project_id, actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, created_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, updated_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    CHECK (
        (status = 'active' AND disabled_at IS NULL AND removed_at IS NULL)
        OR (status = 'disabled' AND disabled_at IS NOT NULL AND removed_at IS NULL)
        OR (status = 'removed' AND removed_at IS NOT NULL)
    )
);

CREATE INDEX external_agent_connections_project_status
    ON external_agent_connections (project_id, status, created_at, id);

CREATE INDEX external_agent_connections_activity
    ON external_agent_connections (last_used_at DESC)
    WHERE status = 'active';

CREATE FUNCTION validate_external_agent_connection()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    connection_actor_kind TEXT;
    connection_role TEXT;
    creator_kind TEXT;
    updater_kind TEXT;
BEGIN
    SELECT a.kind, pm.role
      INTO connection_actor_kind, connection_role
      FROM actors a
      JOIN project_memberships pm
        ON pm.project_id = NEW.project_id AND pm.actor_id = a.id
     WHERE a.id = NEW.actor_id
       FOR SHARE OF a, pm;

    SELECT kind INTO creator_kind
      FROM actors WHERE id = NEW.created_by_actor_id FOR SHARE;
    SELECT kind INTO updater_kind
      FROM actors WHERE id = NEW.updated_by_actor_id FOR SHARE;

    IF connection_actor_kind IS DISTINCT FROM 'external_agent'
       OR connection_role NOT IN ('member', 'viewer') THEN
        RAISE EXCEPTION 'external connection requires an external-agent member or viewer'
            USING ERRCODE = '23514';
    END IF;
    IF creator_kind IS DISTINCT FROM 'human'
       OR updater_kind IS DISTINCT FROM 'human' THEN
        RAISE EXCEPTION 'external connection management requires human provenance'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER external_agent_connections_validate
BEFORE INSERT OR UPDATE OF project_id, actor_id, created_by_actor_id, updated_by_actor_id
ON external_agent_connections
FOR EACH ROW
EXECUTE FUNCTION validate_external_agent_connection();

CREATE FUNCTION preserve_external_agent_actor_kind()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.kind <> 'external_agent'
       AND EXISTS (
           SELECT 1 FROM external_agent_connections WHERE actor_id = NEW.id
       ) THEN
        RAISE EXCEPTION 'an Actor with an external connection must remain external_agent'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER actors_preserve_external_agent_kind
BEFORE UPDATE OF kind
ON actors
FOR EACH ROW
EXECUTE FUNCTION preserve_external_agent_actor_kind();

ALTER TABLE mcp_client_credentials
    DROP CONSTRAINT mcp_client_credentials_token_prefix_check,
    DROP CONSTRAINT mcp_client_credentials_scopes_check,
    ADD COLUMN external_agent_connection_id UUID
        REFERENCES external_agent_connections(id),
    ADD CONSTRAINT mcp_client_credentials_token_prefix_check CHECK (
        char_length(token_prefix) BETWEEN 8 AND 24
        AND token_prefix ~ '^aw_(mcp|ext)_[A-Za-z0-9_-]+$'
    ),
    ADD CONSTRAINT mcp_client_credentials_scopes_check CHECK (
        cardinality(scopes) BETWEEN 1 AND 5
        AND scopes <@ ARRAY[
            'read', 'write', 'events', 'approval_respond', 'worker_request'
        ]::TEXT[]
        AND scopes @> ARRAY['read']::TEXT[]
    );

CREATE INDEX mcp_client_credentials_external_connection
    ON mcp_client_credentials (external_agent_connection_id, created_at DESC)
    WHERE external_agent_connection_id IS NOT NULL;

CREATE FUNCTION validate_mcp_credential_actor_class()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    credential_actor_kind TEXT;
    connection_project_id UUID;
    connection_actor_id UUID;
    connection_status TEXT;
    connection_scopes TEXT[];
BEGIN
    SELECT kind INTO credential_actor_kind
      FROM actors WHERE id = NEW.actor_id FOR SHARE;

    IF credential_actor_kind = 'human' THEN
        IF NEW.external_agent_connection_id IS NOT NULL
           OR NOT (NEW.scopes <@ ARRAY['read', 'write']::TEXT[]) THEN
            RAISE EXCEPTION 'a human MCP credential cannot reference an external connection'
                USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;

    IF credential_actor_kind = 'external_agent'
       AND NEW.external_agent_connection_id IS NOT NULL THEN
        SELECT project_id, actor_id, status, scopes
          INTO connection_project_id, connection_actor_id,
               connection_status, connection_scopes
          FROM external_agent_connections
         WHERE id = NEW.external_agent_connection_id
           FOR SHARE;
        IF connection_project_id IS NOT DISTINCT FROM NEW.project_id
           AND connection_actor_id IS NOT DISTINCT FROM NEW.actor_id
           AND connection_status = 'active'
           AND connection_scopes IS NOT DISTINCT FROM NEW.scopes THEN
            RETURN NEW;
        END IF;
    END IF;

    RAISE EXCEPTION 'MCP credential Actor class and external connection do not match'
        USING ERRCODE = '23514';
END;
$$;

CREATE TRIGGER mcp_client_credentials_validate_actor_class
BEFORE INSERT OR UPDATE OF project_id, actor_id, external_agent_connection_id, scopes
ON mcp_client_credentials
FOR EACH ROW
EXECUTE FUNCTION validate_mcp_credential_actor_class();

ALTER TABLE audit_events
    ADD COLUMN external_agent_connection_id UUID
        REFERENCES external_agent_connections(id);

CREATE INDEX audit_events_external_agent_connection
    ON audit_events (external_agent_connection_id, occurred_at DESC)
    WHERE external_agent_connection_id IS NOT NULL;
