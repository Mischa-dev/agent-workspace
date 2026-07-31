-- Reconcile the M2 mention projection from one row per AST node to one row per
-- distinct mentioned Actor. The AST remains authoritative. This additive
-- migration preserves the immutable 0005 checksum already validated on the
-- Red Hat host while making repeated mentions idempotent for command handlers.

DROP TRIGGER message_mentions_validate ON message_mentions;
DROP FUNCTION validate_message_mention();

ALTER TABLE message_mentions
    DROP CONSTRAINT message_mentions_pkey;

ALTER TABLE message_mentions
    DROP COLUMN IF EXISTS node_index;

DELETE FROM message_mentions duplicate
USING message_mentions retained
WHERE duplicate.message_id = retained.message_id
  AND duplicate.actor_id = retained.actor_id
  AND duplicate.ctid > retained.ctid;

ALTER TABLE message_mentions
    ADD PRIMARY KEY (message_id, actor_id);

CREATE OR REPLACE FUNCTION validate_message_mention()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    ast_contains_actor BOOLEAN;
    mentionable BOOLEAN;
BEGIN
    SELECT EXISTS (
               SELECT 1
                 FROM jsonb_array_elements(message.content->'nodes') AS node
                WHERE node->>'type' = 'mention'
                  AND COALESCE(node->>'actor_id', node->>'actorId') = NEW.actor_id::text
           )
      INTO ast_contains_actor
      FROM messages message
     WHERE message.id = NEW.message_id
       AND message.project_id = NEW.project_id
       FOR KEY SHARE;

    IF ast_contains_actor IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Message mention projection must match its AST'
            USING ERRCODE = '23514';
    END IF;

    SELECT membership.enabled AND actor.status = 'active'
      INTO mentionable
      FROM project_memberships membership
      JOIN actors actor ON actor.id = membership.actor_id
     WHERE membership.project_id = NEW.project_id
       AND membership.actor_id = NEW.actor_id
     FOR KEY SHARE OF membership, actor;

    IF mentionable IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Mentioned Actor must have an enabled active Project membership'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER message_mentions_validate
BEFORE INSERT OR UPDATE
ON message_mentions
FOR EACH ROW
EXECUTE FUNCTION validate_message_mention();
