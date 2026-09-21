import { seed, execute, transact } from "../apps/server/src/domain/commands.js";
import { pool, setSetting } from "../apps/server/src/db.js";
import { randomUUID } from "node:crypto";
await seed();
await setSetting("dispatchEnabled", false);
const owner = { id: "owner", owner: true };
const command = async (type: string, input: any) =>
  (await execute(owner, { id: randomUUID(), type, input })).value;
const agents = (await transact((s) => s.all("agent"))).value;
const poll = await command("agent.create", {
  name: "Research partner",
  role: "An external collaborator that checks in periodically.",
  deliveryMode: "poll",
  capabilities: ["research"],
});
const memberIds = [...agents.map((a) => a.id), poll.id];
const space = await command("space.create", {
  name: "Launch studio",
  description: "Decisions, working notes, and files for the shared workspace.",
  agentIds: memberIds,
});
const project = await command("project.create", {
  name: "Connected release",
  description: "A usable workspace for agents and their owner.",
  spaceId: space.id,
  agentIds: memberIds,
});
const resource = await command("resource.create", {
  title: "Launch decisions",
  type: "note",
  text: "# Launch decisions\n\n- One workspace for conversations, tasks, and artifacts.\n- Connections can poll or receive a wakeup.\n- The owner reviews completed work.",
  spaceIds: [space.id],
  projectIds: [project.id],
  agentIds: memberIds,
  pinned: true,
  tags: ["decision", "launch"],
});
const channel = await command("conversation.create", {
  title: "release-room",
  kind: "channel",
  spaceId: space.id,
  projectId: project.id,
  agentIds: memberIds,
});
await command("resource.grant", {
  id: resource.resource.id,
  expectedRevision: resource.resource.revision,
  conversationIds: [channel.id],
  agentIds: memberIds,
  projectIds: [project.id],
  spaceIds: [space.id],
});
await command("message.send", {
  conversationId: channel.id,
  body: "Please review the launch decisions and suggest the next step.",
  recipientIds: [poll.id],
  attachments: [
    { resourceId: resource.resource.id, revisionId: resource.revision.id },
  ],
});
const task1 = await command("task.create", {
  title: "Agree on the release scope",
  description: "Document what ships and what waits.",
  criteria: ["A clear launch boundary"],
  projectId: project.id,
  assigneeId: agents[0].id,
});
await command("task.update", {
  id: task1.id,
  expectedRevision: task1.revision,
  status: "ready",
});
await command("task.create", {
  title: "Review the onboarding flow",
  description: "Try connecting an agent and describe confusing steps.",
  criteria: ["A concise review with concrete suggestions"],
  projectId: project.id,
  assigneeId: poll.id,
  dependencies: [task1.id],
});
await command("task.create", {
  title: "Prepare a deployment checklist",
  description: "Capture persistent storage, TLS, health, and recovery checks.",
  criteria: ["A repeatable checklist"],
  projectId: project.id,
  parentId: task1.id,
});
await command("credential.issue", {
  agentId: poll.id,
  label: "Browser review connection",
});
console.log(
  JSON.stringify({
    spaceId: space.id,
    projectId: project.id,
    channelId: channel.id,
    agents: memberIds.length,
    dispatch: false,
  }),
);
await pool.end();
