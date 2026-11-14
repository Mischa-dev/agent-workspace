-- P0.36: page comment and page suggestion notifications held a copy of the
-- comment's first 140 characters (`preview`) and the page's title at the time
-- (`title`), against this table's own rule that rows hold ids, not text. New
-- rows hold ids only and the inbox reads both live; this strips the copies
-- from the rows already written. One statement, idempotent, safe to rerun.
UPDATE notifications
SET detail = detail - 'preview' - 'title'
WHERE kind IN ('page_comment', 'page_suggestion')
  AND (detail ? 'preview' OR detail ? 'title');
