-- C2.28 follow-up: clearing a draft (it was sent, or emptied) leaves a dated
-- marker, a row with no content, instead of deleting the row. A slower
-- device's save dated before the clear then loses to it, as it would to a
-- newer draft, and can't bring a sent draft back. Lists skip markers, and the
-- prune job removes them after a day.
ALTER TABLE message_drafts ALTER COLUMN content DROP NOT NULL;
