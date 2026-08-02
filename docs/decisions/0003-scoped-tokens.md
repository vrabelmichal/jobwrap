# ADR-0003: Scoped tokens, not role-only authorization

Date: 2026-08-01

## Status

Accepted

## Context

A simple role ladder (Anonymous < Authenticated < Controller < Owner) lets a
controller act on every job. Agent tokens need fine-grained control so a
compromised low-scope token cannot terminate arbitrary jobs.

## Decision

Token scopes are evaluated independently of role ordering. A token may perform
an action only when it holds the matching grant for the job. The single
`authorize` function combines the profile's required access level with the
principal's identity and the token's grants.

## Consequences

* A token with `job:*:output` can watch but not signal.
* A token with `job:<id>:signal:int` can interrupt only that job.
* Every protected operation has negative authorization tests.
