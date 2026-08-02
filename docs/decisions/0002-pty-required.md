# ADR-0002: PTY for the wrapped process

Date: 2026-08-01

## Status

Accepted

## Context

Using stdout/stderr pipes would change command behavior: buffering, loss of
color, missing progress bars, broken interactive prompts, no terminal
dimensions, and different Ctrl+C/Ctrl+Z semantics.

## Decision

The child always runs inside a pseudo-terminal in its own session and process
group. The wrapper relays bytes between the original terminal and the PTY
master, forwards signals and resize events, and restores terminal state with an
RAII guard.

## Consequences

* Output is raw bytes end to end (not UTF-8).
* Nested interactive shells have edge cases; `jobwrap bash` is supported but
  not the first acceptance criterion.
* All low-level OS code is isolated in `jobwrap-pty`; the only `unsafe` in the
  workspace lives there, with documented safety invariants.
