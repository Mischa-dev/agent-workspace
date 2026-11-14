-- AWD-59, slice 1: a pull request that names a task's key is suggested on
-- that task.
--
-- A pull request whose branch, title or body names one of the project's task
-- keys (AWD-59) gets an unconfirmed link to that task: `branch_key` when the
-- branch names it, `body_marker` when only the title or body does. Unconfirmed
-- links never move a task's status (only confirmed `explicit` links do); a
-- person confirms or dismisses them.
--
-- * `github_pull_request_dismissed_links` remembers a suggestion someone
--   dismissed, so the next sync of that pull request doesn't suggest it again.
-- * `github_suggest_task_links` matches the keys against the project's open
--   tasks and adds the unconfirmed links, branch keys first. It never touches
--   an existing link, and skips archived and dismissed ones.

CREATE TABLE github_pull_request_dismissed_links (
    project_id UUID NOT NULL,
    github_pull_request_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    dismissed_by_actor_id UUID NOT NULL,
    dismissed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (github_pull_request_id, work_item_id),
    FOREIGN KEY (project_id, github_pull_request_id)
        REFERENCES github_pull_requests(project_id, id),
    FOREIGN KEY (project_id, work_item_id)
        REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, dismissed_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE INDEX github_pull_request_dismissed_links_work_item
    ON github_pull_request_dismissed_links (project_id, work_item_id);

CREATE OR REPLACE FUNCTION github_suggest_task_links(
    p_project_id UUID,
    p_pull_request_id UUID,
    p_branch_keys TEXT[],
    p_body_keys TEXT[],
    p_actor_id UUID
)
RETURNS INTEGER
LANGUAGE plpgsql
AS $$
DECLARE
    added INTEGER := 0;
    batch INTEGER;
    source_name TEXT;
    candidate_keys TEXT[];
BEGIN
    FOREACH source_name IN ARRAY ARRAY['branch_key', 'body_marker'] LOOP
        candidate_keys := CASE source_name
            WHEN 'branch_key' THEN COALESCE(p_branch_keys, ARRAY[]::TEXT[])
            ELSE COALESCE(p_body_keys, ARRAY[]::TEXT[])
        END;
        IF cardinality(candidate_keys) > 20 THEN
            candidate_keys := candidate_keys[1:20];
        END IF;
        INSERT INTO github_pull_request_work_item_links (
            project_id, github_pull_request_id, work_item_id,
            source, confirmed, created_by_actor_id
        )
        SELECT DISTINCT p_project_id, p_pull_request_id, item.id,
               source_name, false, p_actor_id
        FROM unnest(candidate_keys) AS named(task_key)
        CROSS JOIN LATERAL (
            SELECT CASE
                WHEN upper(named.task_key) ~ '^[A-Z][A-Z0-9]{1,9}-[1-9][0-9]{0,8}$'
                THEN upper(named.task_key)
            END AS task_key
        ) checked
        JOIN projects project
          ON project.id = p_project_id
         AND project.project_key = split_part(checked.task_key, '-', 1)
        JOIN work_items item
          ON item.project_id = p_project_id
         AND item.number = CASE
                WHEN checked.task_key IS NOT NULL
                THEN split_part(checked.task_key, '-', 2)::BIGINT
             END
         AND item.archived_at IS NULL
        WHERE EXISTS (
                SELECT 1 FROM github_pull_requests pull_request
                WHERE pull_request.project_id = p_project_id
                  AND pull_request.id = p_pull_request_id
            )
          AND NOT EXISTS (
                SELECT 1 FROM github_pull_request_dismissed_links dismissed
                WHERE dismissed.github_pull_request_id = p_pull_request_id
                  AND dismissed.work_item_id = item.id
            )
        ON CONFLICT (github_pull_request_id, work_item_id) DO NOTHING;
        GET DIAGNOSTICS batch = ROW_COUNT;
        added := added + batch;
    END LOOP;
    RETURN added;
END;
$$;

-- Whether a webhook job's `task_keys` (the keys its pull request's branch,
-- title or body names) include one of the project's open tasks. Only such a
-- pull request is read from GitHub and picked up when the app isn't
-- tracking it yet.
CREATE OR REPLACE FUNCTION github_job_names_open_task(p_project_id UUID, p_payload JSONB)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT CASE
        WHEN jsonb_typeof(p_payload -> 'task_keys') IS DISTINCT FROM 'array' THEN false
        ELSE EXISTS (
            SELECT 1
            FROM jsonb_array_elements_text(p_payload -> 'task_keys')
                 WITH ORDINALITY AS named(task_key, position)
            CROSS JOIN LATERAL (
                SELECT CASE
                    WHEN named.position <= 20
                     AND upper(named.task_key) ~ '^[A-Z][A-Z0-9]{1,9}-[1-9][0-9]{0,8}$'
                    THEN upper(named.task_key)
                END AS task_key
            ) checked
            JOIN projects project
              ON project.id = p_project_id
             AND project.project_key = split_part(checked.task_key, '-', 1)
            JOIN work_items item
              ON item.project_id = p_project_id
             AND item.number = CASE
                    WHEN checked.task_key IS NOT NULL
                    THEN split_part(checked.task_key, '-', 2)::BIGINT
                 END
             AND item.archived_at IS NULL
        )
    END
$$;
