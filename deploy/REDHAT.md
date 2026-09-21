# Red Hat deployment

Deployed September 21, 2026 after the owner authorized the hosting and domain choices.

- App: https://workspace.mischanelson.dev
- MCP: https://workspace.mischanelson.dev/mcp
- Owner: the existing `cloud-user` account reached with `ssh redhat`.
- Source and operations: `/home/cloud-user/agent-workspace`.
- Runtime: rootless Podman, application image with Node 24, PostgreSQL 18.6.
- Dedicated Cloudflare tunnel: `agent-workspace`, separate from the existing Crafty/map tunnel.
- Existing NANC, Crafty, Caddy and other applications were not redeployed or reconfigured.

## Access

The private owner credential is `/home/cloud-user/agent-workspace/.local/deployment/owner-token.txt` on Red Hat. A private copy is saved on this Mac at `/Users/mischanelson/Documents/ChatGPT/workspace/.local/hosted/owner-token`. Copy it without printing it:

```sh
pbcopy < /Users/mischanelson/Documents/ChatGPT/workspace/.local/hosted/owner-token
```

The owner browser uses a Secure, HttpOnly session cookie. Agents use their own revocable bearer credentials or the MCP OAuth flow. The public OAuth discovery, registration, PKCE consent, access renewal and official MCP client were verified over the actual Cloudflare route. Temporary qualification credentials were revoked.

Atlas and Ember retain their identities and conversations, with polling defaults on this host. Connect actual clients in Agents. The Mac's Codex/GitHub authentication was not copied; managed coding and authenticated GitHub reads need separate runtime setup on Red Hat. The local installation and its backup remain preserved.

## Services and storage

The app and worker publish only the app's loopback port `127.0.0.1:4310`; PostgreSQL has no published port. Persistent named volumes are `workspace-postgres-data` and `workspace-files`. The three containers each have a 384 MiB memory cap and one CPU, with automatic restart unless intentionally stopped. Observed idle use for all three was about 180 MiB; this is not a load benchmark.

The `cloud-user` systemd services are `workspace.service`, `workspace-backup.service` and `workspace-backup.timer`. Existing lingering was already enabled, so the services can start at boot without an interactive login. The detached-container units use delegation and `KillMode=process`; otherwise systemd can kill Podman's network helpers after the operator command returns. This was reproduced and corrected during the live backup test.

The system service `workspace-tunnel.service` uses `/etc/cloudflared/workspace.yml` and its own tunnel credential. The original `/etc/cloudflared/config.yml` was not edited. No extra public application port was opened.

Run operational commands on Red Hat:

```sh
cd /home/cloud-user/agent-workspace
systemctl --user status workspace.service workspace-backup.timer
node scripts/deploy-podman.mjs status
curl -H 'Host: workspace.mischanelson.dev' http://127.0.0.1:4310/health
systemctl --user restart workspace.service
```

The SSH environment on this host supports `systemctl --user` directly. Do not override its runtime/bus environment. For logs, this host stores user-unit output in the system journal:

```sh
sudo journalctl _SYSTEMD_USER_UNIT=workspace-backup.service -n 50
podman logs --tail 50 workspace-server
podman logs --tail 50 workspace-worker
sudo journalctl -u workspace-tunnel.service -n 50
```

## Backups and recovery

The timer runs at 03:00 America/Denver with up to fifteen minutes of jitter and catches up after a missed run. It briefly stops the application and worker for a consistent database/file pair, verifies both archives, writes checksums and restarts them. A brief HTTP 502 during that window is expected. PostgreSQL remains running. No automatic retention deletion is configured.

```sh
systemctl --user start workspace-backup.service
systemctl --user show workspace-backup.service -p Result -p ExecMainStatus
systemctl --user list-timers workspace-backup.timer
```

Backups live under `.local/deployment/backups/` with private directory/file permissions. Each successful backup includes a database dump, complete file archive, runtime configuration, owner credential, work-hook credential and checksum manifest. Failed partial backups are retained without a manifest. The latest verified backup was also copied to this Mac under `.local/hosted/backups/`; off-server copying is currently manual, not a scheduled remote-storage service.

An actual backup was restored into database `workspace_restore_20260921` and volume `workspace-recovery-files`. All 16 records had an identical fingerprint and the file checksum matched. The restored real server and worker ran on loopback port 14310 with dispatch paused, credentials revoked, OAuth grants cleared and no active/replayed work. The recovery containers are now stopped and the isolated recovery data remains available.

Follow the isolated restore procedure in `README.md` before replacing any production database. Never start restored work automatically. Container health probes use `node:http` to preserve the public Host header, and the image uses Docker format to retain its HEALTHCHECK metadata. Startup and backup completion wait for both API and worker health. This host refuses Podman's automatic user-timer registration, so container health intervals are disabled; the same probe runs explicitly during operations and is available through `podman healthcheck run workspace-server`. The app also exposes `/health`. No shared user bus was restarted to work around this. PostgreSQL archive verification must read standard input directly with `pg_restore --list`; passing `/dev/stdin` as a filename failed during the real Podman verification.

## Release and rollback

The deployed image is `localhost/agent-workspace:release-20260921-verified`, also tagged `localhost/agent-workspace:connected`. The previous server and worker containers are retained, stopped, as `workspace-server-rollback-20260921` and `workspace-worker-rollback-20260921`. The database/file volumes were preserved across promotion. No host reboot was performed because this server runs other applications; service startup/restart, persistence and restored-worker startup were checked directly.

Before an upgrade, run a verified backup, build a new image with bounded memory, and retain the previous app containers/image. Stop only the workspace app/worker, then deliberately replace those two containers against the existing volumes and run migrations. Confirm `/health`, owner access, MCP discovery and worker heartbeat before promoting the new image alias. A schema-changing rollback may require restoring the backup into an isolated database first.

The work-hook credential is separate from owner/agent credentials, stored in `.local/deployment/work-hook-token.txt`; its hash is supplied through `WORK_HOOK_TOKEN_HASH`. It authorizes only the existing approved-scope trigger endpoint. It does not grant a new execution scope or add a runtime to this host.

Evidence is recorded in `evidence/hosted-protocol.json` and `evidence/hosted-deployment.json`. Real third-party client heartbeat/wakeup configuration remains separate from the hosted transport qualification.
