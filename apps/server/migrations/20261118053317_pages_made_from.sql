-- C1.25 (lane D2, documents): a page made from a message, a thread or
-- another page's blocks remembers what it was made from. Like a thread's
-- origin message, the column is mirrored into the link store as a derived
-- `made_from` link, so the page shows "Made from" and its source shows
-- "Made into", each for readers who can see the other end.

ALTER TABLE project_pages ADD COLUMN made_from_id UUID;

CREATE FUNCTION link_of(r project_pages) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.id, r.made_from_id, 'made_from', 'derived',
               NULL, NULL, r.created_by_actor_id)::link_of_row
$$;

-- The name sorts after `things_sync`, so the page's own thing is there first.
CREATE TRIGGER things_sync_links AFTER INSERT OR UPDATE OF made_from_id OR DELETE
    ON project_pages FOR EACH ROW EXECUTE FUNCTION links_mirror();
