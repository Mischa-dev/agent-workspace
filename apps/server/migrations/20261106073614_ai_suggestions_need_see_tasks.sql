-- AWD-74: an AI suggestion is about tasks (work_item_id is always set), so
-- seeing one also takes "see tasks" by the viewer's role in each project
-- it touches: the task's, the other task's, and every task it cites.
-- Before, reaching the project was enough, so someone whose role dropped
-- "see tasks" could still list suggestions, or dismiss one they knew of
-- and read the task's title, status, value and reason back. Every read
-- and every decision goes through this function, as before. An app's own
-- limit is applied in the server around the same reads.

CREATE OR REPLACE FUNCTION ai_suggestion_visible_to(target UUID, viewer UUID, connection UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM ai_suggestions suggestion
        WHERE suggestion.id = target
          AND (suggestion.audience_actor_id IS NULL OR suggestion.audience_actor_id = viewer)
          AND actor_can_access_project(viewer, connection, suggestion.project_id)
          AND EXISTS (
              SELECT 1 FROM project_memberships sees_tasks
              WHERE sees_tasks.project_id = suggestion.project_id
                AND sees_tasks.actor_id = viewer
                AND sees_tasks.enabled
                AND custom_role_allows(sees_tasks.role_id, 'tasks.view')
          )
          AND (suggestion.other_project_id IS NULL
               OR (actor_can_access_project(viewer, connection, suggestion.other_project_id)
                   AND EXISTS (
                       SELECT 1 FROM project_memberships sees_other
                       WHERE sees_other.project_id = suggestion.other_project_id
                         AND sees_other.actor_id = viewer
                         AND sees_other.enabled
                         AND custom_role_allows(sees_other.role_id, 'tasks.view')
                   )))
          AND NOT EXISTS (
              SELECT 1
              FROM jsonb_array_elements(suggestion.sources) source
              WHERE (source->>'kind' = 'task' AND NOT EXISTS (
                        SELECT 1 FROM work_items item
                        JOIN project_memberships sees_source
                          ON sees_source.project_id = item.project_id
                         AND sees_source.actor_id = viewer
                         AND sees_source.enabled
                         AND custom_role_allows(sees_source.role_id, 'tasks.view')
                        WHERE item.id = (source->>'id')::uuid
                          AND actor_can_access_project(viewer, connection, item.project_id)))
                 OR (source->>'kind' = 'message' AND NOT EXISTS (
                        SELECT 1
                        FROM messages message
                        JOIN conversations conversation
                          ON conversation.id = message.conversation_id
                        WHERE message.id = (source->>'messageId')::uuid
                          AND message.removed_at IS NULL
                          AND actor_can_access_project(viewer, connection, conversation.project_id)
                          AND conversation_visible_to(conversation.id, viewer)))
                 OR (source->>'kind' NOT IN ('task', 'message', 'signal'))
          )
    )
$$;
