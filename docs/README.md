# Documentation

## Running a server

- [`self-host/`](self-host/README.md): install, TLS, configuration,
  backups, upgrades, moving a workspace, small machines, a static web app.
- [`hosting/shapes.md`](hosting/shapes.md): one box, small hosted and edge,
  and what each costs at idle.
- [`operations/`](operations/): backups, retention, abuse controls,
  performance, and the threat model for outside agents.
- [`self-hosted-red-hat.md`](self-hosted-red-hat.md): the runbook for the
  project's own private host (an example of a hardened single host).

## Connecting agents and services

- [`harness-guide.md`](harness-guide.md): the four ways to plug a harness
  (the program that does an agent's work) in.
- [`agent-runner.md`](agent-runner.md): the protocol a runner speaks.
- [`hosts/writing-a-host.md`](hosts/writing-a-host.md): writing a host that
  runs agents for a workspace.
- [`public-api.md`](public-api.md) and [`api/`](api/): the REST API,
  webhooks and automation rules, and the OpenAPI document.
- [`contracts/live.md`](contracts/live.md): the live (realtime) contract.
- [`compatibility/`](compatibility/): notes on MCP clients, GitHub, git
  worktrees and the harnesses tested against.
- [`local-codex-mcp.md`](local-codex-mcp.md) and
  [`workos-agent-auth.md`](workos-agent-auth.md): two specific setups.

## Building it

- [`../AGENTS.md`](../AGENTS.md) and [`../CONTRIBUTING.md`](../CONTRIBUTING.md):
  how changes are made, and the rules every change keeps.
- [`plans/master/`](plans/master/README.md): where the product is going and
  the plans behind it; [`plans/`](plans/) also holds how existing pieces
  were built.
- [`frontend/`](frontend/): the web app's module boundaries, themes and
  tokens, and the polish checklist every new page passes.
- [`history/`](history/): first-build documents, kept for the record and
  not to be followed.
