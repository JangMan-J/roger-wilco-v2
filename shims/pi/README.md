# Pi shim

A Pi extension (`roger.ts`) that runs `roger adapter` for each Pi session, renders each
delivered Mail into Pi's transcript, and adds `/roger-send`, `/roger` and the `roger_send`
tool.

## Install

An optional module on top of the roger core, which [deploy/README.md](../../deploy/README.md)
installs. It needs, per host:

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
`${XDG_STATE_HOME:-~/.local/state}/roger-shims/pi/<address>.store`. The core holds a Store
under an exclusive lock, so a second Pi process on the same session cannot run a second
adapter on it: that adapter fails at start naming the Store, and the shim reports its exit
(`roger shim: roger adapter exited (…)` on stderr; a send then fails with that reason). On
`session_shutdown` the shim closes the adapter's stdin, so the adapter unregisters its
Address, and waits for it to exit (SIGKILL after 3 s) before a new session starts the next
one.

Any non-empty environment variable overrides its default: `ROGER_SESSION`, `ROGER_SOCKET`,
`ROGER_BIN`, `ROGER_STORE`, `ROGER_WAKE_LATENCY_MS`, `ROGER_PRESENCE_WINDOW_MS`. The header
of `roger.ts` describes each one.

## Delivery and wake

The shim writes each delivery into Pi's transcript. It declares `wake-latency-ms=<n>`
through `--declare` (`ROGER_WAKE_LATENCY_MS`, default 1000; `off` declares nothing and never
wakes). A delivery with `wake: true` goes to Pi as
`sendMessage(..., { triggerTurn: true, deliverAs: "followUp" })`: an idle Pi starts a turn, a
busy Pi queues it and hands it to the model at the turn boundary. A `wake: false` delivery
(a broadcast copy) is rendered with `triggerTurn: false`.

## Checks

```sh
shims/pi/check                    # unit tests under test/; no pi binary needed
scripts/acceptance pi-shim        # lifecycle and the roger_send tool callback in a real Pi; no model
scripts/acceptance wake           # proof 5: directed mail wakes an idle Pi; busy queues to the turn boundary
scripts/acceptance independence   # proof 2: this shim is one of the two harnesses
```

`scripts/acceptance wake` drives a scripted faux model (`test/faux-model.ts`), so it needs
`pi` but makes no model call. In `scripts/acceptance independence` Pi runs in RPC mode with
wake off and makes no model call.
