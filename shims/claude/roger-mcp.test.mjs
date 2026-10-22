// Shim-level tests: drive roger-mcp.mjs over MCP stdio against a scripted
// stand-in for `roger adapter`. This tests the shim's own lifecycle and
// protocol handling; it is not harness evidence (scripts/acceptance
// independence runs the real Claude Code). Run: node --test shims/claude
import { spawn } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import assert from "node:assert/strict";
import { createInterface } from "node:readline";

const SHIM = new URL("./roger-mcp.mjs", import.meta.url).pathname;
const MAIL = '{"type":"mail","mail":"m-1","from":"f","body":"hello","record":"r-1"}';

// Every shim and temp dir start() makes, killed and removed after the file's
// tests whether or not a test reached its own cleanup (a failed assertion or an
// exit test skips it); a live shim left behind would keep the run from ending.
const children = [];
const dirs = [];
after(() => {
	children.forEach((c) => c.kill());
	dirs.forEach((d) => rmSync(d, { recursive: true, force: true }));
});

// Start the shim with a stand-in adapter (a shell script, or a missing path).
// An env value of undefined removes that variable. With home, the stand-in is
// installed at $HOME/.local/bin/roger under a temp HOME and ROGER_BIN, ROGER_SOCKET,
// ROGER_STORE, ROGER_SESSION and XDG_STATE_HOME start unset, so the shim's defaults apply.
function start(script, env = {}, { home = false } = {}) {
	const dir = mkdtempSync(join(tmpdir(), "roger-shim-"));
	dirs.push(dir);
	let bin = join(dir, "nonexistent");
	if (script) {
		bin = join(dir, "adapter");
		writeFileSync(bin, `#!/bin/sh\n${script}\n`);
		chmodSync(bin, 0o755);
	}
	const base = { ...process.env, ROGER_BIN: bin, ROGER_SOCKET: "s", ROGER_STORE: "t" };
	delete base.CLAUDE_CODE_SESSION_ID; // the test runner may itself be a Claude Code child
	if (home) {
		mkdirSync(join(dir, ".local", "bin"), { recursive: true });
		copyFileSync(bin, join(dir, ".local", "bin", "roger"));
		chmodSync(join(dir, ".local", "bin", "roger"), 0o755);
		Object.assign(base, { HOME: dir, ROGER_BIN: undefined, ROGER_SOCKET: undefined, ROGER_STORE: undefined, ROGER_SESSION: undefined, XDG_STATE_HOME: undefined });
	}
	const merged = { ...base, ...env };
	for (const k of Object.keys(merged)) if (merged[k] === undefined) delete merged[k];
	const child = spawn("node", [SHIM], { env: merged, stdio: ["pipe", "pipe", "pipe"] });
	children.push(child);
	const lines = [];
	const waiters = [];
	createInterface({ input: child.stdout }).on("line", (l) => {
		const v = JSON.parse(l);
		lines.push(v);
		waiters.splice(0).forEach((w) => w());
	});
	let stderr = "";
	child.stderr.on("data", (d) => (stderr += d));
	const exited = new Promise((r) => child.on("exit", (code) => r(code)));
	const reply = async (id) => {
		for (;;) {
			const hit = lines.find((v) => v.id === id);
			if (hit) return hit;
			await new Promise((r) => waiters.push(r));
		}
	};
	const rpc = (id, method, params) => child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
	const call = async (id, name, args) => {
		rpc(id, "tools/call", { name, arguments: args });
		const r = (await within(5000, reply(id))).result;
		return { text: r.content[0].text, isError: r.isError === true };
	};
	// Request lines the scripted adapter wrote to $dir/requests (see `logged`).
	const requests = () => (existsSync(join(dir, "requests")) ? readFileSync(join(dir, "requests"), "utf8") : "").split("\n").filter(Boolean).map((l) => JSON.parse(l));
	return { dir, child, rpc, reply, call, requests, exited, stderr: () => stderr, lines, cleanup: () => (child.kill(), rmSync(dir, { recursive: true, force: true })) };
}

// Shell for a scripted adapter: log one request line, then answer it.
const logged = (...replies) =>
	replies.map((r) => `read -r line; printf '%s\\n' "$line" >>"$(dirname "$0")/requests"; echo '${r}'`).join("\n");
const READY = '{"type":"ready","session":"me-1"}';
const now = Date.now();
const BOARD = JSON.stringify({
	type: "board",
	req: 1,
	sessions: [
		{ session: "me-1", host: "h1", label: "", connected: true, last_seen_ms: now },
		{ session: "old-1", host: "h2", label: "stale [[0:FLEET:0]]", connected: false, last_seen_ms: now - 3_600_000 },
		{ session: "quiet-1", host: "h2", label: "", connected: true, last_seen_ms: now - 3_600_000, extra: "kept" },
	],
	claims: [{ key: "k", holder: "me-1", remaining_ms: 5000 }],
	now_ms: now,
});
const CLAIMED = '{"type":"claim_result","req":1,"op":"acquire","key":"k","ok":true,"holder":"me-1","remaining_ms":900000}';
const RELEASED = '{"type":"claim_result","req":1,"op":"release","key":"k","ok":true}';

