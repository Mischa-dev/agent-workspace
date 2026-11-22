-- Adding a space membership checked its account's kind under FOR UPDATE.
-- That waits on any open transaction that merely references the account
-- (a request id reserved for the call is one), so an operation that keeps
-- its retry key in one transaction and makes a space in another (using a
-- workspace template) waited on itself forever. FOR SHARE still stops the
-- kind changing while the check runs, and doesn't wait on a reference.
CREATE OR REPLACE FUNCTION validate_project_membership_actor_role()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    actor_kind TEXT;
BEGIN
    SELECT kind
      INTO actor_kind
      FROM actors
     WHERE id = NEW.actor_id
       FOR SHARE;

    IF actor_kind IS NOT NULL
       AND NEW.role = 'worker'
       AND actor_kind <> 'execution_worker' THEN
        RAISE EXCEPTION 'The Worker Project role is only for profile agents'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;
