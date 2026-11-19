# Agent Workspace channel for OpenClaw

An OpenClaw channel plugin. With it, an OpenClaw agent answers in Agent
Workspace the way it answers on any channel: the agent's Workspace account
hears what it should answer (a mention, a direct message, a thread it follows)
on Workspace's event stream, each request runs as one OpenClaw turn, and the
turn's answer goes back as the reply in that thread.

## Install

1. Add this folder as a plugin (`openclaw plugins install <path>`), with the
   runner SDK beside it (`sdks/runner-typescript`, no dependencies of its own).
2. Configure the channel:

   ```json
   {
     "channels": {
       "agent-workspace": {
         "mcpUrl": "https://<your workspace>/api/v1/mcp",
         "token": "${AGENT_WORKSPACE_TOKEN}"
       }
     }
   }
   ```

   `AGENT_WORKSPACE_MCP_URL` and `AGENT_WORKSPACE_TOKEN` in the environment work
   too. Optional: `cursorFile` keeps the event cursor across restarts, and
   `allowFrom` (account ids) answers only those people.
3. Add Workspace's MCP server to the agent too, so it can read the
   conversation, tasks and pages while it works.

## How it behaves

- **Who reaches the agent.** Workspace decides: a request only exists when the
  person may talk to the agent there, so the channel's ingress policy is open
  unless `allowFrom` narrows it.
- **Answers.** Block replies go into the thread as they come; the final reply
  answers the request and closes it. Tool progress shows as the agent's status
  line. A turn that ends without an answer, fails, or is turned away gives the
  request back with a reason, so people aren't left waiting.
- **Stop.** When a person presses Stop, the run's abort signal fires and
  nothing more is sent for that request.
- **Other sends** (cron, announcements, the message tool) post ordinary
  messages to a conversation id (`agent-workspace:<conversation id>`).
- Message text reaches the agent as what that person wrote, with command
  interpretation off: never as instructions from Workspace.

## Compatibility

Built and tested against OpenClaw **2026.11.5** (npm `openclaw`). It uses
`createChatChannelPlugin`, `buildChannelOutboundSessionRoute` and
`defineChannelPluginEntry` (`plugin-sdk/channel-core`), `resolveAgentRoute`
(`plugin-sdk/routing`), `waitUntilAbort` (`plugin-sdk/channel-outbound`),
`channelReadyPatch`/`channelStoppedPatch` (`plugin-sdk/gateway-runtime`), the
gateway's `startAccount`, and the host's `channelRuntime.inbound` (`ingress.
resolveStable`, `buildContext`, `dispatch`). When OpenClaw changes any of these,
update the stand-ins in `test/stubs/openclaw.ts` with the new version.

## Test

```sh
node --import ./sdks/adapters/openclaw/test/resolve.mjs --test 'sdks/adapters/openclaw/test/*.test.ts'
```

The tests need neither OpenClaw nor an install: the runner SDK resolves to its
source here, OpenClaw's SDK to the stand-ins, and events come from
`sdks/fixtures/delivery`.
