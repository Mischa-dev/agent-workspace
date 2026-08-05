# Hermes Agent compatibility

The first M9 connector targets Hermes Agent `0.20.0` from release
`v2026.8.3`, commit `3c27eb6234bf91b8ceee9e9071591b31e9b148cb`.
Compatibility is proved from an isolated Red Hat installation and profile; the
existing default Hermes `0.18.2` install and its state are not modified.

Official Hermes configuration supports remote HTTP MCP servers with `url` and
`headers`, profile-secret expansion through `${VAR}` or `${env:VAR}`, explicit
tool include/exclude filters, `supports_parallel_tool_calls`, and independent
resource, prompt, sampling, and elicitation controls. Registered tool names use
`mcp__<server>__<tool>`.

The M9 profile uses this contract:

```yaml
terminal:
  home_mode: profile

mcp_servers:
  agent_workspace:
    url: "http://127.0.0.1:13101/api/v1/mcp"
    headers:
      Authorization: "Bearer ${AGENT_WORKSPACE_MCP_TOKEN}"
    enabled: true
    timeout: 120
    connect_timeout: 30
    supports_parallel_tool_calls: false
    tools:
      include: [] # replaced by the exact M9 allowlist during implementation
      resources: false
      prompts: false
    sampling:
      enabled: false
    elicitation:
      enabled: false
```

Port 13101 is an operator-selected client-side SSH-forward port in the isolated
Hermes profile, not a new server listener. The server remains on its existing
loopback application boundary. The final compatibility record must add the
exact Python/MCP dependency lock, negotiated protocol, discovered tool list,
request/response fixtures, auth/rotation/revocation results, cursor behavior,
failure categories, and real end-to-end evidence.

Primary references:

- <https://github.com/NousResearch/hermes-agent/releases/tag/v2026.8.3>
- <https://github.com/NousResearch/hermes-agent/blob/main/website/docs/reference/mcp-config-reference.md>
- <https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/configuration.md>
