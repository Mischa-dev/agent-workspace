# Upgrades

A release is one image. Upgrading is: back up, pull the new image, start
it. The server migrates the database as it starts.

```sh
cd deploy/container
docker compose --profile backup run --rm backup agent-workspace-server backup create --to /backups   # a set now
# set AW_VERSION in .env to the new release, then:
docker compose pull app
docker compose up -d
```

While it migrates, `/health/ready` answers 503 with the step it's on, and
the container shows healthy once it's done. Read the release notes first:
a release that needs something done by hand (a setting renamed, a step
before stopping the old server) says so there.

## More than one process

Only processes with the `work` role migrate (`AGENT_WORKSPACE_ROLES`, see
[configuration](configuration.md)); `api` and `live` processes start cold
and answer `/health/ready` with 503 until the database is at their
release's schema. So: start one `work` process of the new release first
(or run `agent-workspace-server migrate`), then the rest. Processes drain
when stopped (readiness turns 503 at once, live connections close with a
"reconnect" code, leases are handed back), so a rolling restart loses no
work.

## Going back

An older image refuses to serve a database that has migrations it doesn't
know: its `/health/ready` answers 503 and says so. So going back is only
possible when the newer release added no migration:

- **No new migration** (the release notes say so): set `AW_VERSION` back and
  `docker compose up -d`.
- **New migrations**: restore the backup you took before upgrading into a
  fresh database and data folder ([backups](backups.md)), then start the old
  image on it. Anything written since the upgrade is lost, so prefer a fixed
  newer release when there is one. Never delete rows from
  `_sqlx_migrations` by hand.
