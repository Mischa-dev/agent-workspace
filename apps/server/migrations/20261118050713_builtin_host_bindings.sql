-- A1.16: the built-in host reports on its runs through the host protocol,
-- with the run credential its start carries, as any host does. That
-- credential works for an agent bound to the host, so every built-in agent
-- (an execution worker's account) is bound to its workspace's built-in host
-- row, now and whenever one is made.
CREATE FUNCTION builtin_host_bind_worker() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO host_agents (host_id, account_id, host_agent_ref, bound_by_account_id)
    SELECT host.id, NEW.actor_id, NEW.actor_id::text, host.account_id
      FROM projects project
      JOIN hosts host ON host.owner_workspace_id = project.organization_id AND host.builtin
     WHERE project.id = NEW.project_id
    ON CONFLICT DO NOTHING;
    RETURN NEW;
END;
$$;

CREATE TRIGGER execution_workers_builtin_host_binding
    AFTER INSERT ON execution_workers
    FOR EACH ROW EXECUTE FUNCTION builtin_host_bind_worker();

INSERT INTO host_agents (host_id, account_id, host_agent_ref, bound_by_account_id)
SELECT DISTINCT ON (host.id, worker.actor_id)
       host.id, worker.actor_id, worker.actor_id::text, host.account_id
  FROM execution_workers worker
  JOIN projects project ON project.id = worker.project_id
  JOIN hosts host ON host.owner_workspace_id = project.organization_id AND host.builtin
ON CONFLICT DO NOTHING;

-- The start command the built-in host carried out for a run: fetched again,
-- it gives a fresh run credential (after a restart, or once one expires).
ALTER TABLE agent_runs ADD COLUMN start_command_id UUID;
