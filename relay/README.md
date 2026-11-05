# ReaMesh relay

ReaMesh connects computers directly. That works on a local network or a VPN,
but across the internet routers and firewalls often stand in the way. The
ReaMesh relay is a small server you run yourself, for your own band, studio
or circle of collaborators, that gets those connections through:

- It introduces host and guests so they can open a **direct connection**
  through their routers (hole punching). Usually that succeeds, and from then
  on the relay carries nothing.
- When no direct path exists, it **passes the traffic along**, within limits
  you set.

There is no public ReaMesh relay. Whoever runs a relay runs it for the people
they give a relay code to; nobody else can use it.

**How access works.** The relay prints a *relay code*. The host pastes it into
the relay field in ReaMesh's settings; from then on the host's invite links
carry the relay, so guests set up nothing. Only computers holding a relay code
can host through the relay, and guests can only reach those hosts. Each code
carries an access key; give everyone the same one, or one per person so you
can revoke them separately.

## Setup

Any always-on machine with a public address works: a small VPS, a Raspberry
Pi at home with port forwarding, a spare server. It needs little: the smallest
VM is plenty, and bandwidth only goes to the sessions it has to relay.

### Docker

```sh
curl -fsSLO https://raw.githubusercontent.com/ReaMesh/ReaMesh/main/relay/docker-compose.yml
docker compose up -d && docker compose logs
```

The log shows the relay code. Keys and status:

```sh
docker compose exec relay reamesh-relay key add lena
docker compose exec relay reamesh-relay status
```

The config lives in the `data` volume (`/data/config.toml` in the container);
`docker compose exec relay reamesh-relay code` prints the codes again. To build
the image from source instead: `docker build -f relay/Dockerfile -t
ghcr.io/reamesh/reamesh-relay:latest .` from a checkout of the repository.

### VPS with systemd

