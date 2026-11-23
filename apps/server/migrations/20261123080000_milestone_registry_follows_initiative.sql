-- A milestone's place in the registry is under its initiative. The registry
-- trigger didn't watch `initiative_id`, so a change to that column alone (an
-- import fills it in after the records are in) left the milestone without a
-- parent there.
DROP TRIGGER things_sync ON project_milestones;
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    project_id, name, version, updated_at, archived_at, initiative_id
    ON project_milestones FOR EACH ROW
    EXECUTE FUNCTION things_sync('thing_source_project_milestones', '{milestone}');

-- Milestones under an initiative take their place again, through the trigger.
UPDATE project_milestones SET initiative_id = initiative_id WHERE initiative_id IS NOT NULL;
