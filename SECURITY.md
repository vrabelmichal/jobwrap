# Security

## Reporting vulnerabilities

Please report security issues privately to the maintainers. Do not open a
public issue for a vulnerability.

Include:

* a description of the issue;
* the affected versions;
* a minimal reproduction;
* the impact you believe the issue has.

## Supported versions

Security fixes are applied to the latest release.

## Reporting policy

We will acknowledge reports within three business days and coordinate a fix
and disclosure timeline with you.

## What is in scope

* authorization bypasses (any operation a principal should not be able to
  perform);
* information disclosure through the API, WebSocket, or logs;
* remote code execution paths;
* privilege escalation through the daemon or wrapper;
* token or session compromise;
* denial of service through the daemon.

## General guidance

The security model is documented in `docs/security-model.md`. Any change that
relaxes a control listed there requires a security review.
