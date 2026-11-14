-- K3.6 (build 1E): sheets can be found by search. A sheet's text is its
-- tabs' names and its text cells' inputs, capped at 20,000 characters, kept
-- on the sheet by sheets::apply (a generated column can't read child rows),
-- with a search vector beside it like the whiteboards' one ('simple', since
-- cells are labels and codes more than prose).

-- What a sheet's text_content is: tab names, then text cells in reading
-- order, capped.
CREATE FUNCTION sheet_text_content(target UUID) RETURNS TEXT
LANGUAGE sql STABLE AS $$
    SELECT left(
        concat_ws(' ',
            (SELECT string_agg(tab.name, ' ' ORDER BY tab.position)
             FROM sheet_tabs tab WHERE tab.sheet_id = target),
            (SELECT string_agg(cell.input, ' ' ORDER BY tab.position, cell.row_number, cell.column_number)
             FROM sheet_cells cell
             JOIN sheet_tabs tab ON tab.id = cell.tab_id
             WHERE cell.sheet_id = target AND cell.kind = 'text')
        ),
        20000)
$$;

ALTER TABLE sheets
    ADD COLUMN text_content TEXT NOT NULL DEFAULT ''
        CHECK (char_length(text_content) <= 20000);
UPDATE sheets SET text_content = sheet_text_content(id);
ALTER TABLE sheets
    ADD COLUMN search_vector TSVECTOR GENERATED ALWAYS AS (
        setweight(to_tsvector('simple', name), 'A')
        || setweight(to_tsvector('simple', description), 'B')
        || setweight(to_tsvector('simple', text_content), 'B')
    ) STORED;
CREATE INDEX sheets_search ON sheets USING GIN (search_vector);
