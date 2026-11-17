-- A2.11 (Lane E): follow-ups an account sets for itself.
--
-- An account (an agent, usually) asks to be told again later, once or every
-- so many minutes, optionally about one thing it can see. When one is due the
-- account hears `schedule.due`, addressed to it, carrying the schedule's and
-- the thing's ids; the note stays here. Each due time is one job on the queue
-- (keyed by the schedule and the time, so two workers fire it once); nothing
-- runs per workspace. `space_id` is the space of the thing it is about, if any.

CREATE TABLE agent_schedules (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID REFERENCES projects(id),
    account_id UUID NOT NULL REFERENCES actors(id),
    thing_id UUID,
    thing_kind TEXT CHECK (char_length(thing_kind) <= 40),
    note TEXT NOT NULL DEFAULT '' CHECK (char_length(note) <= 500),
    every_minutes INTEGER CHECK (every_minutes BETWEEN 5 AND 525600),
    next_due_at TIMESTAMPTZ,
    last_due_at TIMESTAMPTZ,
    times_due INTEGER NOT NULL DEFAULT 0 CHECK (times_due >= 0),
    state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'done', 'cancelled')),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((state = 'active') = (next_due_at IS NOT NULL)),
    CHECK ((thing_id IS NULL) = (thing_kind IS NULL))
);
CREATE INDEX agent_schedules_account
    ON agent_schedules (account_id, workspace_id, created_at) WHERE state = 'active';
CREATE INDEX agent_schedules_workspace ON agent_schedules (workspace_id);
SELECT tenancy_protect('agent_schedules', 'workspace_id');

-- A due follow-up is still worth hearing until it is cancelled.
CREATE OR REPLACE FUNCTION delivery_still_relevant(recipient UUID, ev events)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT CASE
        WHEN ev.type IN ('reply.requested', 'reply.followup') THEN EXISTS (
            SELECT 1 FROM reply_requests r
            WHERE r.id = ev.thing_id AND r.agent_actor_id = recipient AND r.status = 'pending')
        WHEN ev.type = 'reply.cancelled' THEN EXISTS (
            SELECT 1 FROM reply_requests r
            WHERE r.id = ev.thing_id AND r.agent_actor_id = recipient AND r.status = 'cancelled')
        WHEN ev.type = 'schedule.due' AND ev.thing_kind::text = 'schedule' THEN EXISTS (
            SELECT 1 FROM agent_schedules s
            WHERE s.id = ev.thing_id AND s.account_id = recipient AND s.state <> 'cancelled')
        ELSE true
    END
$$;
