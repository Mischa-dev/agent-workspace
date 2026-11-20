-- H1.63 with K4.15: rows in the direct scope under strict row rules.
--
-- DMs and groups (and what hangs off them: messages, reads, events,
-- registry rows, links, files) now carry the direct scope as their
-- workspace. No workspace scope lists it, so strict row rules refused
-- them, starting with the DM a request had just made (its participants
-- are added after it, so the participant branch can't see it yet).
--
-- The direct scope belongs to no workspace and to every account in one, so
-- any account's scope and any workspace's scope may read and write its
-- rows: a DM's message is sent from a member scope, and its reply request,
-- run and notices are handled by jobs in the workspace the run lives in.
-- Which DM anyone may see is decided where every conversation read is
-- decided, `conversation_visible_to` (rule 7), as it is for a private
-- channel inside a workspace. Before sign-in no DM row is visible.
DO $migration$
BEGIN
    EXECUTE format($function$
        CREATE OR REPLACE FUNCTION tenancy_allows(workspace UUID) RETURNS BOOLEAN
        LANGUAGE sql STABLE PARALLEL SAFE AS $$
            SELECT (
                workspace = '00000000-0000-7000-8000-00000000d1ec'::UUID
                AND split_part(coalesce(current_setting('app.tenancy', true), ''), ':', 1) IN ('m', 'w')
            ) OR CASE split_part(coalesce(current_setting('app.tenancy', true), ''), ':', 1)
                WHEN 's' THEN true
                WHEN 'w' THEN
                    workspace = NULLIF(split_part(current_setting('app.tenancy', true), '|', 2), '')::UUID
                    OR %1$I.tenancy_refused(workspace)
                WHEN 'm' THEN
                    workspace = ANY (string_to_array(
                        NULLIF(split_part(current_setting('app.tenancy', true), '|', 2), ''), ','
                    )::UUID[])
                    OR %1$I.tenancy_refused(workspace)
                WHEN 'a' THEN %1$I.tenancy_refused(workspace)
                -- No scope: allowed while observing (the server logs the call site).
                ELSE coalesce(current_setting('app.tenancy_strict', true), '') <> 'on'
            END
        $$
    $function$, current_schema());
END;
$migration$;

-- The participant branch K4 put beside each conversation table's rule read
-- `conversations` and `conversation_members`, which carry the same rule:
-- with row rules binding (any role that isn't a superuser), a direct-scope
-- row the first branch refused was checked by reading those tables again,
-- which checked them again, until the stack ran out. Every scope with an
-- account or workspace is let in by `tenancy_allows` above, so the branch
-- adds nothing and refuses the rest without reading anything.
CREATE OR REPLACE FUNCTION tenancy_direct_allows(workspace UUID, conversation UUID)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT false
$$;