const within = (ms, p) =>
	Promise.race([p, new Promise((_, rej) => setTimeout(() => rej(new Error(`not settled in ${ms} ms`)), ms))]);

test("ping gets an empty result (MCP utilities/ping)", async () => {
	const s = start("cat >/dev/null");
	s.rpc(1, "ping");
	assert.deepEqual((await within(3000, s.reply(1))).result, {});
	s.cleanup();
});

test("a malformed line gets a parse error and the shim keeps serving", async () => {
	const s = start("cat >/dev/null");
	s.child.stdin.write("not json\n");
	s.rpc(2, "ping");
	await within(3000, s.reply(2));
	assert.equal(s.lines.find((v) => v.id === null).error.code, -32700);
	s.cleanup();
});

test("a missing adapter binary ends the shim nonzero and names the cause", async () => {
	const s = start(null);
	assert.equal(await within(5000, s.exited), 1);
	assert.match(s.stderr(), /ENOENT/);
});

test("adapter death fails the pending send and ends the shim nonzero", async () => {
	const s = start("read line; exit 3");
	s.rpc(3, "tools/call", { name: "roger_send", arguments: { to: "a", body: "b" } });
	const r = await within(5000, s.reply(3));
	assert.equal(r.result.isError, true);
	assert.match(r.result.content[0].text, /exited \(3\)/);
	assert.equal(await within(5000, s.exited), 1);
});

test("adapter death releases a waiting roger_inbox with an error", async () => {
	const s = start("sleep 0.3; exit 4");
	s.rpc(4, "tools/call", { name: "roger_inbox", arguments: {} });
	const r = await within(5000, s.reply(4));
	assert.equal(r.result.isError, true);
	assert.equal(await within(5000, s.exited), 1);
});

test("two concurrent roger_inbox calls both settle; the Mail is shown once", async () => {
	const s = start(`sleep 0.3; echo '${MAIL}'; cat >/dev/null`);
	s.rpc(5, "tools/call", { name: "roger_inbox", arguments: {} });
	s.rpc(6, "tools/call", { name: "roger_inbox", arguments: {} });
	const [a, b] = await within(3000, Promise.all([s.reply(5), s.reply(6)]));
	const texts = [a, b].map((r) => r.result.content[0].text);
	assert.equal(texts.filter((t) => t.includes("hello")).length, 1, texts.join("|"));
	s.cleanup();
});

test("tools/list names every tool", async () => {
	const s = start("cat >/dev/null");
	s.rpc(7, "tools/list");
	const names = (await within(3000, s.reply(7))).result.tools.map((t) => t.name);
	assert.deepEqual(names, ["roger_send", "roger_inbox", "roger_board", "roger_label", "roger_claim", "roger_release"]);
	s.cleanup();
});

test("ROGER_DECLARE is passed verbatim as --declare; unset declares nothing", async () => {
	// Log each argv entry as a JSON string so requests() can read it.
	const argv = "for a; do printf '\"%s\"\\n' \"$a\"; done >\"$(dirname \"$0\")/requests\"; cat >/dev/null";
	const s = start(argv);
	s.rpc(8, "ping");
	await within(3000, s.reply(8));
	assert.ok(!s.requests().includes("--declare"), s.requests().join(" "));
	s.cleanup();
	const d = start(argv, { ROGER_DECLARE: "x=1 y" });
	d.rpc(9, "ping");
	await within(3000, d.reply(9));
	const args = d.requests();
	assert.deepEqual(args.slice(args.indexOf("--declare")), ["--declare", "x=1 y"]);
	d.cleanup();
});

test("roger_board keeps connected or recent sessions by default, all:true keeps every row verbatim; me comes from ready", async () => {
	const s = start(`echo '${READY}'\n${logged(BOARD, BOARD)}\ncat >/dev/null`);
	const a = JSON.parse((await s.call(10, "roger_board", {})).text);
	assert.equal(a.me, "me-1");
	assert.equal(a.now, now);
	assert.deepEqual(a.sessions.map((r) => r.session), ["me-1", "quiet-1"]);
	assert.deepEqual(a.claims, [{ key: "k", holder: "me-1", remaining_ms: 5000 }]);
	const b = JSON.parse((await s.call(11, "roger_board", { all: true })).text);
	assert.deepEqual(b.sessions, JSON.parse(BOARD).sessions);
	assert.deepEqual(s.requests(), [{ type: "board" }, { type: "board" }]);
	s.cleanup();
});

