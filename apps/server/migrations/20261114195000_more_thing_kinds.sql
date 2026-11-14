-- K3.11 (build 1E): more kinds in the thing registry (docs/plans/master/
-- plans/K3-things-links-events.md K3.11): messages, files, milestones, pull
-- requests, repositories and automations, kept the way K3.10's kinds are (a
-- source view per table, the shared trigger, the backfill job).
--
-- A message's title stays NULL: its text is content that removal scrubs, so
-- nothing of it is copied here; a removed message keeps its row as a
-- tombstone (`deleted_at`). A file's parent is the conversation or task it
-- is attached to, and so is whose visibility governs it.
--
-- The `run` and `request` kinds (on A1's `runs` and `requests`) are added by
-- whichever of builds 1E and 1G merges second: those tables aren't here yet.

INSERT INTO thing_kinds (kind, module) VALUES
    ('message', 'messages'),
    ('file', 'workspace'),
    ('milestone', 'tasks'),
    ('pull_request', 'code'),
    ('repository', 'code'),
    ('automation', 'automations');

CREATE VIEW thing_source_messages AS
SELECT message.id, 'message'::text AS kind, project.organization_id AS workspace_id,
       message.project_id AS space_id, thing_if_registered(message.conversation_id) AS parent_id,
       thing_if_registered(message.conversation_id) AS access_parent_id,
       NULL::text AS title, NULL::text AS status,
       message.author_actor_id AS owner_account_id,
       message.author_actor_id AS created_by_account_id,
       message.removed_by_actor_id AS updated_by_account_id, message.revision::bigint AS version,
       message.created_at, COALESCE(message.removed_at, message.edited_at, message.created_at) AS updated_at,
       NULL::timestamptz AS archived_at, message.removed_at AS deleted_at
FROM messages message
JOIN projects project ON project.id = message.project_id;

CREATE VIEW thing_source_files AS
SELECT file.id, 'file'::text AS kind, file.organization_id AS workspace_id,
       file.project_id AS space_id,
       thing_if_registered(COALESCE(file.conversation_id, file.work_item_id)) AS parent_id,
       thing_if_registered(COALESCE(file.conversation_id, file.work_item_id)) AS access_parent_id,
       left(file.name, 300) AS title, NULL::text AS status,
       file.uploaded_by_actor_id AS owner_account_id,
       file.uploaded_by_actor_id AS created_by_account_id,
       file.removed_by_actor_id AS updated_by_account_id, 1::bigint AS version,
       file.created_at, COALESCE(file.removed_at, file.attached_at, file.created_at) AS updated_at,
       NULL::timestamptz AS archived_at, file.removed_at AS deleted_at
FROM files file;

CREATE VIEW thing_source_project_milestones AS
SELECT milestone.id, 'milestone'::text AS kind, project.organization_id AS workspace_id,
       milestone.project_id AS space_id, NULL::uuid AS parent_id, NULL::uuid AS access_parent_id,
       left(milestone.name, 300) AS title, NULL::text AS status,
       milestone.created_by_actor_id AS owner_account_id,
       milestone.created_by_actor_id AS created_by_account_id,
       NULL::uuid AS updated_by_account_id, milestone.version,
       milestone.created_at, milestone.updated_at, milestone.archived_at,
       NULL::timestamptz AS deleted_at
FROM project_milestones milestone
JOIN projects project ON project.id = milestone.project_id;

CREATE VIEW thing_source_github_pull_requests AS
SELECT pull.id, 'pull_request'::text AS kind, project.organization_id AS workspace_id,
       pull.project_id AS space_id, thing_if_registered(pull.repository_id) AS parent_id,
       NULL::uuid AS access_parent_id, left(pull.title, 300) AS title,
       thing_status_key(CASE WHEN pull.merged_at IS NOT NULL THEN 'merged' ELSE pull.state END) AS status,
       NULL::uuid AS owner_account_id, NULL::uuid AS created_by_account_id,
       NULL::uuid AS updated_by_account_id, pull.version,
       pull.created_at, pull.updated_at, NULL::timestamptz AS archived_at,
       NULL::timestamptz AS deleted_at
FROM github_pull_requests pull
JOIN projects project ON project.id = pull.project_id;

CREATE VIEW thing_source_repositories AS
SELECT repository.id, 'repository'::text AS kind, project.organization_id AS workspace_id,
       repository.project_id AS space_id, NULL::uuid AS parent_id, NULL::uuid AS access_parent_id,
       left(repository.owner || '/' || repository.name, 300) AS title,
       CASE WHEN repository.enabled THEN 'enabled' ELSE 'disabled' END AS status,
       NULL::uuid AS owner_account_id, NULL::uuid AS created_by_account_id,
       NULL::uuid AS updated_by_account_id, repository.version,
       repository.created_at, repository.updated_at, repository.archived_at,
       NULL::timestamptz AS deleted_at
FROM repositories repository
JOIN projects project ON project.id = repository.project_id;

CREATE VIEW thing_source_automation_rules AS
SELECT rule.id, 'automation'::text AS kind, rule.organization_id AS workspace_id,
       rule.project_id AS space_id, NULL::uuid AS parent_id, NULL::uuid AS access_parent_id,
       left(rule.name, 300) AS title,
       CASE WHEN rule.enabled THEN 'enabled' ELSE 'disabled' END AS status,
       rule.owner_actor_id AS owner_account_id, rule.owner_actor_id AS created_by_account_id,
       NULL::uuid AS updated_by_account_id, rule.version,
       rule.created_at, rule.updated_at, NULL::timestamptz AS archived_at,
       NULL::timestamptz AS deleted_at
FROM automation_rules rule;

CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    conversation_id, project_id, revision, edited_at, removed_at
    ON messages FOR EACH ROW EXECUTE FUNCTION things_sync('thing_source_messages', '{message}');
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    conversation_id, work_item_id, name, attached_at, removed_at
    ON files FOR EACH ROW EXECUTE FUNCTION things_sync('thing_source_files', '{file}');
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    project_id, name, version, updated_at, archived_at
    ON project_milestones FOR EACH ROW
    EXECUTE FUNCTION things_sync('thing_source_project_milestones', '{milestone}');
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    project_id, repository_id, title, state, merged_at, version, updated_at
    ON github_pull_requests FOR EACH ROW
    EXECUTE FUNCTION things_sync('thing_source_github_pull_requests', '{pull_request}');
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    project_id, owner, name, enabled, version, updated_at, archived_at
    ON repositories FOR EACH ROW
    EXECUTE FUNCTION things_sync('thing_source_repositories', '{repository}');
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    organization_id, project_id, name, enabled, owner_actor_id, version, updated_at
    ON automation_rules FOR EACH ROW
    EXECUTE FUNCTION things_sync('thing_source_automation_rules', '{automation}');
