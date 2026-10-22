# Claude Code shim: install

An optional module on top of the roger core. It needs, per host:

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

Any non-empty environment variable overrides its default: `ROGER_SESSION`, `ROGER_SOCKET`,
`ROGER_BIN`, `ROGER_STORE`, `ROGER_DECLARE`, `ROGER_PRESENCE_WINDOW_MS`. To set one for
every session, pass `-e KEY=value` to `claude mcp add`. The header of `roger-mcp.mjs`
describes each one.
