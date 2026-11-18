-- C2.21: a record made from a message keeps which message, like a decision
-- does, and K3's link store mirrors it as record -made_from-> message. The
-- timeline shows it under the message for readers who can open the record.

ALTER TABLE collection_records ADD COLUMN source_message_id UUID;
ALTER TABLE collection_records
    ADD CONSTRAINT collection_records_source_message
    FOREIGN KEY (project_id, source_message_id) REFERENCES messages(project_id, id);
CREATE INDEX collection_records_source_message ON collection_records (source_message_id)
    WHERE source_message_id IS NOT NULL;

CREATE FUNCTION link_of(r collection_records) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.id, r.source_message_id, 'made_from', 'derived',
               NULL, NULL, r.created_by_actor_id)::link_of_row
$$;

-- Named to sort after `things_sync`, so the record's thing is there first.
-- The column is set once, as the record is made, and never cleared.
CREATE TRIGGER things_sync_links_made_from
    AFTER INSERT OR UPDATE OF source_message_id ON collection_records
    FOR EACH ROW WHEN (NEW.source_message_id IS NOT NULL)
    EXECUTE FUNCTION links_mirror();
