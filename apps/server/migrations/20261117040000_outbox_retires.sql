-- K3.22 (lane B): the outbox retires. Every write stages its change notice
-- in the event log (`events::emit`); open browsers, notifications and
-- webhooks read the log. Nothing reads or writes `outbox_events` any more.
DROP TABLE outbox_events;
