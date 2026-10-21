// Claude Code shim: an MCP stdio server, the one place that knows the
// harness is Claude Code. It starts the harness-free `roger adapter`,
// forwards tool calls (send, Board, relabel, claim, release) as request
// lines, and hands delivered Mail to the model through `roger_inbox`. It
// never acknowledges a delivery: the adapter acks from its own Store. No
// dependencies; MCP over stdio is one JSON object per line.
//
// Presence: the Board says whether each session is connected (its session
// connection and its host daemon's link are open) and gives last_seen_ms (the
// read time while connected, else its disconnect or last own activity). The
// default view is decided here, at read time: connected sessions plus any seen
// within ROGER_PRESENCE_WINDOW_MS (default 10 minutes); {all: true} skips the
// filter. Board rows pass through verbatim.
//
// Env: ROGER_SOCKET, ROGER_STORE (required), ROGER_SESSION (Address), ROGER_BIN,
// ROGER_DECLARE (opaque text the session states about itself at Register,
// passed verbatim as `--declare`; unset declares nothing). This shim cannot
// start a turn, so it never declares a wake latency.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

const socket = process.env.ROGER_SOCKET;
const store = process.env.ROGER_STORE;
if (!socket || !store) {
	console.error("ROGER_SOCKET and ROGER_STORE are required");
	process.exit(1);
}
function windowMs(env) {
	const n = env === undefined || env.trim() === "" ? NaN : Number(env);
	return Number.isInteger(n) && n >= 0 ? n : 10 * 60_000;
}
const WINDOW_MS = windowMs(process.env.ROGER_PRESENCE_WINDOW_MS);
const args = ["adapter", "--socket", socket, "--store", store];
if (process.env.ROGER_SESSION) args.push("--id", process.env.ROGER_SESSION);
if (process.env.ROGER_DECLARE) args.push("--declare", process.env.ROGER_DECLARE);
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

const pending = []; // requests awaiting their reply, in order
// The adapter answers each request line exactly once, in order; `ready` and
// `mail` are its only unsolicited lines, so every other line is the oldest
// pending request's answer.
let me = process.env.ROGER_SESSION ?? ""; // the Address, from the adapter's ready line
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
	if (ev.type === "ready") me = ev.session;
	else if (ev.type === "mail") {
		if (seen.has(ev.record)) return;
		seen.add(ev.record);
		inbox.push({ mail: ev.mail, from: ev.from, body: ev.body });
		wakeAll();
	} else if (ev.type === "error") pending.shift()?.fail(new Error(ev.error));
	else pending.shift()?.ok(ev);
});

// One request line to the adapter; replies come back in request order.
const request = (req) =>
	new Promise((ok, fail) => {
		if (dead) return fail(dead);
		pending.push({ ok, fail });
		adapter.stdin.write(JSON.stringify(req) + "\n");
	});
const ask = async (req, want) => {
	const r = await request(req);
	if (r.type !== want) throw new Error(`expected ${want}, saw ${JSON.stringify(r)}`);
	return r;
};
const send = async (to, body) => {
	const r = await ask({ type: "send", to, body }, "accepted");
	if (typeof r.mail !== "string") throw new Error(`expected accepted with a mail id, saw ${JSON.stringify(r)}`);
	return r.mail;
};

const present = (r, now) => r.connected || (r.last_seen_ms > 0 && now - r.last_seen_ms <= WINDOW_MS);
const board = async (all) => {
	const b = await ask({ type: "board" }, "board");
	const now = typeof b.now_ms === "number" ? b.now_ms : Date.now();
	const sessions = b.sessions ?? [];
	return { me, sessions: all ? sessions : sessions.filter((r) => present(r, now)), claims: b.claims ?? [], now };
};

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
	{
		name: "roger_board",
		description:
			"Read the Board: registered sessions (address, host, label, connected, last_seen_ms) and active claims. " +
			`By default only sessions connected or seen within ${Math.round(WINDOW_MS / 1000)}s; set all to include stale ones. ` +
			"Connected means the session's connection is open, not that it is working.",
		inputSchema: {
			type: "object",
			properties: { all: { type: "boolean", description: "include sessions neither connected nor seen recently" } },
		},
	},
	{
		name: "roger_label",
		description: "Set this session's label, shown on the Board. Display only: the address does not change.",
		inputSchema: {
			type: "object",
			properties: { label: { type: "string", description: "label text, stored verbatim" } },
			required: ["label"],
		},
	},
	{
		name: "roger_claim",
		description:
			"Acquire, or renew if this session already holds it, an exclusive claim on a key for ttl_seconds " +
			"(default 900). ok is false when another holder has it; the reply names that holder.",
		inputSchema: {
			type: "object",
			properties: {
				key: { type: "string", description: "claim key, stored verbatim" },
				ttl_seconds: { type: "integer", minimum: 1, description: "seconds until the claim expires" },
			},
			required: ["key"],
		},
	},
	{
		name: "roger_release",
		description: "Release a claim this session holds, by its exact key.",
		inputSchema: {
			type: "object",
			properties: { key: { type: "string", description: "claim key, exactly as claimed" } },
			required: ["key"],
		},
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
	if (name === "roger_board") return JSON.stringify(await board(a.all ?? false));
	if (name === "roger_label") {
		const r = await ask({ type: "relabel", label: a.label }, "relabeled");
		return `${r.session} is now ${r.label}`;
	}
	if (name === "roger_claim") {
		const ttl_ms = (a.ttl_seconds ?? 900) * 1000;
		return JSON.stringify(await ask({ type: "claim", op: "acquire", key: a.key, ttl_ms }, "claim_result"));
	}
	if (name === "roger_release")
		return JSON.stringify(await ask({ type: "claim", op: "release", key: a.key }, "claim_result"));
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
