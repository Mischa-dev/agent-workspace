-- DMs and groups now live in the direct scope (K4.15), not in a workspace,
-- so a link between a DM (or a message in one) and a task, page or record
-- crosses from the direct scope into a workspace. Such a link is allowed and
-- belongs to the workspace side; a link between two different workspaces
-- still isn't made. Who may see either end is decided when it is read.
CREATE OR REPLACE FUNCTION links_put(
    p_from UUID, p_to UUID, p_kind TEXT, p_origin TEXT,
    p_source_kind TEXT, p_source_id UUID, p_field_id UUID, p_by UUID
) RETURNS void LANGUAGE sql AS $$
    INSERT INTO links (workspace_id, from_thing_id, to_thing_id, kind, origin,
                       source_kind, source_id, field_id, created_by_account_id)
    SELECT CASE WHEN source.workspace_id = direct_scope_id() THEN target.workspace_id
                ELSE source.workspace_id END,
           source.id, target.id, p_kind, p_origin,
           p_source_kind, p_source_id, p_field_id,
           (SELECT id FROM actors WHERE id = p_by)
    FROM things source
    JOIN things target ON target.id = p_to
     AND (target.workspace_id = source.workspace_id
          OR direct_scope_id() IN (source.workspace_id, target.workspace_id))
    WHERE source.id = p_from AND p_from <> p_to
    ON CONFLICT DO NOTHING
$$;
