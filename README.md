# Roger-Wilco

Durable mail, presence, and work claims between agent sessions on one owner's
hosts. `KERNEL.md`, `INVARIANTS.md`, and `GLOSSARY.md` are frozen: read them
before changing anything.

## Build and test

```sh
cargo test --offline          # unit seams + process seam (real coordinator, daemon, session)
cargo build --release --offline
```

One binary, no external crates. `roger` has five subcommands: `coordinator`,
`daemon`, `session` (the Tracer), `adapter` (a session fronted by a stdin/stdout
line stream for a shim), `send`. Shims live outside `src/`: `shims/pi/roger.ts` (Pi extension) and
`shims/claude/roger-mcp.mjs` (Claude Code MCP server).

## Checks

```sh
scripts/acceptance cross-host     # proof 1: mail survives a recipient SIGKILL across hosts
scripts/acceptance pi-shim        # Pi shim lifecycle and roger_send tool callback; no model
node --test shims/claude          # Claude shim protocol and lifecycle against a scripted adapter
scripts/acceptance independence   # proof 2: Pi and Claude Code sessions (plus a Tracer) exchange mail; loopback
scripts/check-harness-free        # invariant: no harness identity in the core
```

`scripts/acceptance independence` needs `pi`, `claude` (logged in) and `node`. It makes one
real Claude Code model call (haiku); Pi needs none. Claude's tool results are read from its
own stream-json output.

Delivery to a model is best effort and separate from the core's guarantee: the adapter's Store
is durable and acked before the shim is told, but what a shim has shown its harness is not. The
Claude shim keeps unshown Mail in memory for `roger_inbox`; if that process dies first, the Mail
is still in the Store and the receipt stands, and nothing replays it to the model. The Pi shim
writes each delivery into Pi's transcript.

`scripts/acceptance cross-host` runs the coordinator on this host's tailnet
address, one host daemon here and one on `$ROGER_REMOTE_HOST` (default
`agent-jail`), and requires SSH to that host plus `scp`. It builds release
binaries, then removes every process and directory it created.

## Layout

| Path | Role |
| --- | --- |
| `src/coordinator.rs` | central durable store; pushes committed mail |
| `src/daemon.rs` | one per host; local inbox; serves sessions on a Unix socket |
| `src/session.rs` | contract-shaped agent session (Tracer) and adapter; writes its Store, then acks |
| `shims/claude/roger-mcp.mjs` | Claude Code shim: `roger_send` and `roger_inbox` MCP tools over the adapter |
| `shims/pi/roger.ts` | Pi shim: renders mail into the transcript, forwards `/roger-send` and the `roger_send` tool |
| `src/durlog.rs` | append-only fsynced log; replay drops a crash-truncated tail |
| `src/json.rs`, `src/net.rs` | one-object-per-line codec |
| `tests/process.rs` | process seam, including the SIGKILL window |
