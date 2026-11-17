-- A1.19: a pull request a run reported as a result is picked up by its
-- space even when its branch names no task, and tied to the run's thread
-- and task (code_host::pull_requests::link_runs_reporting).

CREATE INDEX run_activities_reported_pull_requests
    ON run_activities (lower(payload ->> 'url'))
    WHERE kind = 'result' AND payload ->> 'kind' = 'pull_request';

CREATE FUNCTION github_job_reported_by_run(
    p_project_id UUID,
    p_repository_id UUID,
    p_payload JSONB
)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT COALESCE((p_payload ->> 'number') ~ '^[1-9][0-9]{0,17}$', false)
       AND EXISTS (
           SELECT 1
           FROM repositories repository
           JOIN run_activities result
             ON result.kind = 'result'
            AND result.payload ->> 'kind' = 'pull_request'
            AND lower(result.payload ->> 'url') = lower(
                    'https://github.com/' || repository.owner || '/' || repository.name
                    || '/pull/' || (p_payload ->> 'number'))
           JOIN runs run ON run.id = result.run_id AND run.space_id = p_project_id
           WHERE repository.project_id = p_project_id
             AND repository.id = p_repository_id
       )
$$;
