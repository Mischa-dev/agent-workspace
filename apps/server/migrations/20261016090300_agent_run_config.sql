-- H4: the settings a run used (harness, model, reasoning level, speed,
-- attempts, sandbox, approval policy and adapter options), resolved when it
-- is created (or, for runs created without them, once when it starts) and
-- frozen from then on. Accounts carry their owner's defaults
-- until workspace and project settings (S2) join the cascade.

ALTER TABLE agent_runs
    ADD COLUMN config JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD CONSTRAINT agent_runs_config_shape_check CHECK (
        jsonb_typeof(config) = 'object'
        AND octet_length(config::text) <= 16384
    );

CREATE OR REPLACE FUNCTION enforce_agent_run_config_frozen()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.config IS DISTINCT FROM OLD.config
       AND OLD.config <> '{}'::jsonb THEN
        RAISE EXCEPTION 'Agent Run settings are frozen once they are set'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER agent_runs_freeze_config
BEFORE UPDATE OF config
ON agent_runs
FOR EACH ROW
EXECUTE FUNCTION enforce_agent_run_config_frozen();

ALTER TABLE provider_accounts
    ADD COLUMN run_defaults JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD CONSTRAINT provider_accounts_run_defaults_shape_check CHECK (
        jsonb_typeof(run_defaults) = 'object'
        AND octet_length(run_defaults::text) <= 8192
    );
