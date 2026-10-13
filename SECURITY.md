# Security Policy

ReaMesh opens network ports inside REAPER, moves project files between peers,
and lets a session host stream plugin chains into other people's projects.
Security reports are taken seriously and handled quickly.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Use GitHub's private vulnerability reporting:
[Report a vulnerability](https://github.com/ReaMesh/ReaMesh/security/advisories/new),
or email [contact@reamesh.com](mailto:contact@reamesh.com). You will get an acknowledgement within a few days. Once a fix is released the
advisory is published with credit to you, unless you prefer otherwise.

Please include what you can of: the affected component (extension, Rust core,
companion app, website), REAPER and OS version, steps to reproduce, and what
an attacker gains.

## Scope

In scope, and especially interesting:

- Bypassing the session passphrase: downloading, editing, or appearing in a
  roster without the token.
- Reading or forging session traffic on a passphrase-protected session
  (the sealed gossip envelopes, the presence MAC, replay of proposals).
- Staying in or rejoining a session after the host blocked your device.
- Hijacking an invite-code join, so a host other than the one named in the
  code is joined.
- Downloading files the host's project doesn't use.
- Path traversal or arbitrary file write through the transfer protocol.
- Memory-safety bugs reachable from a peer (state-chunk parsing, transfer
  decoding, zstd bounds).
- Driving the localhost control server from a web page or another user on
  the machine.
- Crashing a peer's REAPER remotely.

## Out of scope, by design

- **Joining a session runs the host's plugins.** A joiner downloads and opens
  the host's `.rpp`, and the host can stream new FX chains mid-session. That
  is arbitrary code execution and it is the product; the companion app warns
  before joining. Reports that "the host can load a plugin on my machine" are
  not vulnerabilities. Reports that a *non-host* can do this are.
- Open (no-passphrase) sessions are plaintext on the LAN. Confidentiality is
  only claimed for passphrase-protected sessions.
- Weak passphrases. Argon2id slows brute force; it does not make "1234" safe.
- Attacks that require a peer who already holds the passphrase to misbehave
  beyond what the editing model allows are usually a product question, not a
  security one. Send them anyway if unsure.

## Supported versions

Only the latest release receives fixes.
