-- A1.17 step 5: the built-in host answers an account question about its
-- models with the account's catalog, which can outgrow the 64 KiB a host's
-- acknowledgement keeps. An action's answer may hold up to a mebibyte (as
-- JSON text, with the spaces PostgreSQL adds); it is deleted once read.
-- Every other acknowledgement keeps the old limit.
ALTER TABLE host_commands DROP CONSTRAINT host_commands_ack_check;
ALTER TABLE host_commands ADD CONSTRAINT host_commands_ack_check CHECK (
    ack IS NULL OR (
        jsonb_typeof(ack) = 'object'
        AND octet_length(ack::text) <= CASE WHEN kind = 'action' THEN 1400000 ELSE 70000 END
    )
);
