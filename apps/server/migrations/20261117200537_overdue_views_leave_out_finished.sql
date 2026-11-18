-- A finished task is never overdue. The old task list's `overdue=true`
-- left out completed and canceled tasks, and so does its reading now
-- (`task_query::from_params`), but the views `20261117100000_personal_views`
-- carried over took only the date. A Tasks view that asks for overdue tasks
-- and doesn't already leave out finished ones gets that filter. At the
-- deploy both run together, so every such view came from that conversion.
UPDATE collection_views view
SET config = jsonb_set(
        view.config, '{filters}',
        view.config -> 'filters' || jsonb_build_array(jsonb_build_object(
            'field', '0192f5c0-0000-7000-8000-000000000001',
            'op', 'categoryIsNot',
            'values', jsonb_build_array('completed', 'canceled')))),
    version = view.version + 1,
    updated_at = now()
FROM collections collection
WHERE collection.id = view.collection_id
  AND collection.builtin = 'tasks'
  AND jsonb_typeof(view.config -> 'filters') = 'array'
  AND view.config -> 'filters' @> jsonb_build_array(jsonb_build_object(
        'field', '0192f5c0-0000-7000-8000-000000000005',
        'op', 'within',
        'values', jsonb_build_array('overdue')))
  AND NOT view.config -> 'filters' @> jsonb_build_array(jsonb_build_object(
        'field', '0192f5c0-0000-7000-8000-000000000001',
        'op', 'categoryIsNot',
        'values', jsonb_build_array('completed', 'canceled')));
