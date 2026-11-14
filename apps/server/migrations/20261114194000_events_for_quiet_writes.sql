-- K3.8 (build 1E): events for the writes that had none (docs/plans/master/
-- plans/K3-things-links-events.md K3.8; G F5): decisions, conversation pins,
-- milestones, labels, saved views, files, person questions, task field
-- definitions, thread status, task comments and page comments.
--
-- They are emitted by a trigger on each table, so every write path (web,
-- MCP, operations, imports, automations) emits, the way the thing registry
-- is kept. `stage_event` writes one row to `event_staging` exactly as
-- `events::emit` does: who acted, the app and the run come from the
-- transaction's `aw.*` settings, else from the row. Payloads carry ids and
-- keys of what changed, never text (rule 10). Each name is in the Rust
-- catalogue (`events/catalog.rs`); a test keeps the two in step.

CREATE FUNCTION stage_event(
    p_type TEXT,
    p_class TEXT,
    p_space UUID,
    p_thing_kind TEXT,
    p_thing UUID,
    p_audience TEXT,
    p_actor UUID,
    p_conversation UUID DEFAULT NULL,
    p_account UUID DEFAULT NULL,
    p_access_thing UUID DEFAULT NULL,
    p_version BIGINT DEFAULT NULL,
    p_changes TEXT[] DEFAULT '{}',
    p_payload JSONB DEFAULT '{}'
) RETURNS VOID
LANGUAGE sql AS $$
    INSERT INTO event_staging (
        id, workspace_id, space_id, thing_id, thing_kind, access_thing_id, thing_version,
        type, class, changes, audience, conversation_id, conversation_restricted,
        audience_account_id, actor_account_id, via_credential_id, requested_by_account_id,
        run_id, request_id, payload
    )
    SELECT gen_random_uuid(), project.organization_id, p_space, p_thing, p_thing_kind,
           p_access_thing, p_version, p_type, p_class, p_changes, p_audience,
           CASE WHEN p_audience = 'conversation'
                THEN COALESCE(conversation.parent_channel_id, conversation.id) END,
           CASE WHEN p_audience = 'conversation'
                THEN COALESCE(parent.visibility, conversation.visibility) <> 'project' END,
           CASE WHEN p_audience = 'account' THEN p_account END,
           COALESCE(NULLIF(current_setting('aw.actor', true), '')::uuid, p_actor),
           NULLIF(current_setting('aw.via_grant', true), '')::uuid,
           NULLIF(current_setting('aw.requested_by', true), '')::uuid,
           NULLIF(current_setting('aw.run', true), '')::uuid,
           NULLIF(current_setting('aw.request_id', true), ''),
           p_payload
    FROM projects project
    -- visibility: only the root and its restriction are read, to route the event
    LEFT JOIN conversations conversation ON conversation.id = p_conversation
    LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
    WHERE project.id = p_space
      -- A conversation event needs its conversation (gone in a cascade).
      AND (p_audience <> 'conversation' OR conversation.id IS NOT NULL)
$$;

-- Decisions: recorded, then changed (superseded, edited). One from a
-- conversation is heard as that conversation is.
CREATE FUNCTION events_from_decisions() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM stage_event(
        CASE WHEN TG_OP = 'INSERT' THEN 'decision.recorded' ELSE 'decision.updated' END,
        'fact', NEW.project_id, 'decision', NEW.id,
        CASE WHEN NEW.source_conversation_id IS NULL THEN 'space' ELSE 'conversation' END,
        NEW.decided_by_actor_id, NEW.source_conversation_id, NULL, NEW.source_conversation_id,
        NULL, CASE WHEN TG_OP = 'INSERT' THEN '{}'::text[] ELSE '{status}'::text[] END);
    RETURN NULL;
END $$;
CREATE TRIGGER events_from_decisions AFTER INSERT OR UPDATE OF title, detail, status
    ON project_decisions FOR EACH ROW EXECUTE FUNCTION events_from_decisions();

