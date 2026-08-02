# ADR-0001: Two-executable architecture

Date: 2026-08-01

## Status

Accepted

## Context

The wrapped process must remain attached to the original terminal, while web
monitoring and control require a long-running process with an HTTP server.
Embedding the server in the wrapper would couple process lifetime to the
terminal session.

## Decision

Ship two executables:

* `jobwrap` — the foreground wrapper; the only component that creates processes.
* `jobwrapd` — a per-user daemon providing the HTTP API, registry, and storage.

They communicate over a private Unix socket with peer-credential verification.

## Consequences

* The web server cannot launch arbitrary processes by construction.
* A wrapper can run degraded (no monitoring) if the daemon is unavailable.
* An extra binary and socket are required, but socket-permission checks keep
  the trust boundary narrow.