test("roger_label relabels this session and reports the new label", async () => {
	const s = start(`${logged('{"type":"relabeled","req":1,"session":"me-1","label":"Meteor Shower"}')}\ncat >/dev/null`);
	assert.equal((await s.call(12, "roger_label", { label: "Meteor Shower" })).text, "me-1 is now Meteor Shower");
	assert.deepEqual(s.requests(), [{ type: "relabel", label: "Meteor Shower" }]);
	s.cleanup();
});

test("roger_claim acquires with ttl_seconds (default 900) and roger_release releases; replies pair in request order", async () => {
	// Two requests in flight before any reply: the first reply answers the first call.
	const s = start(`${logged(CLAIMED, CLAIMED.replace("900000", "5000"), RELEASED)}\ncat >/dev/null`);
	const [a, b] = await Promise.all([s.call(13, "roger_claim", { key: "k" }), s.call(14, "roger_claim", { key: "k", ttl_seconds: 5 })]);
	assert.equal(JSON.parse(a.text).remaining_ms, 900000);
	assert.equal(JSON.parse(b.text).remaining_ms, 5000);
	const c = JSON.parse((await s.call(15, "roger_release", { key: "k" })).text);
	assert.deepEqual(c, { type: "claim_result", req: 1, op: "release", key: "k", ok: true });
	assert.deepEqual(s.requests(), [
		{ type: "claim", op: "acquire", key: "k", ttl_ms: 900000 },
		{ type: "claim", op: "acquire", key: "k", ttl_ms: 5000 },
		{ type: "claim", op: "release", key: "k" },
	]);
	s.cleanup();
});

test("an adapter error reply is a tool error carrying the adapter's text", async () => {
	const s = start(`${logged('{"type":"error","error":"unknown request relabel"}')}\ncat >/dev/null`);
	const r = await s.call(16, "roger_label", { label: "x" });
	assert.equal(r.isError, true);
	assert.equal(r.text, "unknown request relabel");
	s.cleanup();
});

test("adapter death fails the pending roger_claim and ends the shim nonzero", async () => {
	const s = start("read line; exit 5");
	const r = await s.call(17, "roger_claim", { key: "k" });
	assert.equal(r.isError, true);
	assert.match(r.text, /exited \(5\)/);
	assert.equal(await within(5000, s.exited), 1);
});

test("stdin EOF ends the shim with status 0", async () => {
	const s = start("cat >/dev/null");
	s.child.stdin.end();
	assert.equal(await within(5000, s.exited), 0);
});

// The stand-in logs its argv as JSON strings; the logged file sits beside it
// (the temp dir for ROGER_BIN, or $HOME/.local/bin for the installed default).
const ARGV = "for a; do printf '\"%s\"\\n' \"$a\"; done >\"$(dirname \"$0\")/argv\"; cat >/dev/null";
const argvOf = async (s, id) => {
	s.rpc(id, "ping");
	await within(3000, s.reply(id));
	for (const f of [join(s.dir, "argv"), join(s.dir, ".local", "bin", "argv")])
		if (existsSync(f)) return readFileSync(f, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l));
	throw new Error(`no argv logged; stderr: ${s.stderr()}`);
};
const opt = (argv, flag) => argv[argv.indexOf(flag) + 1];
const SID = "bcbf4c5e-c888-4317-8d21-cadd16846dc1";

test("defaults: the harness session id is the Address; socket, installed bin and Store follow it", async () => {
	const s = start(ARGV, { CLAUDE_CODE_SESSION_ID: SID, XDG_RUNTIME_DIR: "/run/x" }, { home: true });
	const argv = await argvOf(s, 20);
	assert.equal(argv[0], "adapter");
	assert.equal(opt(argv, "--id"), SID);
	assert.equal(opt(argv, "--socket"), "/run/x/roger/daemon.sock");
	assert.equal(opt(argv, "--store"), join(s.dir, ".local", "state", "roger-shims", "claude", `${SID}.store`));
	assert.ok(existsSync(join(s.dir, ".local", "state", "roger-shims", "claude")), "Store directory created");
	s.cleanup();
});