-- Pins on a conversation.
CREATE FUNCTION events_from_conversation_pins() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    pin conversation_pins%ROWTYPE;
BEGIN
    IF TG_OP = 'DELETE' THEN pin := OLD; ELSE pin := NEW; END IF;
    PERFORM stage_event(
        'conversation.pins_changed', 'fact',
        (SELECT project_id FROM conversations WHERE id = pin.conversation_id),
        'conversation', pin.conversation_id, 'conversation', pin.pinned_by_actor_id,
        pin.conversation_id, NULL, NULL, NULL, '{pins}',
        jsonb_build_object('targetKind', pin.target_kind, 'targetId', pin.target_id,
                           'pinned', TG_OP <> 'DELETE'));
    RETURN NULL;
END $$;
CREATE TRIGGER events_from_conversation_pins AFTER INSERT OR DELETE
    ON conversation_pins FOR EACH ROW EXECUTE FUNCTION events_from_conversation_pins();

-- Milestones.
CREATE FUNCTION events_from_milestones() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM stage_event(
        CASE WHEN TG_OP = 'INSERT' THEN 'milestone.created' ELSE 'milestone.updated' END,
        'fact', NEW.project_id, 'milestone', NEW.id, 'space', NEW.created_by_actor_id,
        NULL, NULL, NULL, NEW.version);
    RETURN NULL;
END $$;
CREATE TRIGGER events_from_milestones
    AFTER INSERT OR UPDATE OF name, description, target_date, archived_at
    ON project_milestones FOR EACH ROW EXECUTE FUNCTION events_from_milestones();

-- A space's labels.
CREATE FUNCTION events_from_labels() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM stage_event(
        'label.changed', 'fact', NEW.project_id, 'label', NEW.id, 'space',
        NEW.created_by_actor_id, NULL, NULL, NULL, NEW.version);
    RETURN NULL;
END $$;
CREATE TRIGGER events_from_labels
    AFTER INSERT OR UPDATE OF name, color, description, archived_at
    ON project_labels FOR EACH ROW EXECUTE FUNCTION events_from_labels();

-- Saved views: a personal one is heard by its owner only.
CREATE FUNCTION events_from_saved_views() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    saved saved_views%ROWTYPE;
BEGIN
    IF TG_OP = 'DELETE' THEN saved := OLD; ELSE saved := NEW; END IF;
    PERFORM stage_event(
        'view.changed', 'fact', saved.project_id, 'view', saved.id,
        CASE WHEN saved.owner_actor_id IS NULL THEN 'space' ELSE 'account' END,
        saved.created_by_actor_id, NULL, saved.owner_actor_id, NULL, saved.version, '{}',
        jsonb_build_object('removed', TG_OP = 'DELETE'));
    RETURN NULL;
END $$;
CREATE TRIGGER events_from_saved_views
    AFTER INSERT OR DELETE OR UPDATE OF name, query, layout, fields, position
    ON saved_views FOR EACH ROW EXECUTE FUNCTION events_from_saved_views();

-- Files: attached, then removed. One in a conversation is heard as that
-- conversation is; one on a task as its space is.
CREATE FUNCTION events_from_files() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND (OLD.removed_at IS NOT NULL OR NEW.removed_at IS NULL) THEN
        RETURN NULL;
    END IF;
    PERFORM stage_event(
        CASE WHEN TG_OP = 'INSERT' THEN 'file.attached' ELSE 'file.removed' END,
        'fact', NEW.project_id, 'file', NEW.id,
        CASE WHEN NEW.conversation_id IS NULL THEN 'space' ELSE 'conversation' END,
        COALESCE(NEW.removed_by_actor_id, NEW.uploaded_by_actor_id), NEW.conversation_id,
        NULL, COALESCE(NEW.conversation_id, NEW.work_item_id), NULL, '{}',
        jsonb_strip_nulls(jsonb_build_object('workItemId', NEW.work_item_id,
                                             'messageId', NEW.message_id,
                                             'commentId', NEW.comment_id)));
    RETURN NULL;
END $$;
CREATE TRIGGER events_from_files AFTER INSERT OR UPDATE OF removed_at
    ON files FOR EACH ROW EXECUTE FUNCTION events_from_files();

