// Shim-level tests: drive roger-mcp.mjs over MCP stdio against a scripted
// stand-in for `roger adapter`. This tests the shim's own lifecycle and
// protocol handling; it is not harness evidence (scripts/acceptance
// independence runs the real Claude Code). Run: node --test shims/claude
import { spawn } from "node:child_process";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import assert from "node:assert/strict";
import { createInterface } from "node:readline";

const SHIM = new URL("./roger-mcp.mjs", import.meta.url).pathname;
const MAIL = '{"type":"mail","mail":"m-1","from":"f","body":"hello","record":"r-1"}';

// Start the shim with a stand-in adapter (a shell script, or a missing path).
function start(script) {
	const dir = mkdtempSync(join(tmpdir(), "roger-shim-"));
	let bin = join(dir, "nonexistent");
	if (script) {
		bin = join(dir, "adapter");
		writeFileSync(bin, `#!/bin/sh\n${script}\n`);
		chmodSync(bin, 0o755);
	}
	const child = spawn("node", [SHIM], {
		env: { ...process.env, ROGER_BIN: bin, ROGER_SOCKET: "s", ROGER_STORE: "t" },
		stdio: ["pipe", "pipe", "pipe"],
	});
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
	return { child, rpc, reply, exited, stderr: () => stderr, lines, cleanup: () => (child.kill(), rmSync(dir, { recursive: true })) };
}

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

test("stdin EOF ends the shim with status 0", async () => {
	const s = start("cat >/dev/null");
	s.child.stdin.end();
	assert.equal(await within(5000, s.exited), 0);
});
