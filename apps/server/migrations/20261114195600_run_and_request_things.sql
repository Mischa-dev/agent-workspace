-- K3.11 (build 1E), the rest: runs and requests (build 1G's A1 tables) as
-- kinds in the thing registry, kept like the others (a source view, the
-- shared trigger, the backfill job). There is no trigger on the legacy
-- `agent_runs`: a run reaches `things` by one route only.
--
-- A request's title stays NULL: a question to a person is content, like a
-- message, so nothing of it is copied here. A run's or request's
-- conversation, when it has one, governs who sees it.

INSERT INTO thing_kinds (kind, module) VALUES
    ('run', 'runs'),
    ('request', 'runs');

CREATE VIEW thing_source_runs AS
SELECT run.id, 'run'::text AS kind, run.workspace_id, run.space_id,
       thing_if_registered(COALESCE(run.work_item_id, run.conversation_id)) AS parent_id,
       thing_if_registered(run.conversation_id) AS access_parent_id,
       left(run.title, 300) AS title, thing_status_key(run.state) AS status,
       run.account_id AS owner_account_id,
       COALESCE(run.requested_by_account_id, run.account_id) AS created_by_account_id,
       NULL::uuid AS updated_by_account_id, run.version,
       run.created_at, run.updated_at, NULL::timestamptz AS archived_at,
       NULL::timestamptz AS deleted_at
FROM runs run;

CREATE VIEW thing_source_requests AS
SELECT request.id, 'request'::text AS kind, request.workspace_id, request.space_id,
       thing_if_registered(COALESCE(request.run_id, request.work_item_id, request.conversation_id))
           AS parent_id,
       thing_if_registered(request.conversation_id) AS access_parent_id,
       NULL::text AS title, thing_status_key(request.status) AS status,
       request.raised_by_account_id AS owner_account_id,
       request.raised_by_account_id AS created_by_account_id,
       request.answered_by_account_id AS updated_by_account_id, request.version,
       request.created_at, request.updated_at, NULL::timestamptz AS archived_at,
       NULL::timestamptz AS deleted_at
FROM requests request;

CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    workspace_id, space_id, conversation_id, work_item_id, title, state, account_id,
    requested_by_account_id, version, updated_at
    ON runs FOR EACH ROW EXECUTE FUNCTION things_sync('thing_source_runs', '{run}');
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    workspace_id, space_id, conversation_id, work_item_id, run_id, status,
    answered_by_account_id, version, updated_at
    ON requests FOR EACH ROW EXECUTE FUNCTION things_sync('thing_source_requests', '{request}');