-- Questions put to a person: they hear it asked; the asker hears it
-- answered.
CREATE FUNCTION events_from_person_questions() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        PERFORM stage_event(
            'question.asked', 'fact', NEW.project_id, 'question', NEW.id, 'account',
            NEW.asked_by_actor_id, NULL, NEW.person_actor_id, NEW.conversation_id);
    ELSIF OLD.status IS DISTINCT FROM NEW.status THEN
        PERFORM stage_event(
            'question.answered', 'fact', NEW.project_id, 'question', NEW.id, 'account',
            NEW.person_actor_id, NULL, NEW.asked_by_actor_id, NEW.conversation_id, NULL,
            '{status}');
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER events_from_person_questions AFTER INSERT OR UPDATE OF status
    ON person_questions FOR EACH ROW EXECUTE FUNCTION events_from_person_questions();

-- A space's task fields (a collection's fields change the collection).
CREATE FUNCTION events_from_field_definitions() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM stage_event(
        'field.changed', 'fact', NEW.project_id, 'field', NEW.id, 'space',
        NEW.created_by_actor_id, NULL, NULL, NULL, NEW.version);
    RETURN NULL;
END $$;
CREATE TRIGGER events_from_field_definitions
    AFTER INSERT OR UPDATE OF name, description, field_type, config, position, archived_at
    ON field_definitions FOR EACH ROW WHEN (NEW.collection_id IS NULL)
    EXECUTE FUNCTION events_from_field_definitions();

-- A thread's status.
CREATE FUNCTION events_from_thread_status() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM stage_event(
        'thread.status_changed', 'fact', NEW.project_id, 'thread', NEW.id, 'conversation',
        NEW.thread_status_changed_by_actor_id, NEW.id, NULL, NULL, NEW.version, '{status}');
    RETURN NULL;
END $$;
CREATE TRIGGER events_from_thread_status AFTER UPDATE OF thread_status, thread_outcome
    ON conversations FOR EACH ROW
    WHEN (NEW.kind = 'thread' AND (OLD.thread_status IS DISTINCT FROM NEW.thread_status
                                   OR OLD.thread_outcome IS DISTINCT FROM NEW.thread_outcome))
    EXECUTE FUNCTION events_from_thread_status();

-- Comments on a task: written, then edited or removed.
CREATE FUNCTION events_from_task_comments() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM stage_event(
        CASE WHEN TG_OP = 'INSERT' THEN 'task.commented' ELSE 'task.comment_changed' END,
        'fact', NEW.project_id, 'task', NEW.work_item_id, 'space',
        COALESCE(NEW.removed_by_actor_id, NEW.author_actor_id), NULL, NULL, NULL, NULL,
        CASE WHEN TG_OP = 'INSERT' THEN '{}'::text[]
             WHEN NEW.removed_at IS NOT NULL THEN '{removed}'::text[]
             ELSE '{content}'::text[] END,
        jsonb_build_object('commentId', NEW.id));
    RETURN NULL;
END $$;
CREATE TRIGGER events_from_task_comments AFTER INSERT OR UPDATE OF content, removed_at
    ON work_item_comments FOR EACH ROW EXECUTE FUNCTION events_from_task_comments();

-- Comments on a page: written, then edited, resolved or deleted.
CREATE FUNCTION events_from_page_comments() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM stage_event(
        CASE WHEN TG_OP = 'INSERT' THEN 'page.commented' ELSE 'page.comment_changed' END,
        'fact', NEW.project_id, 'page', NEW.page_id, 'space',
        COALESCE(NEW.resolved_by_actor_id, NEW.author_actor_id), NULL, NULL, NULL, NULL,
        CASE WHEN TG_OP = 'INSERT' THEN '{}'::text[]
             WHEN NEW.deleted_at IS NOT NULL THEN '{removed}'::text[]
             WHEN NEW.resolved_at IS DISTINCT FROM OLD.resolved_at THEN '{resolved}'::text[]
             ELSE '{body}'::text[] END,
        jsonb_build_object('commentId', NEW.id, 'threadId', NEW.thread_id));
    RETURN NULL;
END $$;
CREATE TRIGGER events_from_page_comments
    AFTER INSERT OR UPDATE OF body, resolved_at, deleted_at
    ON project_page_comments FOR EACH ROW EXECUTE FUNCTION events_from_page_comments();
