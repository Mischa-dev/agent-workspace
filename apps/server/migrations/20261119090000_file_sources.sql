-- M1.18: a file source's conformance check, kept on its install. NULL until
-- the suite has run; then {checkedAt, problems: [{path, message}]}. A source
-- with problems is refused until it passes again.
ALTER TABLE module_apps ADD COLUMN file_check JSONB
    CHECK (file_check IS NULL OR (jsonb_typeof(file_check) = 'object'
                                  AND octet_length(file_check::text) <= 16384));
