// Claude Code shim: an MCP stdio server, the one place that knows the
// harness is Claude Code. It starts the harness-free `roger adapter`,
// forwards `roger_send` calls as lines, and hands delivered Mail to the model
// through `roger_inbox`. It never acknowledges a delivery: the adapter acks
// from its own Store. No dependencies; MCP over stdio is one JSON object per line.
//
// Env: ROGER_SOCKET, ROGER_STORE (required), ROGER_SESSION (Address), ROGER_BIN.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

const socket = process.env.ROGER_SOCKET;
const store = process.env.ROGER_STORE;
if (!socket || !store) {
	console.error("ROGER_SOCKET and ROGER_STORE are required");
	process.exit(1);
}
const args = ["adapter", "--socket", socket, "--store", store];
if (process.env.ROGER_SESSION) args.push("--id", process.env.ROGER_SESSION);
const adapter = spawn(process.env.ROGER_BIN ?? "roger", args, { stdio: ["pipe", "pipe", "inherit"] });
let closing = false; // our own stdin ended: the adapter's exit is expected
let dead = null; // why the adapter is gone, once it is
const gone = (e) => {
	if (dead) return;
	dead = e;
	if (closing) process.exit(0);
	console.error(`roger shim: ${e.message}`);
	for (const p of pending.splice(0)) p.fail(e);
	wakeAll();
	process.exitCode = 1;
	// Replies already queued on stdout still flush before the process ends.
	setImmediate(() => process.exit(1));
};
adapter.on("error", gone);
adapter.on("exit", (code, sig) => gone(new Error(`roger adapter exited (${code ?? sig})`)));
adapter.stdin.on("error", () => {}); // the exit/error handler reports it

const pending = []; // send calls awaiting accepted/error, in order
const inbox = []; // delivered Mail not yet shown to the model (in memory: the durable copy is the adapter's Store)
const seen = new Set(); // a redelivery after a crash shares its record id
const waiters = new Set(); // inbox calls waiting for Mail

const wakeAll = () => {
	for (const w of [...waiters]) w();
};

createInterface({ input: adapter.stdout }).on("line", (line) => {
	let ev;
	try {
		ev = JSON.parse(line);
	} catch {
		return; // not an adapter event; nothing to correlate it with
	}
	if (ev.type === "accepted") pending.shift()?.ok(ev.mail);
	else if (ev.type === "error") pending.shift()?.fail(new Error(ev.error));
	else if (ev.type === "mail" && !seen.has(ev.record)) {
		seen.add(ev.record);
		inbox.push({ mail: ev.mail, from: ev.from, body: ev.body });
		wakeAll();
	}
});

const send = (to, body) =>
	new Promise((ok, fail) => {
		if (dead) return fail(dead);
		pending.push({ ok, fail });
		adapter.stdin.write(JSON.stringify({ type: "send", to, body }) + "\n");
	});

// Wait once, until any Mail arrives, the adapter dies, or 10 s pass.
const waitForMail = () =>
	new Promise((resolve) => {
		const done = () => (clearTimeout(timer), waiters.delete(done), resolve());
		const timer = setTimeout(done, 10000);
		waiters.add(done);
	});

const tools = [
	{
		name: "roger_send",
		description: "Send Mail to another agent session by Address.",
		inputSchema: {
			type: "object",
			properties: {
				to: { type: "string", description: "recipient Address (UUID)" },
				body: { type: "string", description: "mail body, delivered verbatim" },
			},
			required: ["to", "body"],
		},
	},
	{
		name: "roger_inbox",
		description:
			"Return Mail delivered to this session and not yet shown. Waits up to 10 seconds for Mail if there is none.",
		inputSchema: { type: "object", properties: {} },
	},
];

async function call(name, a) {
	if (name === "roger_send") return `accepted ${await send(a.to, a.body)}`;
	if (name === "roger_inbox") {
		if (inbox.length === 0 && !dead) await waitForMail();
		if (inbox.length === 0 && dead) throw dead;
		const out = inbox.splice(0).map((m) => `Mail ${m.mail} from ${m.from}:\n${m.body}`);
		return out.length ? out.join("\n\n") : "no mail";
	}
	throw new Error(`unknown tool ${name}`);
}

const reply = (id, result, error) =>
	process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id, ...(error ? { error } : { result }) }) + "\n");

createInterface({ input: process.stdin }).on("line", async (line) => {
	let m;
	try {
		m = JSON.parse(line);
	} catch {
		return reply(null, undefined, { code: -32700, message: "parse error" });
	}
	if (m === null || typeof m !== "object" || Array.isArray(m))
		return reply(null, undefined, { code: -32600, message: "invalid request" });
	if (m.id === undefined) return; // notifications need no reply
	try {
		if (m.method === "initialize")
			reply(m.id, {
				protocolVersion: m.params?.protocolVersion ?? "2025-06-18",
				capabilities: { tools: {} },
				serverInfo: { name: "roger", version: "0.1.0" },
			});
		else if (m.method === "ping") reply(m.id, {});
		else if (m.method === "tools/list") reply(m.id, { tools });
		else if (m.method === "tools/call") {
			try {
				const text = await call(m.params.name, m.params.arguments ?? {});
				reply(m.id, { content: [{ type: "text", text }] });
			} catch (e) {
				reply(m.id, { content: [{ type: "text", text: String(e.message) }], isError: true });
			}
		} else reply(m.id, undefined, { code: -32601, message: "method not found" });
	} catch (e) {
		reply(m.id, undefined, { code: -32603, message: String(e.message) });
	}
});
process.stdin.on("end", () => {
	closing = true;
	adapter.stdin.end();
});
