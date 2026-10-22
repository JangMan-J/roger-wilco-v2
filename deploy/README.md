# Deploy

How the core is installed and run on the owner's hosts. What the core guarantees is in the
[top-level README](../README.md); a shim installs on top of a deployed core per its own
`shims/<name>/README.md`.

## Install

`scripts/deploy <host> [commit]`, run on the coordinator host, installs the core on
`<host>` and (re)starts its units. `commit` defaults to `HEAD`; deploy builds it from
`git archive` with `cargo build --release --offline` and refuses a binary that carries the
`test-hooks` feature. It is idempotent: run it again to update. `<host>` is an ssh
destination, or this host's own name for a local install.

On `<host>` it writes:

| Path | What |
| --- | --- |
| `~/.local/bin/roger` | the release binary; the one it replaces is kept as `roger.prev` |
| `~/.config/roger/env` (0600) | `ROGER_TOKEN`, `ROGER_COORDINATOR`, `ROGER_HOST_ID` (see Host config) |
| `~/.config/systemd/user/roger-daemon.service` | every host |
| `~/.config/systemd/user/roger-coordinator.service` | the coordinator host only |
| `~/.local/share/roger/deployed` | `commit=` and `sha256=` of the installed binary |

The units come from this directory (`deploy/`). At run time the host daemon serves
`$XDG_RUNTIME_DIR/roger/daemon.sock`, the socket every client, Tracer and shim on that host
uses.

## Host config

`~/.config/roger/env` on the coordinator host is the source for every host's env:

- `ROGER_TOKEN`, the fleet token, is generated once, on the first deploy from this host, and
  never rotated by deploy. The units pass it to the core through `EnvironmentFile=`, never
  argv (README `## Build and test`).
- `ROGER_COORDINATOR` is the address the coordinator listens on and every host daemon dials.
  On the first run it is `ROGER_BIND` if set, else this host's tailnet IPv4, port 7477.
- `ROGER_HOST_ID` is `<host>`'s own `hostname`, not the ssh name used to reach it, so a Host
  id matches the `clock` its Stores record. An ssh alias names the route; the hostname names
  the machine. Deploy refuses a hostname that is not `[A-Za-z0-9._-]+`.

## Services

| Unit | Runs | Durable log |
| --- | --- | --- |
| `roger-coordinator.service` | `roger coordinator --bind $ROGER_COORDINATOR` | `~/.local/share/roger/coordinator.log` |
| `roger-daemon.service` | `roger daemon --coordinator $ROGER_COORDINATOR --host-id $ROGER_HOST_ID` | `~/.local/share/roger/daemon.log` |

Both are systemd user units with `Restart=always` (3 s) and no start limit; the daemon
reconnects to the coordinator on its own. Units start at boot only with lingering enabled
(`loginctl enable-linger`). Deploy ends by printing each unit's state and the user's
`Linger` value, and reports `PASS deploy <host> <commit>` only when every unit is active and
the socket exists.

```sh
systemctl --user status roger-daemon roger-coordinator
journalctl --user -u roger-daemon -u roger-coordinator
cat ~/.local/share/roger/deployed
```

## Backups

Take one on every host before a deploy that changes the binary, units or env. The coordinator
log is the fleet's durable state (Mail, Receipts, sessions, claims); each host's daemon log is
its local inbox.

```sh
b=~/.local/share/roger-backup-$(date -u +%Y%m%d-%H%M%S)
mkdir -p "$b"/{bin,config,share,units}
cp -p ~/.local/bin/roger "$b/bin/"
cp -rp ~/.config/roger "$b/config/"
cp -rp ~/.local/share/roger "$b/share/"
cp -p ~/.config/systemd/user/roger-*.service "$b/units/"
```

A shim keeps its own state (its Stores); back that up per its README.

## Procedure

1. Run `scripts/check-all` on the commit to ship and land it on main.
2. Read the Board on each host (`roger board --socket "$XDG_RUNTIME_DIR/roger/daemon.sock"`)
   to see who is registered and what is claimed before the restart. Accepted Mail survives
   the coordinator and daemon restarts; sessions re-register.
3. Back up every host (above).
4. `scripts/deploy <coordinator-host> main`, then `scripts/deploy <host> main` for each other
   host. Each must end in `PASS`.
5. Check live: start a Tracer on two hosts, read the Board from both, send each way, and read
   each Receipt.

   ```sh
   sock="$XDG_RUNTIME_DIR/roger/daemon.sock"
   roger session --socket "$sock" --store /tmp/live.store --label live   # prints READY session=<address>
   roger board --socket "$sock"
   roger send --socket "$sock" --from <address> --to <other address> --mail live-1 --body ping
   roger receipt --socket "$sock" --mail live-1
   ```

   Stop the Tracers afterwards; they leave the Board after `--unregister-after-ms`.
6. `ps` shows no token for the coordinator or a daemon, and
   `grep -c -- --hold-before-ack ~/.local/bin/roger` prints 0.

## Roll back

Deploy an earlier commit: `scripts/deploy <host> <commit>`. `~/.local/bin/roger.prev` holds
the binary the last deploy replaced, and a backup holds the env, units and logs as they were.
