# Roger-Wilco

Durable mail, presence, and work claims between agent sessions on one owner's
hosts. `KERNEL.md`, `INVARIANTS.md`, and `GLOSSARY.md` are frozen: read them
before changing anything.

## Build and test

```sh
cargo test --offline          # unit seams + process seam (real coordinator, daemon, session)
cargo build --release --offline
```

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
`DECLARED address= declared=` line after that session's `SESSION` line. Shims live outside `src/`: `shims/pi/roger.ts` (Pi extension) and
`shims/claude/roger-mcp.mjs` (Claude Code MCP server).

## Checks

`scripts/check` is the gate every change passes before it lands on main (fmt, clippy with
warnings denied, tests, harness-free). Review rules are in `CODING_STANDARDS.md`.

```sh
scripts/check                     # the gate
scripts/acceptance cross-host     # proof 1: mail survives a recipient SIGKILL across hosts
scripts/acceptance broadcast     # proof 3: one broadcast reaches every registered session once, no wake
scripts/acceptance claims        # proof 4: claims are exclusive, renew, expire; board on both hosts
scripts/acceptance wake           # proof 5: directed mail wakes an idle Pi session; busy queues to the turn boundary
scripts/acceptance pi-shim        # Pi shim lifecycle and roger_send tool callback; no model
node --test shims/claude          # Claude shim protocol and lifecycle against a scripted adapter
scripts/acceptance independence   # proof 2: Pi and Claude Code sessions (plus a Tracer) exchange mail; loopback
scripts/acceptance references     # proof 6: references keep both integers verbatim; relabel keeps the Address
scripts/check-harness-free        # invariant: no harness identity in the core
```

`scripts/acceptance references` reads the Board through `roger board` (proof 4) and also
searches `src/` for any code that parses a Reference or inspects a label (invariant 3).

`scripts/acceptance independence` needs `pi`, `claude` (logged in) and `node`. It makes one
real Claude Code model call (haiku); Pi needs none. Claude's tool results are read from its
own stream-json output.

Delivery to a model is best effort and separate from the core's guarantee. The adapter writes
the Mail to its Store (durable), then tells the shim, then sends the Ack; the Ack stays the
adapter's responsibility and a shim never sends it. What a shim has shown its harness is not
durable. The Claude shim keeps unshown Mail in memory for `roger_inbox`. If it dies, the Mail is
still in the Store. If the adapter died before its Ack, the core redelivers and the Store reuses
the same record, so a restarted shim is told again; Mail already acked is not replayed to a model.
The Pi shim writes each delivery into Pi's transcript.

Wake is the shim's job; the core carries no busy or idle state. The Pi shim declares
`wake-latency-ms=<n>` through `--declare` (`ROGER_WAKE_LATENCY_MS`, default 1000; `off` declares
nothing and never wakes). A delivery with `wake: true` goes to Pi as
`sendMessage(..., { triggerTurn: true, deliverAs: "followUp" })`: an idle Pi starts a turn, a
busy Pi queues it and hands it to the model at the turn boundary. A `wake: false` delivery
(a broadcast copy) is rendered with `triggerTurn: false`. The Store write and the Ack wait for
neither. `scripts/acceptance wake` drives this with a scripted faux model
(`shims/pi/test/faux-model.ts`), so it needs `pi` but makes no model call. The Claude shim
declares nothing and does not wake.

`scripts/acceptance cross-host` runs the coordinator on this host's tailnet
address, one host daemon here and one on `$ROGER_REMOTE_HOST` (default
`agent-jail`), and requires SSH to that host plus `scp`. It builds release
binaries, then removes every process and directory it created.

## Layout

| Path | Role |
| --- | --- |
| `src/coordinator.rs` | central durable store; pushes committed mail |
| `src/claims.rs` | claim decisions; expiry read from each committed deadline, never swept |
| `src/daemon.rs` | one per host; local inbox; serves sessions on a Unix socket |
| `src/session.rs` | contract-shaped agent session (Tracer) and adapter; writes its Store, then acks |
| `shims/claude/roger-mcp.mjs` | Claude Code shim: `roger_send` and `roger_inbox` MCP tools over the adapter |
| `shims/pi/roger.ts` | Pi shim: renders mail into the transcript, forwards `/roger-send` and the `roger_send` tool |
| `src/durlog.rs` | append-only fsynced log; replay drops a crash-truncated tail |
| `src/json.rs`, `src/net.rs` | one-object-per-line codec |
| `tests/process.rs` | process seam, including the SIGKILL window |
