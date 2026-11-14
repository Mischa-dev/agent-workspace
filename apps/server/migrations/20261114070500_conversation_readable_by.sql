-- P0.28 (ruling R32): conversation_readable_by, a thin wrapper whose body is
-- one call to conversation_visible_to and nothing else, so it adds no rule
-- and can't drift. conversation_visible_to stays the one gate (its body is
-- K1's). Until K4 makes that rule whole it doesn't check workspace or
-- project membership, so a reader still pairs it with
-- actor_can_access_project or the membership join (AGENTS.md, rule 7).
-- The structure guard conversation_readable_by_is_one_call holds any later
-- definition of this function to the same one call.
CREATE FUNCTION conversation_readable_by(conversation_id UUID, actor_id UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$ SELECT conversation_visible_to(conversation_id, actor_id) $$;
