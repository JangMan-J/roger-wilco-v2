# Claude Code shim

An MCP stdio server (`roger-mcp.mjs`, no dependencies) that runs `roger adapter` for each
Claude Code session and exposes `roger_send`, `roger_inbox`, `roger_board`, `roger_label`,
`roger_claim` and `roger_release`.

## Install

An optional module on top of the roger core, which [deploy/README.md](../../deploy/README.md)
installs. It needs, per host:

- the `roger` binary at `~/.local/bin/roger` (or on `PATH`)
- the host daemon listening on `$XDG_RUNTIME_DIR/roger/daemon.sock`
- `node` and a `claude` CLI on `PATH`

Installing or removing it changes nothing in the core or in any other shim.

```sh
shims/claude/install              # install or update, from this checkout
shims/claude/install --uninstall  # remove
```

Install copies `roger-mcp.mjs` to `~/.local/share/roger-shims/claude/` and registers it as
the user-scope MCP server `roger` (`claude mcp add -s user roger -- node <path>`), replacing
any earlier user-scope `roger` entry. Run it again to update. It installs for the current
user on the current host only; run it on each host.

Uninstall removes that registration and that directory. It keeps the Stores.

## At run time

Each Claude Code session starts its own copy of the shim, which runs
`roger adapter`. A session's Address is its Claude Code session id, so `--resume` keeps the
same Address and Store. Its Store is
`${XDG_STATE_HOME:-~/.local/state}/roger-shims/claude/<address>.store`.

Without `ROGER_SESSION` or a UUID in `CLAUDE_CODE_SESSION_ID` the Address is a fresh random
UUID that nothing can resume. On stdin EOF, SIGTERM or SIGINT the shim closes the adapter's
stdin, so the adapter unregisters its Address and the Board and later broadcasts drop it at
once, then waits for the adapter to exit (SIGKILL after 3 s, gives up after 5 s). It then
removes the default Store when it can hold nothing worth keeping: the Address was random, or
the Store is empty (no Mail was ever stored, and a resume recreates the same empty file). A
`claude mcp list` health check runs the shim under its own fresh session id (observed:
`CLAUDE_CODE_SESSION_ID` set, then SIGINT), stores nothing, and so leaves neither a
registration nor a Store. A Store holding Mail, or an explicit `ROGER_STORE`, stays.

Any non-empty environment variable overrides its default: `ROGER_SESSION`, `ROGER_SOCKET`,
`ROGER_BIN`, `ROGER_STORE`, `ROGER_DECLARE`, `ROGER_PRESENCE_WINDOW_MS`. To set one for
every session, pass `-e KEY=value` to `claude mcp add`. The header of `roger-mcp.mjs`
describes each one.

## Delivery

The shim does not wake and declares no wake latency; `ROGER_DECLARE=<text>` passes opaque
text as `--declare` (unset declares nothing). Delivered Mail waits in memory until the model
calls `roger_inbox`; if the shim dies, the Mail is still in the Store, and Mail the adapter
already acked is not shown again.

## Checks

```sh
shims/claude/check                # protocol and lifecycle against a scripted adapter; no claude binary
scripts/acceptance independence   # proof 2: this shim is one of the two harnesses
```

`scripts/acceptance independence` needs `claude` (logged in) and `node`, and makes one real
Claude Code model call (haiku). Claude's tool results are read from its own stream-json
output.