1. Download the build for your machine (`x86_64`, or `aarch64` for Arm servers
   and a 64-bit Raspberry Pi). It is one static file and runs on any Linux:

   ```sh
   curl -fsSL -o reamesh-relay https://github.com/ReaMesh/ReaMesh/releases/latest/download/reamesh-relay-linux-x86_64
   sudo install -m 0755 reamesh-relay /usr/local/bin/
   ```

   With [Rust](https://rustup.rs) you can build it instead:
   `cargo install --locked --git https://github.com/ReaMesh/ReaMesh reamesh-relay`.

2. Install the service (`reamesh-relay.service` from this folder) and start it:

   ```sh
   sudo curl -fsSL -o /etc/systemd/system/reamesh-relay.service https://raw.githubusercontent.com/ReaMesh/ReaMesh/main/relay/reamesh-relay.service
   sudo systemctl enable --now reamesh-relay
   ```

3. Read the relay code from the log and open the firewall (see below):

   ```sh
   journalctl -u reamesh-relay
   ```

Manage keys with `sudo reamesh-relay --data /var/lib/reamesh-relay key add lena`;
the running relay picks the change up by itself.

### Raspberry Pi

The same as a VPS with the `aarch64` build (64-bit Raspberry Pi OS), or Docker.
A Pi at home sits behind your router, so:

1. Forward TCP and UDP port 4001 on the router to the Pi.
2. Your home IP probably changes: set up a dynamic DNS name (many routers
   offer one) and put it in `config.toml` as `dns_name = "…"`.
3. Restart or reload, then print the code: `reamesh-relay code`.

## Ports and firewall

Open **TCP and UDP 4001** (or whatever `tcp_port` and `quic_port` say) for
incoming connections, in the server's firewall and in the provider's
(security group, cloud firewall):

```sh
sudo ufw allow 4001/tcp && sudo ufw allow 4001/udp
```

Port 443 gets through most restrictive networks (hotels, offices) if nothing
else on the server uses it: set both ports to 443, and for the systemd service
uncomment `AmbientCapabilities=CAP_NET_BIND_SERVICE`.

**Public address.** The relay puts the public IPs of its network interfaces
into the code. Many cloud servers (Oracle Cloud, AWS, Google Cloud) only see a
private IP and are reached through NAT; then the relay warns that it found no
public address. Set the server's public IP (`public_addrs = ["203.0.113.7"]`)
or a DNS name (`dns_name`) in `config.toml`, and run `reamesh-relay code`.

**Free hosting.** Oracle Cloud's *Always Free* tier includes small Arm VMs
with 10 TB of outgoing traffic a month, plenty for a relay. Open the ports in
the VCN security list as well as in the VM (`iptables` on Oracle's Ubuntu
images), and set `public_addrs` as above.

## Configuration

`config.toml` in the data dir (`~/.reamesh-relay`, `/var/lib/reamesh-relay`
for the service, `/data` in Docker) explains every setting. The relay re-reads
it when it changes (or on `systemctl reload` / `kill -HUP`): keys, bans and
limits apply at once; changing a port, the mode or a per-circuit limit
reconnects everyone once.

| Setting | Default | |
| --- | --- | --- |
| `tcp_port`, `quic_port` | 4001 | Ports (TCP, UDP/QUIC) |
| `public_addrs`, `dns_name` | detected | What the relay code points to |
| `mode` | `full` | `coordination`: only help connections go direct, relay no bulk traffic |
| `log_addresses` | `false` | Client IP addresses are never logged unless set |
| `session_list` | `false` | Everyone with a key sees the sessions hosted here (see below) |
| `limits.max_hosts` | 32 | Hosts at the same time |
| `limits.max_circuits` | 64 | Relayed connections at the same time (also `_per_host` 16, `_per_ip` 8) |
| `limits.circuit_max_minutes`, `circuit_max_mb` | 720, 0 | Per relayed connection (0 MB = no limit); ReaMesh reconnects it |
| `limits.monthly_budget_gb` | 500 | Data sent per month (UTC); then coordination mode until the month ends (0 = unlimited) |
| `name`, `contact` | none | Who runs it; ReaMesh only shows them when something is wrong with the relay |
| `alerts.command`, `alerts.budget_percent` | none, 80 | See [Alerts](#alerts) |
| `update.auto`, `update.channel` | `true`, `stable` | See [Updates](#updates) |

The budget counts what the relay sends (what providers usually bill), as the
data inside the encrypted connections: real traffic is a few percent higher.

## Telling your hosts something

```sh
reamesh-relay notice set "Moving servers" --from "2026-11-11 09:00" --until "2026-11-11 14:00"
reamesh-relay notice clear
```

Hosts holding a key see it in ReaMesh within seconds, in their own time zone and
language: in the Lobby from a week ahead, in Settings and on the Session screen.
Guests joining by invite link see it in the join dialog. Both times are optional;
after `--until` it goes away by itself. A relay that stops or restarts also tells
its hosts on the way out, so they see a calm "pausing" line instead of a warning.

## Keys: dates and limits

```sh
reamesh-relay key add guest --expires 30d     # or a date: --expires 2026-12-24
reamesh-relay key expires guest 2027-01-31    # or: never
```

The key works through that day (the relay's time zone), then its hosts lose
access and see that it expired; renewing it works at once. In `config.toml` a
`[[keys]]` entry can also have its own limits:

```toml
max_hosts = 2             # hosts it may have online at once
monthly_budget_gb = 50    # its share of the traffic
```

`status` and `key list` show what each key used this month, counted for its hosts
and their guests. Once a key's share is used up its relayed connections last only
as long as coordination mode allows: enough to go direct, not enough to carry a
session.

## Alerts

With a command set, the relay runs it through the shell when something needs your
attention, with `REAMESH_RELAY_ALERT` (what) and `REAMESH_RELAY_MESSAGE` (a
sentence) in its environment:

```toml
[alerts]
command = 'curl -s -d "$REAMESH_RELAY_MESSAGE" https://ntfy.sh/my-relay'
budget_percent = 80
```

| `REAMESH_RELAY_ALERT` | When |
| --- | --- |
| `budget_warning`, `budget_used_up` | The monthly budget reached `budget_percent`, or ran out (once a month) |
| `key_budget_used_up` | A key used up its own share (once a month) |
| `key_expired` | A key's last day has passed |
| `udp_blocked` | Connections only ever arrive over TCP: UDP looks closed |
| `hosts_gone` | Several hosts left at once and none came back in 5 minutes |

A relay can't test its own public address from inside, so `hosts_gone` is a sign,
not a proof. Try the command with `reamesh-relay alert test`. The Docker image has
no shell, so there alerts only reach the log.

## Session list

Off by default. With `reamesh-relay list on` the relay keeps a list of the sessions
hosted through it, and everyone with a key sees it in their lobby and joins open
sessions with one click, each with their own key. Removing a key or letting it run
out takes that person's view away at once. `list off` forgets the list on the spot;
both apply without a restart. A relay without a list works exactly as before, and
a ReaMesh older than 1.2 neither sees the list nor shows up on it.

The host decides per relay whether a session is listed, and can change that during
the session. An entry is signed by the host and only valid on this relay, ends when
the host's reservation ends, and lives only in memory. It takes up about a kilobyte,
and people looking at the list take no host place. The list keeps working when the
monthly budget is used up; ReaMesh then says joining takes a direct connection.

`status` shows how many sessions are listed, how many people are watching and which
hosts are listed, never a session's name.

A relay code printed while the list is on says so, and ReaMesh tells the person
adding the relay from that code what it means for their sessions. Codes printed
before keep working; ReaMesh finds out about the list on its first connection then.

## Updates

The relay, the ReaMesh app and the extension release together under one version.
By default the relay keeps itself up to date: every 6 hours it reads the release
channel, downloads a newer build into `update/` in the data dir, and restarts into
it at a moment nothing is being relayed. Hosts reconnect by themselves; guests
already connected directly don't notice. If nothing is ever free it simply waits.

Every build is signed with the ReaMesh release key, checked after the download and
before every start, and has to run and report the announced version before it is
kept. A build that doesn't get as far as listening is dropped on the next start,
the relay carries on with the version it had and doesn't take that one again.

With `auto = false` under `[update]` the relay only says when a version is out, in
the log and in `status`; `reamesh-relay update` gets it. `channel = "beta"` also
takes pre-releases. The installed binary (or image) stays untouched as the
fallback, and installing a newer one simply takes over. Updates come for the
published Linux builds; a relay built yourself for another system updates by
rebuilding.

**Older ReaMesh versions keep working.** A relay only passes encrypted bytes along
and never sees what a session contains, so a 1.3.0 relay carries 1.2.0 sessions
just as well. Host and guests have to run the same ReaMesh version among
themselves, with or without a relay. ReaMesh shows the relay's version in Settings,
and calls it outdated only when it lacks something the app needs.

## Commands

```
reamesh-relay [run]           run the relay (creates the data dir on first start)
reamesh-relay status          whether it runs, this month's traffic, the hosts online
reamesh-relay status --watch  the same, kept on screen and updated every 2 seconds
reamesh-relay code [name]     print the relay code for a key (every key without a name)
reamesh-relay key add <name>  create a key and print its relay code (--expires 30d)
reamesh-relay key list        names, fingerprints, dates and when each key was last used
reamesh-relay key expires <name> <when>  change a key's last day (a date, 30d, never)
reamesh-relay key remove <name>  delete a key; its hosts lose access within a few seconds
reamesh-relay list [on|off]   whether everyone with a key sees the sessions hosted here
reamesh-relay notice [set|clear]  show, set or clear the announcement hosts see
reamesh-relay alert test      try the alert command
reamesh-relay update          get the newest version now
```

`code <name> --copy` and `key add <name> --copy` also put the code on your
clipboard, over SSH too, in terminals that allow it (OSC 52: Windows
Terminal, iTerm2, kitty, WezTerm, and tmux with `set-clipboard on`). In
terminals with links (OSC 8), a click on a printed code opens ReaMesh, which
asks before it adds the relay.

Only one relay runs per data dir: a second start on the same dir stops with a
message instead of fighting the first over the ports.

All take `--data <dir>` (or `REAMESH_RELAY_DATA`). `run` also takes
`--log-format json` for log collectors. To type less, add
`alias rr=reamesh-relay` to your shell profile.

A first start, in a terminal (the ReaMesh R sits left of the block):

```
ReaMesh Relay 1.1.0

● running   mode full
port      4001, TCP and QUIC, IPv4 and IPv6
address   203.0.113.7
data      ~/.reamesh-relay

New relay set up. Relay code for default:

reamesh://relay/12D3KooW…?a=203.0.113.7&t=4001&q=4001&k=…&n=default

Paste it in ReaMesh under Settings › Relay. It lets whoever has it host
through this relay, so keep it within your circle.

21:28:38  ready
21:28:44  host band connected
21:28:46  guest joined band via relay
21:28:49  relayed connection to band ended after 3 s
```

A relayed connection that ends after a few seconds usually means host and
guest went direct; one that lasts hours is a session the relay carried. A
config change logs what changed (`max_hosts 32 → 48`, `key lena added`), and
Ctrl+C ends with a summary (`stopped after 3 h 12 min: 4 hosts, 11 relayed
connections, 1.2 GB sent`). When the day changes under a running relay, a
line with the date goes into the log; under systemd the journal stamps lines
itself, so there the relay leaves its own time out.

`status` while it runs:

```
● running for 2 h 14 min   mode full   version 1.1.0

traffic   32 GB this month, now 1.4 MB/s
budget    █░░░░░░░░░░░░░░░░░░░  6 % of 500 GB, about 96 GB by Nov 30
inbound   TCP 12 s ago   QUIC 40 s ago

HOST   KEY        ONLINE      RELAYED
band   ec24d153   1 h 2 min   1 guest
lena   96f676b3   12 min      none
```

The budget line looks ahead once a day of the month has passed, and turns
amber when the month's pace would use the budget up (`used up around Nov 21
at this pace`). `inbound` says when a connection last came in over each
transport: ReaMesh dials both at once and QUIC usually wins, so when only TCP
ever gets through, UDP is most likely closed in a firewall.

Warnings worth acting on are tagged: `limit` when a host, a guest address or
the relay as a whole is at its limit, `budget` when the month's budget is used
up or nearly, `firewall` when several connections came in over TCP and none over
QUIC, `key` when a key expired, `update` when an update failed, `reachability`
when every host left at once. In `docker logs`, journald or any other non-terminal (and with `NO_COLOR`
set) the output is the same text without colour or symbols.

## Privacy

The relay logs events by key name and counts, not who connected from where;
set `log_addresses = true` to log client IPs. Relayed sessions stay end-to-end
encrypted between host and guests; the relay sees only that bytes pass.
`status.json` in the data dir lists the hosts currently connected (key label
and ID), `seen.json` when each key was last used. Reading the release channel is
the only connection the relay opens by itself; `auto = false` doesn't stop that,
it only stops installing.

**With the session list on, that changes for what's on the list:** the relay holds
the name of each listed session, its host's name and the project's size, and passes
them to everyone with a key. It holds them in memory only, and writes them neither
to the log nor to `status.json`. For an open session the entry also contains the
secret that lets people join, and that is the point of the list: it is how a click
is enough. So you, as the operator, could join an open session, and you'd show up
in its list of people like anyone else. You still can't read along: what a session
sends stays encrypted between the people in it. A session with a password is listed
without any way in, and a host who doesn't want a session seen at all turns it off
for this relay. An entry never names the host's IP address or the project's file
name.

## License and the logo

The relay is licensed under [GPL-3.0-only](../LICENSE) like the rest of
ReaMesh. The R it prints comes from [`proprietary/`](../proprietary/) and isn't
covered by the GPL, but unmodified builds of the relay may carry it: building,
running and sharing them, a Docker image on a registry included, is fine. A
modified relay must replace `proprietary/logo/mark.txt` and
`proprietary/logo/mark-blocks.txt` with its own artwork under the same names
(see [License and trademark](../README.md#license-and-trademark)).
