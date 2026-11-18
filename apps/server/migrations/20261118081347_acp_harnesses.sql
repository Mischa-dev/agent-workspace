-- A1.21: a registered harness may be an agent spoken to over the Agent
-- Client Protocol, run on this server like a command-line harness: it has
-- a command, as a command-line harness does.
DO $$
DECLARE
    constraint_name TEXT;
BEGIN
    FOR constraint_name IN
        SELECT con.conname
        FROM pg_constraint con
        WHERE con.conrelid = 'custom_harnesses'::regclass
          AND con.contype = 'c'
          AND pg_get_constraintdef(con.oid) LIKE '%transport%'
          AND pg_get_constraintdef(con.oid) NOT LIKE '%runner_actor_id%'
    LOOP
        EXECUTE format('ALTER TABLE custom_harnesses DROP CONSTRAINT %I', constraint_name);
    END LOOP;
END $$;

ALTER TABLE custom_harnesses
    ADD CONSTRAINT custom_harnesses_transport_known
        CHECK (transport IN ('command', 'acp', 'runner')),
    ADD CONSTRAINT custom_harnesses_program_has_command
        CHECK ((transport IN ('command', 'acp')) = (command IS NOT NULL));
