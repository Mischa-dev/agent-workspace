-- H3: each workspace's model catalog per harness adapter. Adapters that can
-- list their models refresh `provider_data`; people edit `overrides` (context
-- size, prices, names, hiding a model), which a refresh never touches. Adapters
-- that can't list models are filled in by hand (`provider_data` stays NULL).

CREATE TABLE harness_models (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    adapter_kind TEXT NOT NULL CHECK (adapter_kind ~ '^[a-z][a-z0-9_]{0,63}$'),
    model_id TEXT NOT NULL CHECK (
        char_length(model_id) BETWEEN 1 AND 200 AND model_id !~ '[[:cntrl:]]'
    ),
    provider_data JSONB CHECK (
        provider_data IS NULL
        OR (jsonb_typeof(provider_data) = 'object'
            AND octet_length(provider_data::text) <= 16384)
    ),
    overrides JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(overrides) = 'object'
        AND octet_length(overrides::text) <= 16384
    ),
    position INTEGER NOT NULL DEFAULT 0,
    refreshed_at TIMESTAMPTZ,
    edited_by_actor_id UUID REFERENCES actors(id),
    edited_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (organization_id, adapter_kind, model_id)
);

CREATE INDEX harness_models_listing
    ON harness_models (organization_id, adapter_kind, position, model_id);

CREATE TABLE harness_catalog_refreshes (
    organization_id UUID NOT NULL REFERENCES organizations(id),
    adapter_kind TEXT NOT NULL CHECK (adapter_kind ~ '^[a-z][a-z0-9_]{0,63}$'),
    refreshed_at TIMESTAMPTZ NOT NULL,
    model_count INTEGER NOT NULL CHECK (model_count >= 0),
    PRIMARY KEY (organization_id, adapter_kind)
);
