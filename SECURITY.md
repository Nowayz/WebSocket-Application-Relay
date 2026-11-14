# Security policy

## Reporting a vulnerability

Report vulnerabilities through GitHub private vulnerability reporting on
[Nowayz/WebSocket-Application-Relay](https://github.com/Nowayz/WebSocket-Application-Relay/security/advisories/new).
Do not open a public issue for a security problem. Include the commit or
release version, the configuration you used and a way to reproduce the
behavior. You will get an acknowledgement before any fix is published.

## Supported versions

Only the `master` branch is supported. Fixes are made there and included in the
next release; older release archives are not patched.

## Scope and deployment notes

The relay has no built-in authorization. Channel names and session IDs are
routing identifiers, not secrets. `WAR_AUTH_PASSWORD` only gates the
privileged relay commands (listener flags, channel listing and fixed-ID
assignment); it does not control who may join a channel, send to a session or
write a channel variable. For multi-tenant use, applications must carry their
own identity and permission model inside their messages; the relay admits any
client that completes the WebTransport handshake and passes the origin
policy.

Keep the management listener private. A non-loopback management bind requires
`WAR_ADMIN_TOKEN`. The relay terminates TLS itself: give it a publicly trusted
certificate (`--tls-cert`/`--tls-key`), never `--dev-cert` outside local
testing, and set exact browser origins with `WAR_ALLOWED_ORIGINS`; a missing
`Origin` header is still accepted for non-browser clients. The relay has no
connection, channel, membership or rate caps; its memory budgets (queue,
staging and variable bytes) and timeouts bound what a client can make it hold,
and a client that exceeds them is closed. Limit connection counts or rates per
address in front of the relay (firewall or load balancer) if you need them;
see [docs/operations.md](docs/operations.md) for sizing.
