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

The development App is installed only on
`Mischa-dev/agent-workspace-m0-spike`. The private key and webhook secret are
stored outside Git with owner-only permissions.

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
boundary. Live App creation/installation, installation token issuance,
clone/fetch, unique branch push, pull request creation, check read, tunnel
activation, duplicate redelivery, PR closure, and branch deletion are the
operator-assisted M0-008 acceptance sequence.
