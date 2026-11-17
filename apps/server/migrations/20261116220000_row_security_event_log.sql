-- K3/K5 (build 1E) after H1.21 (build 3R): row security on the tenant tables
-- this build added, as 3R's `tenancy_protect` does for every other tenant
-- table. Their own migrations ran before the function existed. Partitions
-- the `events.partitions` job adds later are protected as it makes them.
SELECT tenancy_protect('event_staging', 'workspace_id');
SELECT tenancy_protect('events', 'workspace_id');
SELECT tenancy_protect('events_default', 'workspace_id');
SELECT tenancy_protect('events_p0', 'workspace_id');
SELECT tenancy_protect('links', 'workspace_id');
SELECT tenancy_protect('things', 'workspace_id');
