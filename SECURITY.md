# Security policy

## Reporting a vulnerability

Report vulnerabilities through GitHub private vulnerability reporting on
[Nowayz/WebSocket-Application-Relay](https://github.com/Nowayz/WebSocket-Application-Relay/security/advisories/new).
Do not open a public issue for a security problem. Include the commit or
release version, the configuration you used and a way to reproduce the
behavior. You will get an acknowledgement before any fix is published.

## Supported versions

Only the `main` branch is supported. Fixes are made there and included in the
next release; older release archives are not patched.

## Scope and deployment notes

The relay has no built-in authorization. Channel names and session IDs are
routing identifiers, not secrets. `WAR_AUTH_PASSWORD` only gates the
privileged relay commands (listener flags, channel listing and fixed-ID
assignment); it does not control who may join a channel, send to a session or
write a channel variable. For multi-tenant use the relay must sit behind an
authenticating ingress that decides who may connect, and applications must
carry their own identity and permission model inside their messages.

Keep the management listener private. A non-loopback management bind requires
`WAR_ADMIN_TOKEN`. Terminate TLS at the ingress and set exact browser origins
with `WAR_ALLOWED_ORIGINS`; a missing `Origin` header is still accepted for
non-browser clients. Resource budgets (`WAR_MAX_CONNECTIONS`,
`WAR_MAX_CONNECTIONS_PER_IP`, the queue, read-buffer and variable budgets and
the per-connection rate limits) are the denial-of-service controls; see
[docs/operations.md](docs/operations.md) for sizing.
