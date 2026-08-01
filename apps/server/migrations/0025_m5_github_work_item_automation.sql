-- M5 Project-owned GitHub automation. Only two fixed semantic rules exist:
-- opening an explicitly linked pull request may move work to Review (off by
-- default), while merging it may move eligible work to Verification (on by
-- default). Neither rule can mark work completed or reopen terminal work.

CREATE TABLE project_github_automation_settings (
    project_id UUID PRIMARY KEY REFERENCES projects(id),
    pull_request_open_enabled BOOLEAN NOT NULL DEFAULT false,
    pull_request_merge_enabled BOOLEAN NOT NULL DEFAULT true,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO project_github_automation_settings (project_id)
SELECT id FROM projects
ON CONFLICT DO NOTHING;

CREATE OR REPLACE FUNCTION create_default_project_github_automation_settings()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO project_github_automation_settings (project_id)
    VALUES (NEW.id)
    ON CONFLICT DO NOTHING;
    RETURN NEW;
END;
$$;

CREATE TRIGGER projects_create_default_github_automation_settings
AFTER INSERT ON projects
FOR EACH ROW EXECUTE FUNCTION create_default_project_github_automation_settings();

ALTER TABLE github_pull_requests
    ADD COLUMN automation_state TEXT NOT NULL DEFAULT 'not_evaluated' CHECK (
        automation_state IN ('not_evaluated', 'applied', 'skipped')
    ),
    ADD COLUMN automation_reason TEXT CHECK (
        automation_reason IS NULL OR (
            char_length(automation_reason) BETWEEN 1 AND 64
            AND automation_reason ~ '^[a-z][a-z0-9_]*$'
        )
    ),
    ADD COLUMN automation_evaluated_at TIMESTAMPTZ,
    ADD CONSTRAINT github_pull_requests_automation_shape_check CHECK (
        (automation_state = 'not_evaluated'
            AND automation_reason IS NULL
            AND automation_evaluated_at IS NULL)
        OR (automation_state = 'applied'
            AND automation_reason IS NULL
            AND automation_evaluated_at IS NOT NULL)
        OR (automation_state = 'skipped'
            AND automation_reason IS NOT NULL
            AND automation_evaluated_at IS NOT NULL)
    );

CREATE TABLE github_pull_request_automation_results (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    github_pull_request_id UUID NOT NULL,
    pull_request_version BIGINT NOT NULL CHECK (pull_request_version > 0),
    work_item_id UUID NOT NULL,
    rule TEXT NOT NULL CHECK (rule IN ('pull_request_open', 'pull_request_merge')),
    outcome TEXT NOT NULL CHECK (outcome IN ('applied', 'skipped')),
    reason TEXT CHECK (
        reason IS NULL OR (
            char_length(reason) BETWEEN 1 AND 64
            AND reason ~ '^[a-z][a-z0-9_]*$'
        )
    ),
    previous_status_id UUID NOT NULL,
    new_status_id UUID,
    integration_actor_id UUID NOT NULL,
    work_item_activity_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (github_pull_request_id, pull_request_version, work_item_id, rule),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, github_pull_request_id)
        REFERENCES github_pull_requests(project_id, id),
    FOREIGN KEY (project_id, work_item_id)
        REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, previous_status_id)
        REFERENCES work_item_statuses(project_id, id),
    FOREIGN KEY (project_id, new_status_id)
        REFERENCES work_item_statuses(project_id, id),
    FOREIGN KEY (project_id, integration_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, work_item_id, work_item_activity_id)
        REFERENCES work_item_activity(project_id, work_item_id, id),
    CHECK (
        (outcome = 'applied' AND reason IS NULL
            AND new_status_id IS NOT NULL AND work_item_activity_id IS NOT NULL)
        OR (outcome = 'skipped' AND reason IS NOT NULL
            AND new_status_id IS NULL AND work_item_activity_id IS NULL)
    )
);

ALTER TABLE work_item_activity
    DROP CONSTRAINT work_item_activity_kind_check,
    ADD CONSTRAINT work_item_activity_kind_check CHECK (kind IN (
        'work_item_created', 'work_item_updated', 'work_item_archived',
        'work_item_restored', 'criterion_created', 'criterion_updated',
        'criterion_completed', 'criterion_reopened', 'criterion_removed',
        'comment_created', 'thread_linked', 'thread_unlinked',
        'repository_linked', 'repository_unlinked', 'repository_primary_changed',
        'worker_invoked', 'github_automation_transition'
    ));

CREATE INDEX github_pull_request_automation_results_item
    ON github_pull_request_automation_results (project_id, work_item_id, created_at DESC);
