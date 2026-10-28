# Self-host Agent Workspace on Red Hat Enterprise Linux 10 ARM64

This is the supported private-beta runbook for one Red Hat Enterprise Linux 10.1 ARM64
host. The Mac and the private GitHub repository remain canonical;
the host is an exact-commit build and runtime target. Nothing here creates
public application ingress, changes Tailscale, replaces `/usr/bin/node`, or
replaces a global Codex installation.

Commands are split between the Mac, the existing `bot` build operator, and
`sudo` on Red Hat. Read each destructive warning before running its block.

## Boundary and prerequisites

- `bot` is the interactive SSH/build operator. It needs passwordless sudo only
  for the checked host lifecycle.
- `agent-workspace` is a system, non-login, non-sudo service account.
- HTTP/WebSocket listens on `127.0.0.1:3100`; GitHub webhooks listen separately
  on `127.0.0.1:3101`; PostgreSQL listens on `127.0.0.1:55432`.
- PostgreSQL 18.4 runs in the rootless `bot` Podman Compose project.
- Node 24.18.1 and Codex 0.146.0 live below `/opt/agent-workspace`.
- Rust uses the repository's pinned toolchain below the `bot` account's
  `.local/share/agent-workspace` root.
- Releases are immutable directories at
  `/opt/agent-workspace/releases/<full-git-sha>`; `current` is the reversible
  selector.
- Configuration and external credentials live below `/etc/agent-workspace`
  with owner-restricted permissions and never enter Git, bundles, logs, or
  browser responses.

Required host programs are `git`, `curl`, `tar`, `xz`, `podman`,
`podman-compose`, `systemd`, and the ordinary C/C++ build toolchain. Install
them through the Red Hat package manager under the operator's change policy;
this repository never pipes a network response into a shell.

## Credential-free source transfer

First require the exact commit to be green in GitHub. On the Mac, replace the
example object ID with that full 40-character commit:

```bash
cd /Users/mischanelson/Documents/Agent-workspace
git fetch origin main
git merge-base --is-ancestor EXAMPLE_GREEN_COMMIT origin/main
git bundle create /tmp/agent-workspace.bundle EXAMPLE_GREEN_COMMIT
git bundle verify /tmp/agent-workspace.bundle
scp /tmp/agent-workspace.bundle bot@redhat:/home/bot/agent-workspace.bundle
```

The bundle contains repository objects only. It must not contain an `.env`,
GitHub App key, webhook secret, database password, browser session, MCP token,
or Codex profile. On Red Hat:

```bash
install -d -m 0700 /home/bot/deployments
if test ! -d /home/bot/deployments/agent-workspace/.git; then
  git clone /home/bot/agent-workspace.bundle /home/bot/deployments/agent-workspace
fi
git -C /home/bot/deployments/agent-workspace fetch \
  /home/bot/agent-workspace.bundle EXAMPLE_GREEN_COMMIT
git -C /home/bot/deployments/agent-workspace switch --detach EXAMPLE_GREEN_COMMIT
git -C /home/bot/deployments/agent-workspace status --short
rm /home/bot/agent-workspace.bundle
```

The last status output must be empty. Remove the Mac bundle after the host has
verified the commit: `rm /tmp/agent-workspace.bundle`.

## First host bootstrap

From the clean checkout as `bot`:

```bash
cd /home/bot/deployments/agent-workspace
sudo ./deploy/redhat/bootstrap-host.sh
sudo ./deploy/redhat/install-toolchains.sh
./deploy/redhat/install-build-rust.sh
```

Confirm isolation:

```bash
getent passwd agent-workspace
sudo -u agent-workspace sh -c 'test ! -w /opt/agent-workspace'
/usr/bin/node --version 2>/dev/null || true
/opt/agent-workspace/toolchains/node-v24.18.1-linux-arm64/bin/node --version
/opt/agent-workspace/bin/codex --version
```

Do not alter the system Node result or any existing global Codex path.

## Rootless PostgreSQL

Choose a new high-entropy PostgreSQL password interactively. Do not paste it
into shell history. Set it only in the `bot` login environment long enough for
the first Compose creation, then clear it:

```bash
cd /home/bot/deployments/agent-workspace
read -r -s -p 'New Agent Workspace PostgreSQL password: ' \
  AGENT_WORKSPACE_POSTGRES_PASSWORD
export AGENT_WORKSPACE_POSTGRES_PASSWORD
podman-compose -f docker-compose.dev.yml up -d
unset AGENT_WORKSPACE_POSTGRES_PASSWORD
podman ps --filter name=agent-workspace_postgres_1
```

The named volume retains PostgreSQL independently of application releases.
Never run `podman-compose down -v` on a retained host.

## Owner-only runtime configuration

Create `/etc/agent-workspace/agent-workspace.env` with `sudoedit`; do not pass
secrets on command lines. The required non-secret shape is:

