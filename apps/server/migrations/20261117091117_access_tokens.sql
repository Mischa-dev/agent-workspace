-- K1.24: narrow tokens. A1's interim run credential becomes the general
-- short-lived access token: one account, one workspace, a home space, and
-- optionally a narrower set of permissions and spaces than the account
-- holds. A run's token is one kind of use (run_id, host_id and command_id
-- set together); anything that needs a short, narrow token for an account
-- issues one through `access::tokens::issue_scoped`.
--
-- Run tokens already issued live an hour at most, so none is converted:
-- they keep working under the new name until they lapse.
ALTER TABLE run_credentials RENAME TO access_tokens;

ALTER TABLE access_tokens ALTER COLUMN run_id DROP NOT NULL;
ALTER TABLE access_tokens ALTER COLUMN host_id DROP NOT NULL;
ALTER TABLE access_tokens ALTER COLUMN command_id DROP NOT NULL;

ALTER TABLE access_tokens
    -- NULL: everything the account holds where the token reaches.
    ADD COLUMN permissions TEXT[] CHECK (permissions IS NULL OR cardinality(permissions) <= 200),
    -- NULL: every space of the workspace the account is in; otherwise only
    -- these (the home space among them).
    ADD COLUMN spaces UUID[] CHECK (spaces IS NULL OR cardinality(spaces) BETWEEN 1 AND 200),
    ADD COLUMN requested_by_actor_id UUID REFERENCES actors(id),
    ADD CONSTRAINT access_tokens_short CHECK (expires_at <= created_at + interval '24 hours'),
    ADD CONSTRAINT access_tokens_run_whole CHECK (
        (run_id IS NULL AND host_id IS NULL AND command_id IS NULL)
        OR (run_id IS NOT NULL AND host_id IS NOT NULL AND command_id IS NOT NULL)
    ),
    ADD CONSTRAINT access_tokens_home_in_spaces CHECK (spaces IS NULL OR space_id = ANY (spaces));

ALTER INDEX run_credentials_run RENAME TO access_tokens_run;
ALTER INDEX run_credentials_workspace RENAME TO access_tokens_workspace;
CREATE INDEX access_tokens_account ON access_tokens (account_id) WHERE revoked_at IS NULL;
