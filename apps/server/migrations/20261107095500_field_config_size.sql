-- AWD-89: room for the largest choice field the app accepts.
--
-- field_definitions.config held at most 16 KB as text (migration
-- 20261105041500). A choice field crate::fields accepts can be larger: 100
-- options (MAX_OPTIONS), each a 36-character ID, a name of 60 characters
-- that may all be four-byte (240 bytes; an escaped quote or backslash is
-- only two), a color of up to 7 characters and the archived flag. As
-- PostgreSQL writes jsonb, with a space after every ":" and ",", that's 336
-- bytes an option and about 34 KB in all. 64 KB covers it with room to
-- spare, as field_values' limit does.
--
-- A new migration rather than an edit, so databases that already ran
-- 20261105041500 keep its checksum.

ALTER TABLE field_definitions DROP CONSTRAINT field_definitions_config_check;

ALTER TABLE field_definitions ADD CONSTRAINT field_definitions_config_check CHECK (
    jsonb_typeof(config) = 'object' AND octet_length(config::text) <= 65536
);
