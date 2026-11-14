-- A1.7: the requests record (A1 section 3.8). One record behind approvals,
-- questions and acceptance of proposals: something an account needs from
-- someone.
--
-- * requests: the record. raised_via_host_id is a plain UUID until A1.11
--   adds hosts. Who may answer: anyone holding answer_permission in the
--   space who can see the subject, narrowed to the named accounts, roles
--   or groups when any are named (requests::may_answer, A1.8).
-- * agent_approvals and person_questions are copied in with their ids, so
--   notifications, push actions and links keep working. Until A1.22 makes
--   them views, every writer of the old tables brings the copy up to date
--   in the same transaction (requests::legacy, in code: no trigger), using
--   requests_sync_approval and requests_sync_question below, which this
--   copy uses too.
-- * notifications.approval_id points at requests.

CREATE TABLE requests (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID REFERENCES projects(id),
    kind TEXT NOT NULL CHECK (kind IN ('approval', 'question', 'proposal')),
    raised_by_account_id UUID NOT NULL REFERENCES actors(id),
    raised_via_host_id UUID,
    run_id UUID REFERENCES runs(id),
    host_request_ref TEXT CHECK (
        host_request_ref IS NULL OR char_length(host_request_ref) BETWEEN 1 AND 512
    ),
    client_request_id TEXT CHECK (
        client_request_id IS NULL OR char_length(client_request_id) BETWEEN 1 AND 200
    ),
    -- What it is about: a proposal (C1), a sponsored run request, a thing.
    subject_kind TEXT CHECK (subject_kind IS NULL OR subject_kind ~ '^[a-z][a-z0-9_]{0,63}$'),
    subject_id UUID,
    conversation_id UUID,
    work_item_id UUID,
    title TEXT NOT NULL CHECK (char_length(btrim(title)) BETWEEN 1 AND 2000),
    detail TEXT CHECK (detail IS NULL OR char_length(detail) <= 8000),
    -- [{"id", "label", "style"?: "approve" | "deny"}]
    options JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(options) = 'array' AND jsonb_array_length(options) <= 8
        AND octet_length(options::text) <= 4000
    ),
    allow_text BOOLEAN NOT NULL DEFAULT false,
    answer_permission TEXT NOT NULL CHECK (answer_permission ~ '^[a-z]+\.[a-z_.]+$'),
    addressed_account_ids UUID[] NOT NULL DEFAULT '{}' CHECK (cardinality(addressed_account_ids) <= 20),
    addressed_role_keys TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(addressed_role_keys) <= 10),
    addressed_group_ids UUID[] NOT NULL DEFAULT '{}' CHECK (cardinality(addressed_group_ids) <= 10),
    status TEXT NOT NULL DEFAULT 'open' CHECK (
        status IN ('open', 'answered', 'expired', 'cancelled', 'superseded')
    ),
    answer JSONB CHECK (
        answer IS NULL OR (jsonb_typeof(answer) = 'object' AND octet_length(answer::text) <= 8192)
    ),
    answered_by_account_id UUID REFERENCES actors(id),
    answered_via_app_id UUID,
    answered_at TIMESTAMPTZ,
    -- The raiser (or its host) has the answer.
    delivered_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    on_expiry TEXT NOT NULL DEFAULT 'expire' CHECK (on_expiry IN ('expire', 'deny')),
    legacy_source TEXT CHECK (legacy_source IN ('agent_approval', 'person_question')),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (run_id, host_request_ref),
    UNIQUE (raised_by_account_id, client_request_id),
    FOREIGN KEY (space_id, conversation_id) REFERENCES conversations(project_id, id),
    FOREIGN KEY (space_id, work_item_id) REFERENCES work_items(project_id, id),
    CHECK ((status = 'answered') = (answered_at IS NOT NULL AND answer IS NOT NULL))
);

CREATE INDEX requests_open_space ON requests (space_id, created_at) WHERE status = 'open';
CREATE INDEX requests_open_addressed ON requests USING gin (addressed_account_ids)
    WHERE status = 'open';
CREATE INDEX requests_open_run ON requests (run_id) WHERE status = 'open';
CREATE INDEX requests_expiring ON requests (expires_at)
    WHERE status = 'open' AND expires_at IS NOT NULL;
CREATE INDEX requests_raised ON requests (raised_by_account_id, created_at DESC);

