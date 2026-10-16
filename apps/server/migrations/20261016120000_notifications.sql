-- Notifications for people (roadmap N1) and the inbox they fill (N2).
--
-- One row per recipient and subject group: repeats (three mentions in the
-- same channel, a run that fails twice) collapse into one row whose
-- repeat_count grows and which turns unread again. Rows hold IDs only. Text
-- shown in the inbox (message previews, names, titles) is read live through
-- joins when listing, so an edited or removed message, a renamed channel or a
-- lost membership is reflected at once and no copy of message text is kept.
--
-- Every row belongs to a project, so it is workspace scoped through it.
-- Readers must still check conversation_visible_to (conversation_id) and
-- pull_request_visible_to (pull_request_id) for the viewer, plus
-- actor_can_access_project: access can be lost after a row was written.

CREATE TABLE notifications (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    recipient_actor_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    -- Open-ended so later workstreams (capacity alerts, AI suggestions) add
    -- their own kinds without a migration.
    kind TEXT NOT NULL CHECK (
        char_length(kind) BETWEEN 1 AND 64 AND kind ~ '^[a-z][a-z0-9_]*$'
    ),
    subject_kind TEXT NOT NULL CHECK (
        char_length(subject_kind) BETWEEN 1 AND 64
        AND subject_kind ~ '^[a-z][a-z0-9_]*$'
    ),
    subject_id UUID NOT NULL,
    -- Who caused the latest occurrence, when someone did.
    actor_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    conversation_id UUID REFERENCES conversations(id) ON DELETE CASCADE,
    message_id UUID REFERENCES messages(id) ON DELETE SET NULL,
    work_item_id UUID REFERENCES work_items(id) ON DELETE CASCADE,
    comment_id UUID REFERENCES work_item_comments(id) ON DELETE SET NULL,
    agent_run_id UUID REFERENCES agent_runs(id) ON DELETE CASCADE,
    approval_id UUID REFERENCES agent_approvals(id) ON DELETE SET NULL,
    pull_request_id UUID REFERENCES github_pull_requests(id) ON DELETE CASCADE,
    -- Bounded structured detail for kinds that need more than the IDs above.
    -- Never message text.
    detail JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(detail) = 'object' AND octet_length(detail::text) <= 4096
    ),
    group_key TEXT NOT NULL CHECK (char_length(group_key) BETWEEN 1 AND 200),
    repeat_count INTEGER NOT NULL DEFAULT 1 CHECK (repeat_count >= 1),
    read_at TIMESTAMPTZ,
    archived_at TIMESTAMPTZ,
    snoozed_until TIMESTAMPTZ,
    -- When the row last came to the top of the inbox: its newest occurrence,
    -- or the end of its snooze.
    surfaced_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Repeats collapse into the one open (unarchived) row of their group.
CREATE UNIQUE INDEX notifications_open_group
    ON notifications (recipient_actor_id, group_key)
    WHERE archived_at IS NULL;

CREATE INDEX notifications_inbox
    ON notifications (recipient_actor_id, surfaced_at DESC, id DESC);

CREATE INDEX notifications_unread
    ON notifications (recipient_actor_id, project_id)
    WHERE read_at IS NULL AND archived_at IS NULL;

CREATE INDEX notifications_message ON notifications (message_id)
    WHERE message_id IS NOT NULL;

-- Each source occurrence (one mention, one assignment, one failed run) is
-- applied once per recipient, so the at-least-once outbox can redeliver.
CREATE TABLE notification_occurrences (
    recipient_actor_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    dedupe_key TEXT NOT NULL CHECK (char_length(dedupe_key) BETWEEN 1 AND 300),
    notification_id UUID NOT NULL REFERENCES notifications(id) ON DELETE CASCADE,
    message_id UUID REFERENCES messages(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (recipient_actor_id, dedupe_key)
);

CREATE INDEX notification_occurrences_notification
    ON notification_occurrences (notification_id, created_at DESC);
CREATE INDEX notification_occurrences_message
    ON notification_occurrences (message_id)
    WHERE message_id IS NOT NULL;

-- New and changed notifications reach the recipient's open browsers.
ALTER TABLE outbox_events
    DROP CONSTRAINT outbox_events_resource_kind_check,
    ADD CONSTRAINT outbox_events_resource_kind_check CHECK (
        resource_kind IN (
            'project', 'conversation', 'message', 'read_state', 'agent_run',
            'work_item', 'notification'
        )
    );
