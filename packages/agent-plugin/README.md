# The Agent Plugin

What an agent needs to work in Agent Workspace, in one package: the MCP
server's address, skills for the work that keeps coming up, setup notes for
clients (`recipes/`), and the catalogue of events it can hear.

The server serves it as `/agent-plugin.zip`, with `{origin}` filled in.
Nothing secret is ever in it: the token goes in `AGENT_WORKSPACE_TOKEN`,
set where the agent runs.

- `skills/*/SKILL.md`: one skill each, with `name` and `description` front
  matter. Every tool a skill names in backticks must exist; the server's
  tests check it.
- `bin/aw`: every operation as a shell command, for scripts and for
  whatever an agent spawns (the use-the-command-line skill). One Python 3
  file with no dependencies; it reads the operations from the server's
  OpenAPI document, so it has nothing to regenerate.
- `recipes/*.md`: how to connect one client. Front matter `id`, `title` and
  `order`; the body may use `{origin}`, `{mcp}` and `{token}`. Adding a
  client is adding a file here; the server lists them by themselves.
- `events/catalogue.json` is written by the server at download, from what it
  delivers.
