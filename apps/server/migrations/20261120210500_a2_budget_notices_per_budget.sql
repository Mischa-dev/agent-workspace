-- A2.14 follow-up: a budget's notice is kept per budget. Two budgets on
-- the same meter and account (say a soft one and a hard one) each tell
-- whoever set them, so a notice can name its `spend_limits` row as its
-- scope ('budget').
ALTER TABLE limit_notices DROP CONSTRAINT limit_notices_scope_kind_check;
ALTER TABLE limit_notices ADD CONSTRAINT limit_notices_scope_kind_check
    CHECK (scope_kind IN ('workspace', 'account', 'space', 'conversation', 'budget'));
