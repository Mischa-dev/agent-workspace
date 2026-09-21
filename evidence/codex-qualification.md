# Codex connector qualification

September 21, 2026. Installed `codex-cli 0.148.0`, existing ChatGPT login. No credentials were copied into this app. The connector starts a separate stdio app-server for every invocation, uses ephemeral threads and a private temporary working directory, and resolves a supported default through `model/list`. The advertised default used in successful live tests was `gpt-5.6-sol`. The user's configured default `gpt-6-astra` was rejected by this CLI version; the connector does not inherit that stale setting.

## Verified boundary

`environments: []` disables environment access at thread and turn creation. Shell, file tools, browser, apps, plugins, memories, skills discovery and MCP access are disabled. Global instruction files are disabled and unexpected instruction sources cause a pre-dispatch failure. Per-thread overrides disable every configured MCP server. `agents.enabled=false` is necessary in addition to the multi-agent feature flags.

This runtime requires its code-mode host to execute dynamic tools. Its model-visible tools are `functions.exec` and `functions.wait`, a V8 wrapper with no Node, filesystem or network API. The only nested function is `workspace`. `orchestrator.skills.enabled=false` and `orchestrator.mcp.enabled=false` are necessary; merely hiding skills instructions leaves skill tools available.

The local-provider fixture in [codex-tool-catalog.json](codex-tool-catalog.json) records the actual provider request produced by the installed runtime. It verifies the wrapper tool list, the sole nested function, and absence of ambient skill/memory/app instruction markers. This test uses a local fake model response and consumes no provider capacity.

The live tests in [codex-live.json](codex-live.json) use two separate real model threads. One agent creates and retrieves a Resource through a dedicated file-backed test callback. The second exercises V8 globals and an attempted `node:fs` import, observes only `workspace` in `ALL_TOOLS`, and cannot retrieve a private-file sentinel or the first agent's conversation. These connector tests do not claim to verify the application's PostgreSQL permission commands. Application-level live tests are a separate qualification.

## Delivery and interruption

The adapter awaits the application's dispatch-intent event before sending `turn/start`. Runtime IDs, acceptance, tool calls and observed termination have separate events. A transport failure after dispatch returns `uncertain` and never automatically retries. Interrupt request and acknowledgment are distinct from a terminal `interrupted` turn. An unobserved terminal outcome remains uncertain.

Ten protocol fixtures verify bound callback delivery, metadata, duplicate tool-call memoization, wrong-thread rejection, denial of repository tools in chat, observed cancellation, transport loss, version/instruction-source failures and request timeouts. These interruption checks are fixtures; live app interruption still needs application-level qualification. Tool callbacks receive trusted `{callId, threadId, turnId}` metadata for persistent application command deduplication. In-memory memoization alone is not an exactly-once guarantee.

Execution uses the same runtime isolation and only adds checked `repo.list`, `repo.read`, `repo.write` and `repo.check` operations. `repo.write` takes `{path, text}`. The application fixes the checkout and checks paths. Arbitrary shell commands and project test scripts are unavailable in this profile. Real repository execution must be qualified through the application's checked callback.

## Reproduction

With the repository's Node 24 and pnpm environment:

```sh
pnpm exec tsx --test tests/codex-protocol.test.ts
pnpm exec tsx scripts/qualify-codex.ts catalog
pnpm exec tsx scripts/qualify-codex.ts live
```

The live command uses account capacity and stops its child runtimes when each test completes. The runtime version is pinned in the adapter; another version fails closed until its profile is inspected and qualified again.

Protocol references are the installed generated schema under `/tmp/workspace-codex-schema`, the [official app-server documentation](https://developers.openai.com/codex/app-server/), and the [official configuration schema](https://developers.openai.com/codex/config-schema.json). Remote documentation informed configuration choices; the emitted-catalog fixture verifies behavior on the installed binary.
