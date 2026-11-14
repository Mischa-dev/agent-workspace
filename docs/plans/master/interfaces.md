# Interface changes

One row per change to a name, shape or behaviour that another plan relies on
(definition of done, item 14). Newest last.

| Date | Slice | Interface | Old → new | Plans that rely on it |
|---|---|---|---|---|
| 2026-11-14 | H1.2 | Outbound HTTP: `outbound::client_for`, `pinned_client`, `webhook_client`, `PagePolicy`, `permit(workspace)` | `replies::validate_webhook_url` and `link_previews::guard` → `outbound` (one guard; every address pinned, no redirects, no proxy, 8 at once per workspace) | A2 (webhooks), C1 (link previews), K5 (the `work` role) |
| 2026-11-14 | H1.12 | Configuration: `AGENT_WORKSPACE_<NAME>_FILE`, `AGENT_WORKSPACE_PROFILE` (`small`, `standard`, `hosted`), `AGENT_WORKSPACE_LINK_PREVIEWS`; `ALLOWED_ORIGINS` defaults to the public origin outside development | new | H1.13 (image, compose), H1.19, K5 |
| 2026-11-14 | H1.3 | `tenancy::tables`: `TENANT_TABLES`, `GLOBAL_TABLES`, `MISSING_WORKSPACE_COLUMN`, `MUST_BE_GLOBAL`, `tenant_column`, `is_global` | new | H1.4 to H1.7, H1.9, H1.21, H1.22, K4; every slice adding a table |
| 2026-11-14 | H1.20 | `tenancy::{Tenancy, scoped, system, signed_in, current}`, `SYSTEM_REASONS`; `db::{scoped_tx, scoped_one}`; `app.tenancy` = `m:<account>\|<ws>,…`, `w:<account>\|<ws>`, `s:<reason>`, `a`, set with `BEGIN` | new | K2.3 (the runner sets the scope; stopgap until then: `run_tool` and `jobs::execute`), H1.21, H1.22, H1.45 to H1.63, K5 |
| 2026-11-14 | H1.16 | CLI: `backup create\|schedule\|key`, `restore`, `restore drill` | new (same set format as `deploy/redhat`) | H1.13, H1.19 |
| 2026-11-14 | H1.41 | `GET /client-config` `{apiOrigin, liveOrigin, signIn: {methods}, signup, features, version}`; `GET /api/v1/auth/csrf`; `AGENT_WORKSPACE_LIVE_ORIGIN`; CORS for `ALLOWED_ORIGINS` only; web `apiUrl`, `liveUrl`, `setApiOrigins` | W1's text said `/api/v1/client-config` → `/client-config` (R30) | W1.38, K5.19 |
| 2026-11-14 | H1.44 | `tenancy/ratchet.json`: unscoped pool call sites per directory, keys `<dir>/` and `src a-c`, `src d-l`, `src m-r`, `src s-z` | new; may only go down | H1.45 to H1.63; every slice adding a query |
| 2026-11-14 | H1.8 | `export::{ExportDecl, KindDecl, ExportPart, parts, export_kind, import_kind}`; declarations in `export/registry.rs` | new (in-code list, R41) | M1.3 (moves them into `Registrar::export`), H1.9, H1.10; every slice adding a table (definition of done, item 16) |
