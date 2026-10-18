-- Bug sweep: an agent's saved items and reminders remember the connection it
-- saved them through. Access checks for agents go through their connection's
-- rules (actor_can_access_project with a connection); without one, every
-- check for an agent failed, so agents could save nothing and their
-- reminders never fired.

ALTER TABLE saved_items
    ADD COLUMN external_agent_connection_id UUID
        REFERENCES external_agent_connections(id) ON DELETE CASCADE;
