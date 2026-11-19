-- K1.27 follow-up: what the roles given to an account's groups in a space
-- allow there, in one place. `access::holding::load` and the conversation
-- access row already add these to the account's own role; every check
-- built from a seat (`access::membership_allows`) reads them from here, so
-- someone whose own seat is a Viewer and whose group is a Member can do
-- what a Member does.
CREATE FUNCTION space_group_permissions(account UUID, space UUID)
RETURNS TEXT[]
LANGUAGE sql
STABLE
AS $$
    SELECT COALESCE(array_agg(DISTINCT granted.key), ARRAY[]::TEXT[])
    FROM role_grants grant_row
    JOIN account_groups grp
      ON grp.id = grant_row.group_id AND grp.archived_at IS NULL
    JOIN account_group_members member
      ON member.group_id = grant_row.group_id AND member.account_id = account
    CROSS JOIN LATERAL unnest(workspace_role_permissions(grant_row.role_id)) AS granted(key)
    WHERE grant_row.space_id = space
$$;