```dotenv
AGENT_WORKSPACE_ENV=production
AGENT_WORKSPACE_SERVER_ADDR=127.0.0.1:3100
AGENT_WORKSPACE_WEBHOOK_ADDR=127.0.0.1:3101
AGENT_WORKSPACE_ALLOWED_ORIGINS=http://127.0.0.1:3100
AGENT_WORKSPACE_DATABASE_URL=postgres://agent_workspace:REPLACE_INTERACTIVELY@127.0.0.1:55432/agent_workspace
AGENT_WORKSPACE_DATABASE_MAX_CONNECTIONS=5
AGENT_WORKSPACE_DATA_ROOT=/var/lib/agent-workspace
AGENT_WORKSPACE_DATA_QUOTA_BYTES=21474836480
AGENT_WORKSPACE_MINIMUM_FREE_BYTES=5368709120
AGENT_WORKSPACE_CODEX_BINARY=/opt/agent-workspace/bin/codex
AGENT_WORKSPACE_CODEX_HOME_ROOT=/var/lib/agent-workspace/codex/profiles
AGENT_WORKSPACE_CODEX_HOST_CONCURRENCY_LIMIT=1
AGENT_WORKSPACE_CODEX_MAX_LINE_BYTES=1048576
AGENT_WORKSPACE_CODEX_MAX_STDERR_BYTES=262144
AGENT_WORKSPACE_CODEX_MAX_BUFFERED_EVENTS=64
AGENT_WORKSPACE_CODEX_REQUEST_TIMEOUT_SECONDS=120
AGENT_WORKSPACE_CODEX_SHUTDOWN_TIMEOUT_SECONDS=5
AGENT_WORKSPACE_DEMO_EXECUTOR=false
AGENT_WORKSPACE_LOG_FILTER=agent_workspace=info,tower_http=info
```

Install permissions after editing:

```bash
sudo chown root:agent-workspace /etc/agent-workspace/agent-workspace.env
sudo chmod 0640 /etc/agent-workspace/agent-workspace.env
sudo stat -c '%U %G %a %n' /etc/agent-workspace/agent-workspace.env
```

When people reach the app through a public address (a domain, a tunnel or a
reverse proxy), also set `AGENT_WORKSPACE_PUBLIC_ORIGIN` to that address, such as
`https://workspace.example.com`, and add it to `AGENT_WORKSPACE_ALLOWED_ORIGINS`.
Invitation links, sign-in callbacks and connect instructions use it; without it
they point at `SERVER_ADDR`, which only works on the host itself.

Agent wakeup webhooks must be public `https` addresses. An agent host that
listens on plain `http` on a private or tailnet address (such as the crew's
wakeup listener on its Tailscale IP) works only once its exact origin is listed
in `AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS`, comma separated, such as
`http://100.119.56.3:8732`. Set it before upgrading to a release with these
checks, or those wakeups start failing and agents fall back to their timers.

Two more optional settings. `AGENT_WORKSPACE_OPERATOR_LOGINS` lists, comma
separated, the logins (besides whoever ran `bootstrap-admin`) who see
Settings › Workspace › Server; a workspace owner who doesn't run the server
doesn't. `AGENT_WORKSPACE_ALLOW_PRIVATE_ADDRESSES=true` lets a workspace point
its model provider or SMTP server at this machine or a private network (such
as a local model or mail relay); without it both must be public addresses.

GitHub App settings are optional and must be installed as separate owner-only
files before adding their paths to this environment. Never print those files.

## Build, migrate, and deploy the exact commit

Deploying main over the release from before the roadmap? Start with
`docs/operations/alpha-deploy.md`, which lists the new settings it needs.

The deployment script installs a new immutable release, atomically selects it,
and restarts the hardened unit:

```bash
cd /home/bot/deployments/agent-workspace
test -z "$(git status --porcelain)"
git rev-parse HEAD
./deploy/redhat/deploy-release.sh
```

The server applies checked migrations before accepting traffic. Verify the
active object, service identity, loopback listeners, and health without
printing configuration:

```bash
readlink /opt/agent-workspace/current
systemctl is-active agent-workspace
systemctl show agent-workspace -p User -p Group -p NoNewPrivileges -p FragmentPath
curl --fail --silent http://127.0.0.1:3100/health/live
curl --fail --silent http://127.0.0.1:3100/health/ready
sudo ss -ltnp | grep -E '127\.0\.0\.1:(3100|3101|55432)'
sudo journalctl -u agent-workspace --since '-5 minutes' --no-pager
```

For a new private host, create the first browser administrator once, from an
interactive terminal. The command prompts twice and never accepts a password
argument:

```bash
sudo /opt/agent-workspace/current/bin/agent-workspace-service-command \
  bootstrap-admin \
  --login-name example-admin \
  --display-name 'Example Administrator' \
  --organization-name 'Example Private Host' \
  --organization-slug example-private-host
```

