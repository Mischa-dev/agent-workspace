# GitHub App compatibility

## Locked API and permissions

- REST API version: `2026-03-10`
- App JWT: RS256, issued 60 seconds in the past, expires after nine minutes
- Installation tokens: repository-scoped, opaque, and discarded after the
  child API or Git operation
- Initial permissions: Metadata read, Contents read/write, Pull requests
  read/write, Checks read
- Actions read is added only if Checks cannot provide the required state
- Events: installation/repository access, push, pull request, review, check
  suite, and check run only

The installation API's explicit `events` array does not enumerate GitHub's
automatically delivered installation and repository-access webhook types. The
retained live installation reports `push`, `pull_request`,
`pull_request_review`, `check_suite`, `check_run`, and an optional legacy
`repository` subscription. M5 accepts that configuration but the webhook
listener does not accept or persist `repository` deliveries; its payload
allowlist remains limited to the product events above.

The development App is installed only on
`Mischa-dev/agent-workspace-m0-spike`. The private key and webhook secret are
stored outside Git with owner-only permissions.

The live development registration is `Agent Workspace M0 Development` (App ID
`4442593`, installation ID `150232852`). App and installation IDs are public
identifiers, not credentials. The installation has access to the spike
repository only.

## Credentialed Git boundary

Clone/fetch/push operations use an absolute Git executable, argument arrays,
`core.hooksPath=/dev/null`, empty credential helpers, a temporary owner-only
`GIT_ASKPASS` script, and a cleared child environment. The installation token
exists only in the child environment and is redacted from bounded error output.
It never appears in a URL, argument, durable Git configuration, log, fixture,
or provider environment.

## Webhook boundary

The listener binds separately on `127.0.0.1:3101`. It verifies:

- `X-Hub-Signature-256` using HMAC-SHA256 and constant-time comparison
- a non-empty `X-GitHub-Delivery`
- an explicitly subscribed `X-GitHub-Event`
- exact `repository.full_name`
- bounded in-memory duplicate delivery IDs

The Cloudflare Quick Tunnel exposes only this listener and is stopped
immediately after the signed delivery proof.

## Verification record

Automated tests cover signature rejection, repository identity, delivery
deduplication, opaque-token redaction, and the child-only Git credential
boundary.

The live M0-008 sequence completed on July 30, 2026 (America/Denver):

- The App was created and installed only on the private spike repository.
- Each remote operation requested a fresh repository-scoped installation
  token. The backend cloned the repository, pushed a unique branch, opened
  pull request `#2`, read an empty Checks result, closed the pull request, and
  deleted the branch.
- Actions access was not added because the Checks endpoint was readable; the
  spike repository simply had no check runs configured.
- GitHub delivered signed push and pull-request events through the Quick
  Tunnel. Delivery `7790a17e-8c99-11f1-9b64-0d912688f536` returned HTTP 202.
  Redelivering the same delivery produced `duplicate: true` and HTTP 200.
- The tunnel was stopped immediately after duplicate proof. The App webhook
  was then deactivated while the development App and repository installation
  were retained for repeatability.
- The redacted evidence fixture is
  `tools/github-spike/fixtures/live-evidence.json`.

The current GitHub settings form retained
`integration[hook_attributes][_destroy]=true` when the previously inactive
webhook was first enabled. The browser acceptance run corrected that hidden
marker to `false`, submitted the intended active configuration, and re-read
the persisted state. Once the App webhook exists, later temporary URL and
secret changes use the documented `PATCH /app/hook/config` JWT endpoint through
the checked-in `configure-webhook` binary. The App remained inactive after the
proof.
