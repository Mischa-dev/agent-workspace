-- T1.41: records can be proposed, with history.
--
-- Someone whose role only suggests on records proposes a change to one
-- (a row in `proposals`, kind 'record'); someone who edits it accepts it
-- from the proposal's request card, and the change is written as its
-- author's. The history row says who accepted it.
--
-- From now on each change in a record's history also keeps the stored
-- values it replaced and wrote (`was`, `is`), so an earlier version can be
-- shown and put back. Rows from before keep only how values read, and a
-- version that needs them says so.

ALTER TABLE collection_record_history
    ADD COLUMN accepted_by_actor_id UUID REFERENCES actors(id);
