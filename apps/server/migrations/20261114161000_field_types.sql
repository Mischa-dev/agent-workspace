-- T1.10 (build 1T): the field types a field can be made with, widened once
-- for every stored name the T1 plan names, so later slices (relation,
-- rollup, formula, file, button) need no further change to the check.
--
-- * long_text, email, phone and status store values as the older types do
--   (status: an option ID; its options carry a category in config).
-- * created_time, edited_time, created_by, edited_by and auto_id are system
--   fields: a row only names the column; nothing is stored in field_values.
--
-- Existing rows all hold older names, so they pass as they are.

ALTER TABLE field_definitions
    DROP CONSTRAINT field_definitions_field_type_check,
    ADD CONSTRAINT field_definitions_field_type_check CHECK (
        field_type IN (
            'text', 'number', 'date', 'single_choice', 'multiple_choice',
            'person', 'url', 'checkbox', 'link',
            'long_text', 'email', 'phone', 'status',
            'relation', 'rollup', 'formula', 'file',
            'created_time', 'edited_time', 'created_by', 'edited_by', 'auto_id',
            'button'
        )
    );
