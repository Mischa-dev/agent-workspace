-- Lane D (T1.29): public form limits. A form signs the page it renders
-- with a key only the server holds, so an answer can show the page was
-- loaded a few seconds before; and it counts its answers per UTC day so a
-- form that reaches its daily limit pauses itself.
ALTER TABLE bug_report_links
    ADD COLUMN render_key BYTEA NOT NULL
        DEFAULT (uuid_send(gen_random_uuid()) || uuid_send(gen_random_uuid()))
        CHECK (octet_length(render_key) = 32),
    ADD COLUMN answers_day DATE,
    ADD COLUMN answers_on_day INTEGER NOT NULL DEFAULT 0 CHECK (answers_on_day >= 0);
