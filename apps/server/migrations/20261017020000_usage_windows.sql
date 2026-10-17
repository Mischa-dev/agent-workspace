-- U1, U3, U6: usage windows, capacity alert rules and banked resets.
--
-- Each provider account has whichever windows its harness reports (a
-- five-hour session, a week, a month of credits), read in the background by
-- the usage.poll job without taking a run slot, and refreshed from run
-- events. Accounts whose harness reports nothing get windows from schedules
-- people set ("resets Mondays at 09:00"); those are computed on read and
-- not stored here. Everything belongs to the account's workspace.

CREATE TABLE usage_windows (
    account_id UUID NOT NULL REFERENCES provider_accounts(id) ON DELETE CASCADE,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    window_key TEXT NOT NULL CHECK (
        char_length(window_key) BETWEEN 1 AND 64 AND window_key ~ '^[a-z0-9][a-z0-9._-]*$'
    ),
    limit_name TEXT CHECK (
        limit_name IS NULL OR (char_length(limit_name) BETWEEN 1 AND 80 AND limit_name !~ '[[:cntrl:]]')
    ),
    duration_minutes INTEGER CHECK (duration_minutes IS NULL OR duration_minutes BETWEEN 1 AND 525600),
    used_percent SMALLINT NOT NULL CHECK (used_percent BETWEEN 0 AND 100),
    resets_at TIMESTAMPTZ,
    limit_reached BOOLEAN NOT NULL DEFAULT false,
    -- When the harness last reported this window.
    read_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, window_key)
);

CREATE INDEX usage_windows_resets ON usage_windows (resets_at) WHERE resets_at IS NOT NULL;

-- Per account: when it was last read, what failed, and its banked resets.
CREATE TABLE usage_accounts (
    account_id UUID PRIMARY KEY REFERENCES provider_accounts(id) ON DELETE CASCADE,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    plan TEXT CHECK (plan IS NULL OR (char_length(plan) BETWEEN 1 AND 40 AND plan !~ '[[:cntrl:]]')),
    polled_at TIMESTAMPTZ,
    -- The last read that failed, and a category (never provider text).
    poll_failed_at TIMESTAMPTZ,
    poll_failure TEXT CHECK (poll_failure IS NULL OR poll_failure ~ '^[a-z][a-z0-9_]{0,63}$'),
    -- NULL: the harness has no banked resets.
    reset_credits_available INTEGER CHECK (
        reset_credits_available IS NULL OR reset_credits_available BETWEEN 0 AND 10000
    ),
    -- [{id, title, expiresAt, grantedAt}], neutral shape, bounded.
    reset_credits JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(reset_credits) = 'array' AND octet_length(reset_credits::text) <= 16384
    ),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Reset schedules people set for accounts whose harness reports nothing.
CREATE TABLE usage_schedules (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES provider_accounts(id) ON DELETE CASCADE,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    label TEXT NOT NULL CHECK (char_length(label) BETWEEN 1 AND 60 AND label !~ '[[:cntrl:]]'),
    cadence TEXT NOT NULL CHECK (cadence IN ('daily', 'weekly', 'monthly')),
    -- 1 Monday .. 7 Sunday, for weekly.
    weekday SMALLINT CHECK (weekday IS NULL OR weekday BETWEEN 1 AND 7),
    -- 1..28, for monthly, so every month has the day.
    month_day SMALLINT CHECK (month_day IS NULL OR month_day BETWEEN 1 AND 28),
    minute_of_day SMALLINT NOT NULL CHECK (minute_of_day BETWEEN 0 AND 1439),
    time_zone TEXT NOT NULL CHECK (char_length(time_zone) BETWEEN 1 AND 64),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((cadence = 'weekly') = (weekday IS NOT NULL)),
    CHECK ((cadence = 'monthly') = (month_day IS NOT NULL))
);

CREATE INDEX usage_schedules_account ON usage_schedules (account_id, created_at, id);

-- Capacity alert rules (U3). Every rule is on by default with built-in
-- thresholds; a row here is the owner's change to one.
CREATE TABLE capacity_alert_rules (
    account_id UUID NOT NULL REFERENCES provider_accounts(id) ON DELETE CASCADE,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    rule TEXT NOT NULL CHECK (
        rule IN ('reset_soon', 'high_usage', 'limit_reached', 'reset_credits')
    ),
    enabled BOOLEAN NOT NULL,
    hours SMALLINT CHECK (hours IS NULL OR hours BETWEEN 1 AND 168),
    percent SMALLINT CHECK (percent IS NULL OR percent BETWEEN 1 AND 100),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, rule)
);

-- Which alerts already fired, so each fires once per window cycle (or once
-- per banked reset). cycle_at is the reset time the alert was about.
CREATE TABLE capacity_alert_fires (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES provider_accounts(id) ON DELETE CASCADE,
    rule TEXT NOT NULL CHECK (char_length(rule) BETWEEN 1 AND 40),
    subject_key TEXT NOT NULL CHECK (char_length(subject_key) BETWEEN 1 AND 240),
    cycle_at TIMESTAMPTZ,
    fired_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX capacity_alert_fires_lookup
    ON capacity_alert_fires (account_id, rule, subject_key, cycle_at);
