# Security Policy

ReaMesh opens network ports inside REAPER, moves project files between peers,
and lets a session host stream plugin chains into other people's projects.
Security reports are taken seriously and handled quickly.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Use GitHub's private vulnerability reporting:
[Report a vulnerability](https://github.com/ReaMesh/ReaMesh/security/advisories/new),
or email [contact@reamesh.com](mailto:contact@reamesh.com). You will get an
acknowledgement within a few days. Once a fix is released the advisory is
published with credit to you, unless you prefer otherwise.

Please include what you can of: the affected component (extension, Rust core,
ReaMesh app, website), REAPER and OS version, steps to reproduce, and what
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
- A `reamesh://` link or a web page that makes the ReaMesh app join, host or
  install something without the user confirming it.
- Getting an app or extension update installed that wasn't signed with the
  ReaMesh updater key, or downgrading an installation to an older release.
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
  is arbitrary code execution and it is the product; the ReaMesh app warns
  before joining. Reports that "the host can load a plugin on my machine" are
  not vulnerabilities. Reports that a *non-host* can do this are.
- Open (no-passphrase) sessions have no access control and no
  confidentiality: anyone who can reach the host over the LAN or a shared VPN
  can download the project, edit it and read the traffic. Both are only
  claimed for passphrase-protected sessions.
- What a session announces in the clear: the lobby announce (session name,
  passphrase salt, host build) is visible to everyone on the LAN, and an
  invite code contains the host's IP addresses.
- Weak passphrases. Argon2id slows brute force; it does not make "1234" safe.
- Attacks that require a peer who already holds the passphrase to misbehave
  beyond what the editing model allows are usually a product question, not a
  security one. Send them anyway if unsure.

## Supported versions

| Version                                  | Supported |
| ---------------------------------------- | --------- |
| Latest release on the Stable channel     | Yes       |
| Latest release on the Beta channel       | Yes       |
| Anything older                           | No        |

Fixes ship as a new release on the update channels rather than as
backports to older versions; the ReaMesh app installs it, together with
the matching extension, through its built-in updater.
