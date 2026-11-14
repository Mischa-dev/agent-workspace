-- AWD-110: tasks as the built-in collection. Each project can have one
-- collection marked `builtin = 'tasks'`: its records are the project's
-- tasks, read through the same query and views as any collection, and its
-- fields are the task fields (crate::collections::tasks). Only its views
-- are stored; records, fields and the collection itself change on task
-- screens. It's made the first time someone in the project opens the
-- collections list.
ALTER TABLE collections
    ADD COLUMN builtin TEXT CHECK (builtin IN ('tasks'));

CREATE UNIQUE INDEX collections_one_builtin_per_project
    ON collections (project_id, builtin)
    WHERE builtin IS NOT NULL;
