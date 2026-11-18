-- A2.17: the clients a setup link is for are the agent plugin's recipes,
-- which are data; the server checks the name against them.
ALTER TABLE agent_setup_links DROP CONSTRAINT IF EXISTS agent_setup_links_app_check;
ALTER TABLE agent_setup_links
    ADD CONSTRAINT agent_setup_links_app_shape CHECK (app ~ '^[a-z][a-z0-9_]{0,39}$');
