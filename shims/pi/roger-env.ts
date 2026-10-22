// Shim defaults for the Pi shim. Pure, so node --test shims/pi/test/roger-env.test.ts
// covers it without Pi.
import { existsSync } from "node:fs";
import { join } from "node:path";

export type Env = Record<string, string | undefined>;
export type Settings = { socket: string; bin: string; session: string; store: string };

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/**
 * The adapter settings: an explicit, non-empty env value wins, then the
 * defaults. `sessionId` is Pi's own session id, used as the Address when it is
 * a UUID, so a resumed session keeps its Address and Store; else `fresh()`.
 */
export function settings(env: Env, sessionId: string | undefined, home: string, fresh: () => string, exists = existsSync): Settings {
	const get = (k: string) => env[k] || undefined; // empty counts as unset
	const runtime = get("XDG_RUNTIME_DIR");
	const socket = get("ROGER_SOCKET") ?? (runtime ? join(runtime, "roger", "daemon.sock") : undefined);
	if (!socket) throw new Error("ROGER_SOCKET is unset and XDG_RUNTIME_DIR is unset: no host daemon socket to use");
	const installed = join(home, ".local", "bin", "roger");
	const bin = get("ROGER_BIN") ?? (exists(installed) ? installed : "roger");
	const session = get("ROGER_SESSION") ?? (sessionId && UUID.test(sessionId) ? sessionId : fresh());
	const state = get("XDG_STATE_HOME") ?? join(home, ".local", "state");
	const store = get("ROGER_STORE") ?? join(state, "roger-shims", "pi", `${session}.store`);
	return { socket, bin, session, store };
}
