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
git clone https://github.com/ReaMesh/ReaMesh
cd ReaMesh/relay
docker compose up -d && docker compose logs
```

The log shows the relay code. Keys and status:

```sh
docker compose exec relay reamesh-relay key add lena
docker compose exec relay reamesh-relay status
```

The image builds from the repository root (compose sets that up), since the
relay shares its protocol crate (`relay-proto/`) with ReaMesh.

The config lives in the `data` volume (`/data/config.toml` in the container);
`docker compose exec relay reamesh-relay code` prints the codes again.

### VPS with systemd

1. Build and install (needs [Rust](https://rustup.rs)):

   ```sh
   cargo install --locked --git https://github.com/ReaMesh/ReaMesh reamesh-relay
   sudo cp ~/.cargo/bin/reamesh-relay /usr/local/bin/
   ```

2. Install the service (`reamesh-relay.service` from this folder) and start it:

   ```sh
   sudo cp reamesh-relay.service /etc/systemd/system/
   sudo systemctl enable --now reamesh-relay
   ```

3. Read the relay code from the log and open the firewall (see below):

   ```sh
   journalctl -u reamesh-relay
   ```

Manage keys with `sudo reamesh-relay --data /var/lib/reamesh-relay key add lena`;
the running relay picks the change up by itself.

### Raspberry Pi

The same as a VPS (64-bit Raspberry Pi OS; building takes a while on the Pi,
or use Docker). A Pi at home sits behind your router, so:

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
| `limits.max_hosts` | 32 | Hosts at the same time |
| `limits.max_circuits` | 64 | Relayed connections at the same time (also `_per_host` 16, `_per_ip` 8) |
| `limits.circuit_max_minutes`, `circuit_max_mb` | 720, 0 | Per relayed connection (0 MB = no limit); ReaMesh reconnects it |
| `limits.monthly_budget_gb` | 500 | Data sent per month (UTC); then coordination mode until the month ends (0 = unlimited) |

The budget counts what the relay sends (what providers usually bill), as the
data inside the encrypted connections: real traffic is a few percent higher.

## Commands

```
reamesh-relay [run]           run the relay (creates the data dir on first start)
reamesh-relay status          whether it runs, this month's traffic, the hosts online
reamesh-relay status --watch  the same, kept on screen and updated every 2 seconds
reamesh-relay code [name]     print the relay code for a key (every key without a name)
reamesh-relay key add <name>  create a key and print its relay code
reamesh-relay key list        names, fingerprints, dates and when each key was last used
reamesh-relay key remove <name>  delete a key; its hosts lose access within a few seconds
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
ReaMesh Relay 0.1.0

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
● running for 2 h 14 min   mode full   version 0.1.0

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
up, `firewall` when several connections came in over TCP and none over QUIC. In `docker logs`, journald or any other non-terminal (and with `NO_COLOR`
set) the output is the same text without colour or symbols.

## Privacy

The relay logs events by key name and counts, not who connected from where;
set `log_addresses = true` to log client IPs. Relayed sessions stay end-to-end
encrypted between host and guests; the relay sees only that bytes pass.
`status.json` in the data dir lists the hosts currently connected (key label
and ID), `seen.json` when each key was last used.

## License and the logo

The relay is licensed under [GPL-3.0-only](../LICENSE) like the rest of
ReaMesh. The R it prints comes from [`proprietary/`](../proprietary/) and isn't
covered by the GPL, but unmodified builds of the relay may carry it: building,
running and sharing them, a Docker image on a registry included, is fine. A
modified relay must replace `proprietary/logo/mark.txt` and
`proprietary/logo/mark-blocks.txt` with its own artwork under the same names
(see [License and trademark](../README.md#license-and-trademark)).
