-- M4 ordered acceptance criteria, immutable comments, and one Work Item timeline.

CREATE TABLE work_item_acceptance_criteria (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    text TEXT NOT NULL CHECK (char_length(text) BETWEEN 1 AND 1000),
    position INTEGER NOT NULL CHECK (position BETWEEN 1 AND 1000),
    completed BOOLEAN NOT NULL DEFAULT false,
    completed_by_actor_id UUID,
    completed_at TIMESTAMPTZ,
    created_by_actor_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, work_item_id, id),
    CONSTRAINT work_item_acceptance_criteria_position_unique
        UNIQUE (project_id, work_item_id, position)
        DEFERRABLE INITIALLY IMMEDIATE,
    FOREIGN KEY (project_id, work_item_id)
        REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, completed_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, created_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    CHECK (
        (NOT completed AND completed_by_actor_id IS NULL AND completed_at IS NULL)
        OR
        (completed AND completed_by_actor_id IS NOT NULL AND completed_at IS NOT NULL)
    )
);

CREATE TABLE work_item_comments (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    author_actor_id UUID NOT NULL,
    content_version INTEGER NOT NULL DEFAULT 1 CHECK (content_version = 1),
    content JSONB NOT NULL CHECK (
        jsonb_typeof(content) = 'object'
        AND content->>'version' = content_version::text
        AND jsonb_typeof(content->'nodes') = 'array'
    ),
    plain_text TEXT NOT NULL CHECK (char_length(plain_text) BETWEEN 1 AND 20000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, work_item_id, id),
    FOREIGN KEY (project_id, work_item_id)
        REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, author_actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE OR REPLACE FUNCTION reject_work_item_comment_mutation()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'Work Item comments are immutable'
        USING ERRCODE = '23514';
END;
$$;

CREATE TRIGGER work_item_comments_reject_mutation
BEFORE UPDATE OR DELETE
ON work_item_comments
FOR EACH ROW
EXECUTE FUNCTION reject_work_item_comment_mutation();

CREATE TABLE work_item_comment_mentions (
    project_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    comment_id UUID NOT NULL,
    actor_id UUID NOT NULL,
    node_index INTEGER NOT NULL CHECK (node_index >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (comment_id, node_index),
    FOREIGN KEY (project_id, work_item_id, comment_id)
        REFERENCES work_item_comments(project_id, work_item_id, id),
    FOREIGN KEY (project_id, actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE TABLE message_work_item_references (
    project_id UUID NOT NULL,
    message_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    node_index INTEGER NOT NULL CHECK (node_index >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (message_id, node_index),
    FOREIGN KEY (project_id, message_id)
        REFERENCES messages(project_id, id),
    FOREIGN KEY (project_id, work_item_id)
        REFERENCES work_items(project_id, id)
);

CREATE TABLE work_item_comment_references (
    project_id UUID NOT NULL,
    source_work_item_id UUID NOT NULL,
    comment_id UUID NOT NULL,
    referenced_work_item_id UUID NOT NULL,
    node_index INTEGER NOT NULL CHECK (node_index >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (comment_id, node_index),
    FOREIGN KEY (project_id, source_work_item_id, comment_id)
        REFERENCES work_item_comments(project_id, work_item_id, id),
    FOREIGN KEY (project_id, referenced_work_item_id)
        REFERENCES work_items(project_id, id)
);

CREATE OR REPLACE FUNCTION validate_work_item_comment_mention()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    node JSONB;
    node_actor_id TEXT;
    mentionable BOOLEAN;
BEGIN
    SELECT comment.content->'nodes'->NEW.node_index
      INTO node
      FROM work_item_comments comment
     WHERE comment.id = NEW.comment_id
       AND comment.project_id = NEW.project_id
       AND comment.work_item_id = NEW.work_item_id
     FOR KEY SHARE;

    node_actor_id := COALESCE(node->>'actor_id', node->>'actorId');
    IF node IS NULL
       OR node->>'type' IS DISTINCT FROM 'mention'
       OR node_actor_id IS DISTINCT FROM NEW.actor_id::text THEN
        RAISE EXCEPTION 'Work Item comment mention must match its AST node'
            USING ERRCODE = '23514';
    END IF;

    SELECT membership.enabled AND actor.status = 'active'
      INTO mentionable
      FROM project_memberships membership
      JOIN actors actor ON actor.id = membership.actor_id
     WHERE membership.project_id = NEW.project_id
       AND membership.actor_id = NEW.actor_id
     FOR KEY SHARE OF membership, actor;
    IF mentionable IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Mentioned Actor must have an enabled Project membership'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER work_item_comment_mentions_validate
BEFORE INSERT OR UPDATE
ON work_item_comment_mentions
FOR EACH ROW
EXECUTE FUNCTION validate_work_item_comment_mention();

CREATE OR REPLACE FUNCTION validate_work_item_reference_projection()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    node JSONB;
    expected_work_item_id UUID;
BEGIN
    IF TG_TABLE_NAME = 'message_work_item_references' THEN
        SELECT message.content->'nodes'->NEW.node_index
          INTO node
          FROM messages message
         WHERE message.id = NEW.message_id AND message.project_id = NEW.project_id
         FOR KEY SHARE;
        expected_work_item_id := NEW.work_item_id;
    ELSE
        SELECT comment.content->'nodes'->NEW.node_index
          INTO node
          FROM work_item_comments comment
         WHERE comment.id = NEW.comment_id
           AND comment.project_id = NEW.project_id
           AND comment.work_item_id = NEW.source_work_item_id
         FOR KEY SHARE;
        expected_work_item_id := NEW.referenced_work_item_id;
    END IF;

    IF node IS NULL
       OR node->>'type' IS DISTINCT FROM 'work_item_ref'
       OR COALESCE(node->>'work_item_id', node->>'workItemId')
          IS DISTINCT FROM expected_work_item_id::text THEN
        RAISE EXCEPTION 'Work Item reference projection must match its AST node'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER message_work_item_references_validate
BEFORE INSERT OR UPDATE
ON message_work_item_references
FOR EACH ROW
EXECUTE FUNCTION validate_work_item_reference_projection();

CREATE TRIGGER work_item_comment_references_validate
BEFORE INSERT OR UPDATE
ON work_item_comment_references
FOR EACH ROW
EXECUTE FUNCTION validate_work_item_reference_projection();

CREATE OR REPLACE FUNCTION reject_work_item_projection_mutation()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'Structured content projections are immutable'
        USING ERRCODE = '23514';
END;
$$;

CREATE TRIGGER work_item_comment_mentions_reject_mutation
BEFORE UPDATE OR DELETE ON work_item_comment_mentions
FOR EACH ROW EXECUTE FUNCTION reject_work_item_projection_mutation();
CREATE TRIGGER message_work_item_references_reject_mutation
BEFORE UPDATE OR DELETE ON message_work_item_references
FOR EACH ROW EXECUTE FUNCTION reject_work_item_projection_mutation();
CREATE TRIGGER work_item_comment_references_reject_mutation
BEFORE UPDATE OR DELETE ON work_item_comment_references
FOR EACH ROW EXECUTE FUNCTION reject_work_item_projection_mutation();

ALTER TABLE work_items
    ADD COLUMN next_timeline_sequence BIGINT NOT NULL DEFAULT 1
        CHECK (next_timeline_sequence > 0);

ALTER TABLE work_item_activity
    DROP CONSTRAINT work_item_activity_kind_check,
    ADD CONSTRAINT work_item_activity_kind_check CHECK (
        kind IN (
            'work_item_created', 'work_item_updated', 'work_item_archived',
            'work_item_restored', 'criterion_created', 'criterion_updated',
            'criterion_completed', 'criterion_reopened', 'criterion_removed',
            'comment_created', 'thread_linked', 'thread_unlinked',
            'repository_linked', 'repository_unlinked', 'repository_primary_changed',
            'worker_invoked'
        )
    ),
    ADD CONSTRAINT work_item_activity_project_item_id_unique
        UNIQUE (project_id, work_item_id, id);

CREATE TABLE work_item_timeline_entries (
    project_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    sequence BIGINT NOT NULL CHECK (sequence > 0),
    kind TEXT NOT NULL CHECK (kind IN ('activity', 'comment')),
    activity_id UUID,
    comment_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, work_item_id, sequence),
    FOREIGN KEY (project_id, work_item_id)
        REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, work_item_id, activity_id)
        REFERENCES work_item_activity(project_id, work_item_id, id),
    FOREIGN KEY (project_id, work_item_id, comment_id)
        REFERENCES work_item_comments(project_id, work_item_id, id),
    CHECK (
        (kind = 'activity' AND activity_id IS NOT NULL AND comment_id IS NULL)
        OR
        (kind = 'comment' AND comment_id IS NOT NULL AND activity_id IS NULL)
    )
);

WITH ordered AS (
    SELECT project_id, work_item_id, id,
           row_number() OVER (
               PARTITION BY project_id, work_item_id ORDER BY created_at, id
           ) AS sequence
      FROM work_item_activity
)
INSERT INTO work_item_timeline_entries (
    project_id, work_item_id, sequence, kind, activity_id, created_at
)
SELECT ordered.project_id, ordered.work_item_id, ordered.sequence,
       'activity', ordered.id, activity.created_at
  FROM ordered
  JOIN work_item_activity activity ON activity.id = ordered.id;

UPDATE work_items item
SET next_timeline_sequence = timeline.next_sequence
FROM (
    SELECT project_id, work_item_id, max(sequence) + 1 AS next_sequence
      FROM work_item_timeline_entries
     GROUP BY project_id, work_item_id
) timeline
WHERE item.project_id = timeline.project_id AND item.id = timeline.work_item_id;
