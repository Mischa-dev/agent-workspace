---
id: codex
title: Codex
order: 20
---

1. Save the token where only you can read it, for example as
   `AGENT_WORKSPACE_MCP_TOKEN` in the environment Codex starts in:

```bash
export AGENT_WORKSPACE_MCP_TOKEN='{token}'
```

2. Add this to `~/.codex/config.toml`, then restart Codex:

```toml
[mcp_servers.agent_workspace]
url = "{mcp}"
bearer_token_env_var = "AGENT_WORKSPACE_MCP_TOKEN"
```

To add the skills too, download `{origin}/agent-plugin.zip`, unzip it and
copy its `skills` folder where Codex reads skills from.
