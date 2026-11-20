-- M1.12: connected modules. An outside app becomes a module by publishing a
-- manifest at its own address; nothing it supplies runs in this server or
-- in the app's pages.
--
-- * module_apps: one row per connected module (a `modules` row of kind
--   'connected'): the app account it acts as, the role made for it holding
--   exactly the approved permissions, the manifest as approved, and a
--   re-fetched one that asks for more, waiting for approval.
-- * The signing secrets live in `secrets` (sealed, the workspace's, made
--   by the server); this row only names them. Revoking deletes them.
-- * The one-time install code is kept as a digest. The app gets it at the
--   install address its manifest names and exchanges it once for its
--   account's secret and the signing secrets, so it proves it controls the
--   manifest's address before it gets anything.

CREATE TABLE module_apps (
    module_id UUID PRIMARY KEY REFERENCES modules(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id),
    role_id UUID NOT NULL REFERENCES workspace_roles(id),
    manifest_url TEXT NOT NULL CHECK (char_length(manifest_url) BETWEEN 1 AND 2000),
    base_url TEXT NOT NULL CHECK (char_length(base_url) BETWEEN 1 AND 2000),
    manifest JSONB NOT NULL CHECK (
        jsonb_typeof(manifest) = 'object' AND octet_length(manifest::text) <= 131072
    ),
    manifest_digest TEXT NOT NULL CHECK (manifest_digest ~ '^[0-9a-f]{64}$'),
    manifest_fetched_at TIMESTAMPTZ NOT NULL,
    -- A re-fetched manifest that widens what the app may do (new scopes,
    -- frame origins or address), applied only once approved.
    pending_manifest JSONB CHECK (
        pending_manifest IS NULL OR (
            jsonb_typeof(pending_manifest) = 'object'
            AND octet_length(pending_manifest::text) <= 131072
        )
    ),
    pending_digest TEXT CHECK (pending_digest IS NULL OR pending_digest ~ '^[0-9a-f]{64}$'),
    frame_origins TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(frame_origins) <= 4),
    scopes_requested TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(scopes_requested) <= 100),
    scopes_approved TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(scopes_approved) <= 100),
    -- Workspace signs what it sends the app with this one, and checks what
    -- the app sends with the other. NULL once revoked.
    outbound_secret_id UUID REFERENCES secrets(id) ON DELETE SET NULL,
    inbound_secret_id UUID REFERENCES secrets(id) ON DELETE SET NULL,
    -- The account's secret, once the app has exchanged its install code.
    credential_id UUID,
    install_code_digest BYTEA CHECK (install_code_digest IS NULL OR octet_length(install_code_digest) = 32),
    install_code_expires_at TIMESTAMPTZ,
    install_sent_at TIMESTAMPTZ,
    approved_by_actor_id UUID NOT NULL REFERENCES actors(id),
    approved_at TIMESTAMPTZ NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'active', 'suspended', 'revoked')),
    revoked_by_actor_id UUID REFERENCES actors(id),
    revoked_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((state = 'revoked') = (revoked_at IS NOT NULL))
);

CREATE UNIQUE INDEX module_apps_account ON module_apps (account_id);
CREATE INDEX module_apps_workspace ON module_apps (workspace_id, state);
CREATE INDEX module_apps_role ON module_apps (role_id);
CREATE UNIQUE INDEX module_apps_install_code
    ON module_apps (install_code_digest) WHERE install_code_digest IS NOT NULL;

SELECT tenancy_protect('module_apps', 'workspace_id');
