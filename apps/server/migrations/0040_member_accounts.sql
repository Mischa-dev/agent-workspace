-- Stable organization profiles for people and agents. Existing actor IDs and
-- project memberships remain the identity and authorization boundaries.
CREATE TABLE organization_actor_profiles (
    organization_id UUID NOT NULL REFERENCES organizations(id),
    actor_id UUID NOT NULL REFERENCES actors(id),
    display_name TEXT NOT NULL CHECK (char_length(display_name) BETWEEN 1 AND 120),
    handle TEXT NOT NULL CHECK (handle ~ '^[a-z0-9][a-z0-9_-]{1,31}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, actor_id),
    UNIQUE (organization_id, handle)
);

INSERT INTO organization_actor_profiles (organization_id, actor_id, display_name, handle)
SELECT DISTINCT ON (p.organization_id, pm.actor_id)
    p.organization_id, pm.actor_id, pm.display_name, pm.handle
FROM project_memberships pm
JOIN projects p ON p.id = pm.project_id
JOIN actors a ON a.id = pm.actor_id
WHERE a.kind IN ('human', 'external_agent')
ORDER BY p.organization_id, pm.actor_id, (p.kind = 'direct') DESC, pm.created_at, pm.id;

-- Refuse ambiguous legacy names through the unique constraint above.
UPDATE project_memberships pm
SET display_name = profile.display_name, handle = profile.handle
FROM projects p, organization_actor_profiles profile
WHERE p.id = pm.project_id AND profile.organization_id = p.organization_id
  AND profile.actor_id = pm.actor_id;

CREATE FUNCTION canonical_member_profile() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE org UUID; canonical organization_actor_profiles%ROWTYPE;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM actors WHERE id = NEW.actor_id AND kind IN ('human', 'external_agent')) THEN
        RETURN NEW;
    END IF;
    SELECT organization_id INTO org FROM projects WHERE id = NEW.project_id;
    INSERT INTO organization_actor_profiles (organization_id, actor_id, display_name, handle)
    VALUES (org, NEW.actor_id, NEW.display_name, NEW.handle)
    ON CONFLICT (organization_id, actor_id) DO NOTHING;
    SELECT * INTO canonical FROM organization_actor_profiles
    WHERE organization_id = org AND actor_id = NEW.actor_id;
    NEW.display_name := canonical.display_name;
    NEW.handle := canonical.handle;
    RETURN NEW;
END;
$$;
CREATE TRIGGER project_memberships_canonical_profile
BEFORE INSERT OR UPDATE OF display_name, handle ON project_memberships
FOR EACH ROW EXECUTE FUNCTION canonical_member_profile();

CREATE TABLE workos_identities (
    client_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    user_id UUID NOT NULL REFERENCES users(id),
    email TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, subject_id),
    UNIQUE (client_id, user_id)
);
CREATE TABLE workos_login_attempts (
    state_hash BYTEA PRIMARY KEY CHECK (octet_length(state_hash) = 32),
    browser_hash BYTEA NOT NULL CHECK (octet_length(browser_hash) = 32),
    link_session_id UUID REFERENCES sessions(id),
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX workos_login_attempts_expiry ON workos_login_attempts(expires_at);
ALTER TABLE sessions ADD COLUMN workos_session_id TEXT;

-- The credential's home project remains its enrollment/revocation anchor.
-- An agent can additionally act in projects where an administrator added that
-- same actor, within the same organization. Human credentials are unchanged.
CREATE FUNCTION agent_connection_can_access(connection_uuid UUID, target_project UUID)
RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM external_agent_connections c
        JOIN projects home ON home.id = c.project_id AND home.archived_at IS NULL
        JOIN projects target ON target.id = target_project
          AND target.organization_id = home.organization_id AND target.archived_at IS NULL
        JOIN actors a ON a.id = c.actor_id AND a.kind = 'external_agent' AND a.status = 'active'
        JOIN organization_memberships om ON om.organization_id = home.organization_id
          AND om.actor_id = a.id AND om.enabled
        JOIN project_memberships anchor ON anchor.project_id = home.id
          AND anchor.actor_id = a.id AND anchor.enabled
        JOIN project_memberships member ON member.project_id = target.id
          AND member.actor_id = a.id AND member.enabled
        WHERE c.id = connection_uuid AND c.status = 'active'
    );
$$;

ALTER TABLE external_agent_events
    DROP CONSTRAINT external_agent_events_project_id_connection_id_fkey,
    ADD FOREIGN KEY (project_id) REFERENCES projects(id),
    ADD FOREIGN KEY (connection_id) REFERENCES external_agent_connections(id);

CREATE FUNCTION validate_agent_event_organization() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM external_agent_connections c
        JOIN projects home ON home.id = c.project_id
        JOIN projects target ON target.id = NEW.project_id AND target.organization_id = home.organization_id
        WHERE c.id = NEW.connection_id
    ) THEN RAISE EXCEPTION 'agent event cannot cross organizations' USING ERRCODE = '23514'; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER external_events_organization BEFORE INSERT OR UPDATE OF project_id, connection_id
ON external_agent_events FOR EACH ROW EXECUTE FUNCTION validate_agent_event_organization();

CREATE FUNCTION external_event_visible_to(connection_uuid UUID, resource_type TEXT, resource_uuid UUID)
RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT CASE resource_type
        WHEN 'message' THEN EXISTS (SELECT 1 FROM messages m WHERE m.id = resource_uuid AND conversation_visible_to(m.conversation_id, c.actor_id))
        WHEN 'reply_request' THEN EXISTS (SELECT 1 FROM reply_requests r WHERE r.id = resource_uuid AND r.agent_actor_id = c.actor_id AND conversation_visible_to(r.conversation_id, c.actor_id))
        WHEN 'agent_approval' THEN EXISTS (SELECT 1 FROM agent_approvals a JOIN agent_runs r ON r.id = a.agent_run_id WHERE a.id = resource_uuid AND conversation_visible_to(r.conversation_id, c.actor_id))
        WHEN 'conversation' THEN conversation_visible_to(resource_uuid, c.actor_id)
        ELSE true
    END FROM external_agent_connections c WHERE c.id = connection_uuid;
$$;

ALTER TABLE workos_login_attempts ADD COLUMN signup_username TEXT;
ALTER TABLE sessions ADD COLUMN workos_checked_at TIMESTAMPTZ;