Do not run `dev-seed` in production. It is only for an explicitly disposable
development database on a loopback development configuration.

From the Mac, browse through SSH forwarding:

```bash
ssh -N -L 3100:127.0.0.1:3100 bot@redhat
```

Open `http://127.0.0.1:3100`. Port 3101 remains separate and private unless an
operator deliberately performs the documented temporary webhook ceremony.

## Retained upgrade

Before changing `current`, identify both ends and create an ordinary backup:

```bash
current_release="$(readlink /opt/agent-workspace/current)"
current_commit="${current_release##*/}"
printf '%s\n' "${current_commit}"
df -h /var/lib/agent-workspace /opt/agent-workspace /var/backups/agent-workspace
```

Write only the non-secret database name to an owner-only file, then use the
exact current release paths shown in the host-maintenance README. Validate the
finished backup before transferring the new Git bundle. Record the current and
previous migration checksums from the backup manifest; do not edit migration
rows manually.

After the exact new commit is green, repeat credential-free transfer and run
`deploy-release.sh`. Re-run readiness, listener, service identity, storage
report, retention dry run, and a controlled restart:

```bash
sudo /opt/agent-workspace/current/bin/agent-workspace-service-command \
  retention-external-events
sudo systemctl restart agent-workspace
curl --fail --silent http://127.0.0.1:3100/health/ready
```

External-agent event retention is a dry run unless the owner adds the literal
`--apply true`. It deletes only expired addressed invalidation rows in bounded
batches; it does not delete Messages, Work Items, approvals, Agent Runs,
connections, or audit history.

Provider profiles can require interactive reauthentication after restore or
credential rotation. Use the personal profile's normal Codex login ceremony;
never copy profile contents between identities.

## Rollback boundary

Application-only rollback is allowed only when the old binary understands the
current database migration set. Stop if the failed release applied any new
migration that is absent from the previous release. In that case use a forward
fix, or restore the pre-upgrade backup into a clean database and data location;
never delete migration rows or manually reverse production SQL.

When schemas are compatible, select the previous immutable directory and
restart:

```bash
sudo ln -sfn /opt/agent-workspace/releases/EXAMPLE_PREVIOUS_COMMIT \
  /opt/agent-workspace/current.next
sudo mv -Tf /opt/agent-workspace/current.next /opt/agent-workspace/current
sudo systemctl restart agent-workspace
curl --fail --silent http://127.0.0.1:3100/health/ready
```

If readiness fails, restore the failed release selector and retain logs. Do
not delete either release until the incident is closed.

## Maintenance and incident diagnostics

- The daily encrypted off-host backup, the separate secrets-key backup, and
  the restore drill are in `docs/operations/backups.md`.
- Backup, validation, clean-location restore, storage reporting, and database
  retention commands are in `tools/host-maintenance/README.md`.
- Run retention without `--apply` first. The explicit apply path returns only
  aggregate counts and uses a database advisory lock.
- Run `agent-workspace-server retention-worktrees` without `--apply` first.
  Dirty, unpushed, non-terminal, active-session, ambiguous, and too-young
  worktrees remain preserved even above the disk watermark.
- Rotate application-owned secrets with the checked Red Hat installer and the
  secret inventory. Revoke sessions/MCP credentials/OAuth grants through their
  bounded application commands; do not rotate a third-party account password.
- Restore requires a new database and empty drill roots. Never point restore at
  `current`, the active database, or `/var/lib/agent-workspace`.
- Repository repair uses the checked Git runtime's argument arrays and
  repository metadata lock. Never `git reset --hard` or delete a dirty tree to
  recover space.
- GitHub diagnostics use delivery IDs, bounded categories, permission state,
  and check status. Never dump installation tokens, App keys, webhook secrets,
  request bodies, or child environments.
- MCP/OAuth cleanup is covered by the retention policy; authorization failures
  require normal reauthorization, not copying token rows.

Useful bounded evidence:

```bash
systemctl show agent-workspace -p ActiveState -p SubState -p MainPID -p User
journalctl -u agent-workspace --since '-15 minutes' --no-pager \
  | grep -E 'request_id|error|failed|recovery|storage'
podman inspect --format '{{.State.Status}} {{.State.Health.Status}}' \
  agent-workspace_postgres_1
```

Investigate the first bounded error category. `storage` means new growth is
blocked while reads, interrupts, reporting, backup, and recovery remain
available. `git_worktree` means preserve and inspect the tree. `database`
requires SQLSTATE/constraint evidence and a backup before corrective work.
`provider` or authentication-required status means use the supported login
ceremony. None of these categories authorize manual database edits.

After a successful clean drill or incident test, remove only the exact
disposable bundle, test database, temporary root, transient unit, measurement
process, and credential created for that test. State what was removed and
whether a retained backup can recover it. Never use a recursive command against
`/`, `/home`, `/var/lib/agent-workspace`, `/opt/agent-workspace`, or an
unresolved variable.
