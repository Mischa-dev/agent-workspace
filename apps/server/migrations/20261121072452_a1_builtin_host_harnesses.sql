-- A1.17: the harnesses the built-in host runs, as it describes them. The
-- host sends its manifests (models, options, features) when it takes its
-- lease; the server offers harnesses and checks launches from these rows,
-- not from the host's own memory, so the host can run as its own program.
-- One row per harness kind for the whole install: the built-in host serves
-- every workspace (ruling R24). A workspace's registered harnesses stay in
-- custom_harnesses.
CREATE TABLE builtin_host_harnesses (
    kind TEXT PRIMARY KEY CHECK (kind ~ '^[a-z][a-z0-9_]{0,63}$'),
    manifest JSONB NOT NULL CHECK (jsonb_typeof(manifest) = 'object'),
    described_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
