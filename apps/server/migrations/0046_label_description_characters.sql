-- Label descriptions refuse the same characters on every database
-- (docs/plans/mcp-tools.md, Phase 2).
--
-- 0044 checked descriptions with [[:cntrl:]], whose meaning follows the
-- database's locale: some locales count the line and paragraph separators
-- (U+2028, U+2029) as control characters, the C locale only ASCII ones. The
-- server (work_item_labels::normalize_label_description) refuses control
-- characters (U+0001 to U+001F and U+007F to U+009F) and those two
-- separators, so a description stays on one line. The CHECK now names the
-- same code points, so the server and the database agree whatever the
-- locale, and the database never refuses a description the server let
-- through.
--
-- A database whose locale let a separator in keeps that description, with
-- the separator turned into a space.

UPDATE project_labels
SET description = regexp_replace(description, '[  ]', ' ', 'g')
WHERE description ~ '[  ]';

ALTER TABLE project_labels
    DROP CONSTRAINT project_labels_description_check,
    ADD CONSTRAINT project_labels_description_check CHECK (
        char_length(description) <= 200
        AND description !~ '[\u0001-\u001f\u007f-\u009f  ]'
    );
