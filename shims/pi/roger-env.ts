// Shim defaults and the Store lock for the Pi shim. Pure where it can be, so
// node --test shims/pi/test/roger-env.test.ts covers it without Pi.
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, unlinkSync, writeSync } from "node:fs";
import { dirname, join } from "node:path";

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

const alive = (pid: number) => {
	try {
		process.kill(pid, 0);
		return true;
	} catch (e) {
		return (e as NodeJS.ErrnoException).code === "EPERM";
	}
};

/**
 * Take `<store>.lock` for this process, so a second Pi process on the same
 * session never starts a second adapter on the same Store: two adapters on one
 * Store would each number records from the same replay and write duplicate
 * record ids. A lock whose pid is
 * gone is stale and taken over. Returns a release function, or throws naming
 * the live holder.
 */
export function lockStore(store: string, pid = process.pid): () => void {
	const path = `${store}.lock`;
	mkdirSync(dirname(path), { recursive: true });
	for (let tries = 0; tries < 2; tries++) {
		try {
			const fd = openSync(path, "wx", 0o600);
			writeSync(fd, `${pid}\n`);
			closeSync(fd);
			return () => {
				try {
					if (readFileSync(path, "utf8").trim() === String(pid)) unlinkSync(path);
				} catch {
					// already gone
				}
			};
		} catch (e) {
			if ((e as NodeJS.ErrnoException).code !== "EEXIST") throw e;
			let holder = NaN;
			try {
				holder = Number(readFileSync(path, "utf8").trim());
			} catch {
				continue; // removed between open and read: try again
			}
			if (Number.isInteger(holder) && holder > 0 && alive(holder))
				throw new Error(`Store ${store} is held by pid ${holder} (${path})`);
			try {
				unlinkSync(path); // stale: its holder is gone
			} catch {
				// another taker removed it first
			}
		}
	}
	throw new Error(`could not take ${path}`);
}
