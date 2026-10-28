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
docker compose exec relay reamesh-relay key add Lena
docker compose exec relay reamesh-relay status
```

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

Manage keys with `sudo reamesh-relay --data /var/lib/reamesh-relay key add Lena`;
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
reamesh-relay [run]            run the relay (creates the data dir on first start)
reamesh-relay status           hosts, relayed connections, traffic, mode
reamesh-relay code [<label>]   print the relay code(s)
reamesh-relay key add <label>  new access key, prints its code
reamesh-relay key list         labels and fingerprints
reamesh-relay key remove <label>  revoke a key; its hosts are cut off at once
```

All take `--data <dir>` (or `REAMESH_RELAY_DATA`).

## Privacy

The relay logs events and counts, not who connected from where; set
`log_addresses = true` to log client IPs. Relayed sessions stay end-to-end
encrypted between host and guests; the relay sees only that bytes pass.
`status.json` in the data dir lists the hosts currently connected (key label
and ID).
