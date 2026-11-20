# Security

## Reporting a vulnerability

Please report it privately, through **Report a vulnerability** on this
repository's Security tab (GitHub's private vulnerability reporting). Don't
open a public issue, pull request or discussion for it.

Say what is affected, how to reproduce it, and what someone could do with
it. You'll hear back within a week; once a fix is out we publish an
advisory, crediting you unless you'd rather we didn't.

## Supported versions

Agent Workspace is before its first release. Fixes go to the main branch
and the next image built from it; there are no older versions to patch yet.
Once releases start, the latest release gets security fixes.

## What's in scope

The server, the web app, the container image and the documented
self-hosting setup. Everything here is built to keep each workspace's data
inside it and each private conversation to its members: a way to read or
change another workspace's rows, a direct message or group you aren't in,
or anything a role shouldn't allow is exactly what we want to hear about.
A model or provider you connect yourself is out of scope, as is a server
someone has configured against the documentation.
