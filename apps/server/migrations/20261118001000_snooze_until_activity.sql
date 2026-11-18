-- C2.10: snooze until something new happens. A row snoozed this way comes
-- back unread, at the top, the next time its group gets an occurrence
-- (`deliver` clears the snooze), or after a year if nothing happens.
ALTER TABLE notifications
    ADD COLUMN wake_on_activity BOOLEAN NOT NULL DEFAULT false;
