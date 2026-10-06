-- A project's default worker can be created before anyone's Codex account is
-- signed in (AWD-11). It waits, disabled, until a profile on its provider
-- becomes available, then switches to that profile and turns on. A worker an
-- admin turned off is never pending, so it stays off.
ALTER TABLE execution_workers
    ADD COLUMN pending_setup BOOLEAN NOT NULL DEFAULT false,
    ADD CONSTRAINT execution_workers_pending_setup_disabled
        CHECK (NOT (pending_setup AND enabled));
