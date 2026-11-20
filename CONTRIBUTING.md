# Contributing

## Before the licence

The project hasn't chosen its licence yet. Until a `LICENSE` file is here,
the code is published to read and run, and pull requests from outside the
project wait: once a licence is chosen, contributions open under it.
Issues and security reports (see [`SECURITY.md`](SECURITY.md)) are welcome
now.

## Sign-off (DCO)

Every commit from outside the project will carry a sign-off,
`Signed-off-by: Your Name <you@example.com>` (`git commit -s`), certifying
the [Developer Certificate of Origin](https://developercertificate.org/):
that you wrote the change or have the right to submit it under the
project's licence. There is no separate contributor agreement, so changing
the licence later would need every contributor's agreement.

## How changes are made

- [`AGENTS.md`](AGENTS.md) has the rules for every change, for people and
  agents alike: where a new write goes, how access is decided, the guard
  tests that hold the foundations, and which checks to run.
- `.claude/skills/` has step-by-step guides for the common jobs: a new
  operation, a migration, an MCP tool, checking a page in a browser,
  driving a pull request to green.
- One pull request per piece of work, with tests where a bug would be
  silent and costly (access, private conversations, claims, credentials).
- Never commit a secret, a token, or a machine's address or path; the
  checks refuse some of these, and the rest is on you.
