# Pi shim: install

An optional module on top of the roger core. It needs, per host:

- the `roger` binary at `~/.local/bin/roger` (or on `PATH`)
- the host daemon listening on `$XDG_RUNTIME_DIR/roger/daemon.sock`
- `node` and a `pi` CLI on `PATH`

Installing or removing it changes nothing in the core or in any other shim.

```sh
shims/pi/install              # install or update, from this checkout
shims/pi/install --uninstall  # remove
```

Install copies `roger.ts`, `roger-command.ts`, `roger-target.ts`, `roger-env.ts` and
`package.json` to `~/.local/share/roger-shims/pi/` and lists that directory once in the
`packages` array of Pi's `settings.json` (`${PI_CODING_AGENT_DIR:-~/.pi/agent}`), keeping
every other setting. Pi loads it as a local package: nothing is fetched, and Pi itself
provides `@earendil-works/pi-ai` and `@earendil-works/pi-coding-agent`. Run it again to
update; new Pi sessions pick up the copy. It installs for the current user on the current
host only; run it on each host.

Uninstall removes that entry and that directory. It keeps the Stores.

## At run time

Each Pi session starts its own `roger adapter`. A session's Address is its Pi session id
when that is a UUID (the default), so resuming keeps the same Address and Store; `/new` and
`/fork` give a new Address. Its Store is
`${XDG_STATE_HOME:-~/.local/state}/roger-shims/pi/<address>.store`. A `<store>.lock` file
stops a second Pi process from running a second adapter on the same Store.

Any non-empty environment variable overrides its default: `ROGER_SESSION`, `ROGER_SOCKET`,
`ROGER_BIN`, `ROGER_STORE`, `ROGER_WAKE_LATENCY_MS`, `ROGER_PRESENCE_WINDOW_MS`. The header
of `roger.ts` describes each one.
