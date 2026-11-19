-- A2.9: following and ambient mode.
--
-- `follows`: an account follows a thing (a task, page, sheet, record,
-- whiteboard or conversation) or a whole space. 'changes' hears the
-- thing's own events; 'everything' adds its discussion (comments, and the
-- messages of a followed conversation or space). It replaces
-- `work_item_watchers`: watching a task is following it with
-- 'everything', and every watcher is carried over once here.
--
-- `conversation_agent_settings`: per account and conversation, whether it
-- is in ambient mode there (invited, with an optional reply request, to
-- speak on each new message that nobody prompted) and how long it waits
-- after answering before it is invited again.

CREATE TABLE follows (
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    -- NULL for a workspace whiteboard, which lives in no space.
    space_id UUID REFERENCES projects(id),
    account_id UUID NOT NULL REFERENCES actors(id),
    target_kind TEXT NOT NULL CHECK (target_kind IN (
        'space', 'task', 'page', 'sheet', 'record', 'whiteboard', 'conversation'
    )),
    target_id UUID NOT NULL,
    level TEXT NOT NULL DEFAULT 'changes' CHECK (level IN ('changes', 'everything')),
    created_by_account_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, target_kind, target_id)
);
CREATE INDEX follows_target ON follows (target_kind, target_id);
CREATE INDEX follows_space ON follows (space_id, account_id);
SELECT tenancy_protect('follows', 'workspace_id');

INSERT INTO follows (
    workspace_id, space_id, account_id, target_kind, target_id, level,
    created_by_account_id, created_at
)
SELECT watcher.workspace_id, watcher.project_id, watcher.actor_id, 'task',
       watcher.work_item_id, 'everything', watcher.actor_id, watcher.created_at
FROM work_item_watchers watcher
ON CONFLICT DO NOTHING;

DROP TABLE work_item_watchers;

CREATE TABLE conversation_agent_settings (
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID NOT NULL REFERENCES projects(id),
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id),
    ambient BOOLEAN NOT NULL DEFAULT false,
    -- NULL: the default (300 seconds).
    reply_cooldown_seconds INTEGER CHECK (reply_cooldown_seconds BETWEEN 0 AND 86400),
    set_by_account_id UUID NOT NULL REFERENCES actors(id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (conversation_id, account_id)
);
CREATE INDEX conversation_agent_settings_ambient
    ON conversation_agent_settings (conversation_id) WHERE ambient;
SELECT tenancy_protect('conversation_agent_settings', 'workspace_id', 'conversation_id');
