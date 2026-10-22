# Roger-Wilco

Durable mail, presence, and work claims between agent sessions on one owner's
hosts. `KERNEL.md`, `INVARIANTS.md`, and `GLOSSARY.md` are frozen: read them
before changing anything.

## Build and test

```sh
cargo test --offline --features test-hooks   # unit seams + process seam (real coordinator, daemon, session)
cargo build --release --offline              # what deploy ships: no test hooks
```

The `test-hooks` feature adds `--hold-before-ack <marker>` to `session` and `adapter` (park
between the Store write and the Ack, for the SIGKILL window). Only tests and
`scripts/acceptance` build with it; `scripts/deploy` refuses a binary that carries it.
`coordinator` and `daemon` read the fleet token from `ROGER_TOKEN` in their environment,
never from argv, which `ps` shows to every user.

One binary, no external crates. `roger` subcommands: `coordinator`,
`daemon`, `session` (the Tracer), `adapter` (a session fronted by a stdin/stdout
line stream for a shim), `send`, `label`, and the claim and Board client: `claim`, `renew`,
`release` (`--socket --owner --key [--ttl-ms]`; exit 3 when refused; `--owner` must be a
registered session Address, anything else is refused with a reason) and `board`
(`--socket`; prints `SESSION address= host= label=`, `CLAIM holder= remaining_ms= key=`,
`BOARD sessions= claims=`). A session may register with `--label <text>`;
`label --socket <sock> --from <id> --label <text>` changes it. Labels are stored and
returned verbatim and never change an Address or its mailbox. `send --to '*'` is a
Broadcast: the coordinator commits it once and fans it out to every session registered at
that moment, one Mail per recipient with id `<mail>/<address>`, each delivered and acked
like directed mail; a Mail id containing `/` is refused, so no directed id can equal a
copy id. Every delivery carries `wake`: true for directed mail, false for a
broadcast copy; the Tracer prints it on its `STORED` line and the adapter passes it to the
shim. `session` and `adapter` take `--declare <text>`: opaque text the session states about
itself at Register, stored by the core and returned verbatim; `board` prints it as a
`DECLARED address= declared=` line after that session's `SESSION` line.

## Registration

A Register adds an Address to the Board and to every later Broadcast. The registration ends
in one of two ways, both committed to the coordinator log as an `unregister` event:

- the session says so: `{"type":"unregister"}` on its daemon connection (the adapter sends
  it when its stdin closes), at once;
- the session stays disconnected (its connection or its host daemon's link closed, or it
  never re-registered after a coordinator restart) for `--unregister-after-ms` (default
  300000), read like a claim deadline at the next request that reads the registered set
  (Broadcast, Board, claim, relabel). No timer scans for it.

Mail already accepted for an unregistered Address stays pending and is delivered if that
Address registers again; a connected session never expires.

## Deploy

`scripts/deploy <host> [commit]`, run on the coordinator host, installs the core on
`<host>`: the release binary at `~/.local/bin/roger` (the replaced one kept as
`roger.prev`), `~/.config/roger/env`, and the systemd user units in `deploy/`
(`roger-daemon.service` on every host, `roger-coordinator.service` on the coordinator host).
The host daemon serves `$XDG_RUNTIME_DIR/roger/daemon.sock`. That socket and the binary are
the whole surface for anything on top: a plain client (`roger send`, `roger board`), the
Tracer, or an adapter-fronted shim, each installed separately. Units start at boot only
with lingering enabled (`loginctl enable-linger`).

## Claims

A claim's deadline is `now + ttl_ms` on the coordinator's wall clock, in milliseconds
since the Unix epoch, and is stored absolute so it survives a restart; a monotonic
clock alone cannot do that. The coordinator therefore trusts its own clock: a forward
step (NTP, suspend and resume) frees held keys early and a backward step extends them.
The holder may repeat either grant and land in the same state: `claim` by the current
holder is a retry-safe refresh that resets the deadline, and `renew` sets the deadline
to `now + ttl_ms` even when that is sooner than before. One failed log append stops
the coordinator rather than letting memory and disk diverge; a restart replays the log.