test("XDG_STATE_HOME moves the default Store; empty counts as unset", async () => {
	const state = mkdtempSync(join(tmpdir(), "roger-state-"));
	dirs.push(state);
	const s = start(ARGV, { CLAUDE_CODE_SESSION_ID: SID, XDG_RUNTIME_DIR: "/run/x", XDG_STATE_HOME: state }, { home: true });
	assert.equal(opt(await argvOf(s, 24), "--store"), join(state, "roger-shims", "claude", `${SID}.store`));
	assert.ok(existsSync(join(state, "roger-shims", "claude")), "Store directory created");
	s.cleanup();
	const e = start(ARGV, { CLAUDE_CODE_SESSION_ID: SID, XDG_RUNTIME_DIR: "/run/x", XDG_STATE_HOME: "" }, { home: true });
	assert.equal(opt(await argvOf(e, 25), "--store"), join(e.dir, ".local", "state", "roger-shims", "claude", `${SID}.store`));
	e.cleanup();
});

test("a harness session id that is not a UUID is ignored: each process gets a fresh UUID", async () => {
	const ids = [];
	for (const id of [21, 22]) {
		const s = start(ARGV, { CLAUDE_CODE_SESSION_ID: "not-a-uuid", XDG_RUNTIME_DIR: "/run/x" }, { home: true });
		const argv = await argvOf(s, id);
		ids.push(opt(argv, "--id"));
		assert.equal(opt(argv, "--store"), join(s.dir, ".local", "state", "roger-shims", "claude", `${opt(argv, "--id")}.store`));
		s.cleanup();
	}
	for (const id of ids) assert.match(id, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
	assert.notEqual(ids[0], ids[1]);
});

test("explicit env wins over every default", async () => {
	const s = start(ARGV, { CLAUDE_CODE_SESSION_ID: SID, ROGER_SESSION: "explicit-id", ROGER_SOCKET: "/sock", ROGER_STORE: "store-path" });
	const argv = await argvOf(s, 23);
	assert.equal(opt(argv, "--id"), "explicit-id");
	assert.equal(opt(argv, "--socket"), "/sock");
	assert.equal(opt(argv, "--store"), "store-path");
	s.cleanup();
});

test("no ROGER_SOCKET and no XDG_RUNTIME_DIR ends the shim nonzero and says so", async () => {
	const s = start(ARGV, { XDG_RUNTIME_DIR: undefined }, { home: true });
	assert.equal(await within(5000, s.exited), 1);
	assert.match(s.stderr(), /XDG_RUNTIME_DIR/);
});

// A stand-in that creates its --store file, as the real adapter does, then
// reads stdin to EOF and leaves an `eof` marker beside itself before exiting.
const STORE_THEN_EOF =
	'while [ $# -gt 0 ]; do [ "$1" = --store ] && store="$2"; shift; done; : >"$store"; cat >/dev/null; : >"$(dirname "$0")/eof"';
const storeOf = async (s, id) => {
	s.rpc(id, "ping");
	await within(3000, s.reply(id));
	const store = join(s.dir, ".local", "state", "roger-shims", "claude");
	for (let i = 0; i < 100; i++) {
		const f = readdirSync(store).find((n) => n.endsWith(".store"));
		if (f) return join(store, f);
		await new Promise((r) => setTimeout(r, 20));
	}
	throw new Error(`no Store created; stderr: ${s.stderr()}`);
};

test("a random Address leaves no Store once stdin closes and the adapter has exited", async () => {
	const s = start(STORE_THEN_EOF, { XDG_RUNTIME_DIR: "/run/x" }, { home: true });
	const store = await storeOf(s, 30);
	s.child.stdin.end();
	assert.equal(await within(5000, s.exited), 0);
	assert.ok(existsSync(join(s.dir, ".local", "bin", "eof")), "adapter saw stdin EOF");
	assert.equal(existsSync(store), false, `${store} must be removed`);
});

test("a ROGER_SESSION or harness-session Address keeps its Store at shutdown", async () => {
	for (const [id, env] of [
		[31, { ROGER_SESSION: "keep-id" }],
		[32, { CLAUDE_CODE_SESSION_ID: SID }],
	]) {
		const s = start(STORE_THEN_EOF, { XDG_RUNTIME_DIR: "/run/x", ...env }, { home: true });
		const store = await storeOf(s, id);
		assert.equal(store.endsWith(`${env.ROGER_SESSION ?? SID}.store`), true, store);
		s.child.stdin.end();
		assert.equal(await within(5000, s.exited), 0);
		assert.ok(existsSync(store), `${store} must stay`);
	}
});

test("SIGTERM and SIGINT close the adapter's stdin and wait for its exit", async () => {
	for (const [id, sig, code] of [
		[33, "SIGTERM", 143],
		[34, "SIGINT", 130],
	]) {
		const s = start("cat >/dev/null; sleep 0.2; : >\"$(dirname \"$0\")/eof\"");
		s.rpc(id, "ping");
		await within(3000, s.reply(id));
		s.child.kill(sig);
		assert.equal(await within(5000, s.exited), code);
		assert.ok(existsSync(join(s.dir, "eof")), `${sig}: adapter saw stdin EOF before the shim exited`);
	}
});