-- Bring the request copied from one agent_approvals row up to date.
CREATE FUNCTION requests_sync_approval(target UUID)
RETURNS void
LANGUAGE sql
AS $$
    INSERT INTO requests AS request (
        id, workspace_id, space_id, kind, raised_by_account_id, run_id, host_request_ref,
        subject_kind, subject_id, conversation_id, title, options, allow_text,
        answer_permission, addressed_account_ids, addressed_role_keys, status, answer,
        answered_by_account_id, answered_at, delivered_at, legacy_source, version,
        created_at, updated_at
    )
    SELECT approval.id, project.organization_id, project.id, approval.kind,
           approval.requesting_actor_id, approval.agent_run_id,
           left(approval.provider_request_id, 512),
           CASE WHEN approval.external_execution_request_id IS NOT NULL THEN 'run_request' END,
           approval.external_execution_request_id,
           COALESCE(run.conversation_id, external_request.conversation_id),
           COALESCE(NULLIF(btrim(left(approval.summary, 2000)), ''),
                    CASE approval.kind WHEN 'question' THEN 'A question' ELSE 'An approval' END),
           CASE WHEN jsonb_typeof(approval.options) = 'array'
                     AND jsonb_array_length(approval.options) <= 8
                     AND octet_length(approval.options::text) <= 4000
                THEN approval.options ELSE '[]'::jsonb END,
           approval.kind = 'question',
           CASE approval.kind WHEN 'question' THEN 'requests.answer' ELSE 'runs.approve' END,
           CASE WHEN approval.eligible_actor_id IS NOT NULL
                THEN ARRAY[approval.eligible_actor_id] ELSE '{}'::uuid[] END,
           CASE WHEN approval.eligible_actor_id IS NULL
                     AND approval.eligible_role ~ '^[a-z][a-z0-9-]{0,39}$'
                THEN ARRAY[approval.eligible_role] ELSE '{}'::text[] END,
           CASE approval.status
                WHEN 'pending' THEN 'open'
                WHEN 'resolved' THEN 'answered'
                WHEN 'superseded' THEN 'superseded'
                ELSE 'cancelled' END,
           CASE WHEN approval.status = 'resolved' THEN
                CASE WHEN jsonb_typeof(approval.response) = 'object'
                          AND octet_length(approval.response::text) <= 8192
                     THEN approval.response
                     ELSE jsonb_build_object('legacy', true) END END,
           CASE WHEN approval.status = 'resolved' THEN approval.responded_by_actor_id END,
           CASE WHEN approval.status = 'resolved'
                THEN COALESCE(approval.responded_at, approval.created_at) END,
           approval.provider_response_delivered_at,
           'agent_approval', approval.version, approval.created_at,
           COALESCE(approval.responded_at, approval.created_at)
    FROM agent_approvals approval
    LEFT JOIN agent_runs run ON run.id = approval.agent_run_id
    LEFT JOIN external_execution_requests external_request
           ON external_request.id = approval.external_execution_request_id
    JOIN projects project
      ON project.id = COALESCE(run.project_id, external_request.project_id)
    WHERE approval.id = target
    ON CONFLICT (id) DO UPDATE SET
        status = EXCLUDED.status,
        answer = EXCLUDED.answer,
        answered_by_account_id = EXCLUDED.answered_by_account_id,
        answered_at = EXCLUDED.answered_at,
        delivered_at = EXCLUDED.delivered_at,
        version = GREATEST(request.version + 1, EXCLUDED.version),
        updated_at = now()
    WHERE (request.status, request.answer, request.answered_by_account_id,
           request.answered_at, request.delivered_at)
          IS DISTINCT FROM
          (EXCLUDED.status, EXCLUDED.answer, EXCLUDED.answered_by_account_id,
           EXCLUDED.answered_at, EXCLUDED.delivered_at)
$$;

-- Bring the request copied from one person_questions row up to date.
CREATE FUNCTION requests_sync_question(target UUID)
RETURNS void
LANGUAGE sql
AS $$
    INSERT INTO requests AS request (
        id, workspace_id, space_id, kind, raised_by_account_id, conversation_id,
        work_item_id, title, options, allow_text, answer_permission,
        addressed_account_ids, status, answer, answered_by_account_id, answered_at,
        legacy_source, created_at, updated_at
    )
    SELECT question.id, project.organization_id, project.id, 'question',
           question.asked_by_actor_id, question.conversation_id, question.work_item_id,
           question.question, question.options, true, 'requests.answer',
           ARRAY[question.person_actor_id],
           CASE question.status WHEN 'answered' THEN 'answered' ELSE 'open' END,
           CASE WHEN question.status = 'answered' THEN jsonb_strip_nulls(jsonb_build_object(
                'optionId', question.answer_option_id, 'text', question.answer_text)) END,
           CASE WHEN question.status = 'answered' THEN question.person_actor_id END,
           question.answered_at, 'person_question', question.created_at, question.updated_at
    FROM person_questions question
    JOIN projects project ON project.id = question.project_id
    WHERE question.id = target
    ON CONFLICT (id) DO UPDATE SET
        status = EXCLUDED.status,
        answer = EXCLUDED.answer,
        answered_by_account_id = EXCLUDED.answered_by_account_id,
        answered_at = EXCLUDED.answered_at,
        version = request.version + 1,
        updated_at = now()
    WHERE (request.status, request.answer, request.answered_by_account_id, request.answered_at)
          IS DISTINCT FROM
          (EXCLUDED.status, EXCLUDED.answer, EXCLUDED.answered_by_account_id,
           EXCLUDED.answered_at)
$$;

SELECT requests_sync_approval(id) FROM agent_approvals ORDER BY id;
SELECT requests_sync_question(id) FROM person_questions ORDER BY id;

ALTER TABLE notifications DROP CONSTRAINT notifications_approval_id_fkey;
ALTER TABLE notifications
    ADD CONSTRAINT notifications_request_fk
        FOREIGN KEY (approval_id) REFERENCES requests(id) ON DELETE SET NULL NOT VALID;
ALTER TABLE notifications VALIDATE CONSTRAINT notifications_request_fk;