## Checks

`scripts/check` is the core gate every change passes before it lands on main (fmt, clippy
with warnings denied, tests, harness-free, no-poll); it names no harness.
`scripts/check-all` runs it and then every `shims/<name>/check` it finds. Review rules are
in `CODING_STANDARDS.md`.

```sh
scripts/check                     # the core gate
scripts/check-all                 # the core gate, then each shim's own check
scripts/check-harness-free        # invariant: no harness identity anywhere in the core
scripts/acceptance cross-host     # proof 1: mail survives a recipient SIGKILL across hosts
scripts/acceptance no-poll        # invariant 2: mail, receipts, claim expiry, unregister on committed events; loopback
scripts/acceptance independence   # proof 2: sessions from two harnesses (plus a Tracer) exchange mail; loopback
scripts/acceptance broadcast      # proof 3: one broadcast reaches every registered session once, no wake
scripts/acceptance claims         # proof 4: claims are exclusive, renew, expire; board on both hosts
scripts/acceptance wake           # proof 5: directed mail wakes an idle session; busy queues to the turn boundary
scripts/acceptance references     # proof 6: references keep both integers verbatim; relabel keeps the Address
```

`scripts/check-harness-free` covers every file outside `shims/` and `VECTORS/` (source,
tests, scripts, deploy units, and docs, this README included), except `scripts/acceptance`,
whose proofs 2 and 5 run real harnesses by definition. What a proof needs from a harness is
in that shim's README.

`scripts/acceptance no-poll` runs the real binaries with no sweep (the core has none; a sweep
added later needs a switch that this check turns off). Each phase leaves one event trigger as
the only path to its result, so removing any trigger fails it, and its idle windows fail if any
core log grows while nobody asks.

`scripts/acceptance references` reads the Board through `roger board` (proof 4) and also
searches `src/` for any code that parses a Reference or inspects a label (invariant 3).

`scripts/acceptance cross-host` runs the coordinator on this host's tailnet
address, one host daemon here and one on `$ROGER_REMOTE_HOST` (default
`agent-jail`), and requires SSH to that host plus `scp`. It builds release
binaries, then removes every process and directory it created.

## Adapters

Anything that fronts a harness sits on top of the core as an adapter-fronted shim under
`shims/<name>/`, each with its own README, install script and `check`. A shim starts
`roger adapter` and talks to it in lines; the core never learns which harness is there.
Every durable log (a Store, the daemon and coordinator logs) is held under an exclusive OS
lock while open: a second process on the same Store fails at start, naming the file, and
exits nonzero. When the adapter's stdin reaches EOF it unregisters its Address (see
Registration) before it exits; a shim ends its adapter by closing that stdin.

Delivery to a model is best effort and separate from the core's guarantee. The adapter writes
the Mail to its Store (durable), then tells the shim, then sends the Ack; the Ack stays the
adapter's responsibility and a shim never sends it. What a shim has shown its harness is not
durable. If the adapter died before its Ack, the core redelivers and the Store reuses the same
record, so a restarted shim is told again; Mail already acked is not replayed to a model.

Wake is the shim's job; the core carries no busy or idle state. A shim that can start a turn
declares `wake-latency-ms=<n>` through `--declare` and acts on each delivery's `wake` flag;
the Store write and the Ack wait for neither.

## Layout

| Path | Role |
| --- | --- |
| `src/coordinator.rs` | central durable store; pushes committed mail |
| `src/claims.rs` | claim decisions; expiry read from each committed deadline, never swept |
| `src/daemon.rs` | one per host; local inbox; serves sessions on a Unix socket |
| `src/session.rs` | contract-shaped agent session (Tracer) and adapter; writes its Store, then acks |
| `shims/<name>/` | one adapter-fronted shim per harness: README, install, check |
| `src/durlog.rs` | append-only fsynced log; replay drops a crash-truncated tail |
| `src/json.rs`, `src/net.rs` | one-object-per-line codec |
| `tests/process.rs` | process seam, including the SIGKILL window |
