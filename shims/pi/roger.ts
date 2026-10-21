// Pi shim: the one place that knows Pi runs this session. It starts the
// harness-free `roger adapter`, renders each delivered Mail into the
// transcript, and forwards send calls as lines. It never acknowledges a
// delivery: the adapter acks from its own Store.
//
// Wake: a delivered Mail that carries a wake request (every directed Mail;
// a Broadcast sets `wake: false`) starts a turn when Pi is idle, and when Pi
// is busy it is queued as a follow-up that Pi hands the model at the turn
// boundary. Pi decides idle or busy; the core never learns either. The shim
// declares its wake latency to the Board as `wake-latency-ms=<n>`.
//
// Env: ROGER_SOCKET (host daemon socket) and ROGER_STORE (Store path) are
// required; ROGER_SESSION is the Address (adapter generates one if unset);
// ROGER_BIN is the adapter binary (default `roger` on PATH);
// ROGER_WAKE_LATENCY_MS is the declared wake latency (default 1000), or `off`
// to render Mail without ever starting a turn and declare nothing.
import { spawn, type ChildProcess } from "node:child_process";
import { createInterface } from "node:readline";
import { Type } from "@earendil-works/pi-ai";
import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";

type Pending = { ok: (mail: string) => void; fail: (e: Error) => void };

/** The declared wake latency in ms, or undefined when wake is off. */
export function wakeLatency(env: string | undefined): number | undefined {
	if (env === "off") return undefined;
	if (env === undefined || env === "") return 1000;
	const n = Number(env);
	if (!Number.isInteger(n) || n <= 0) throw new Error(`ROGER_WAKE_LATENCY_MS must be a positive integer or off, saw ${env}`);
	return n;
}

export default function (pi: ExtensionAPI) {
	const wakeMs = wakeLatency(process.env.ROGER_WAKE_LATENCY_MS);
	let adapter: ChildProcess | undefined;
	let why = "roger adapter is not running"; // why the last adapter is gone
	const seen = new Set<string>(); // a redelivery after a crash shares its record id
	const pending: Pending[] = []; // send calls awaiting accepted/error, in order

	const failAll = (e: Error) => {
		for (const p of pending.splice(0)) p.fail(e);
	};

	const start = () => {
		// session_start recurs (new, resume, fork, reload) while this process
		// lives; a second adapter would be a second writer of the same Store.
		if (adapter) return;
		const socket = process.env.ROGER_SOCKET;
		const store = process.env.ROGER_STORE;
		if (!socket || !store) throw new Error("ROGER_SOCKET and ROGER_STORE are required");
		const args = ["adapter", "--socket", socket, "--store", store];
		if (process.env.ROGER_SESSION) args.push("--id", process.env.ROGER_SESSION);
		if (wakeMs !== undefined) args.push("--declare", `wake-latency-ms=${wakeMs}`);
		const child = spawn(process.env.ROGER_BIN ?? "roger", args, { stdio: ["pipe", "pipe", "inherit"] });
		adapter = child;
		// 'error' (spawn failure) and 'exit' both end this adapter; an unhandled
		// 'error' would crash the harness, and a pending send would hang.
		const gone = (e: Error) => {
			if (adapter !== child) return;
			adapter = undefined;
			why = e.message;
			process.stderr.write(`roger shim: ${e.message}\n`);
			failAll(e);
		};
		child.on("error", (e) => gone(e));
		child.on("exit", (code, sig) => gone(new Error(`roger adapter exited (${code ?? sig})`)));
		child.stdin!.on("error", () => {}); // the exit/error handler reports it
		createInterface({ input: child.stdout! }).on("line", (line) => {
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
				const wake = wakeMs !== undefined && ev.wake !== false;
				// Idle: triggerTurn starts a turn now. Busy: followUp waits for the
				// current turn to end, then Pi continues with this Mail.
				pi.sendMessage(
					{
						customType: "roger-mail",
						content: `Mail ${ev.mail} from ${ev.from}:\n${ev.body}`,
						display: true,
						details: { mail: ev.mail, from: ev.from, record: ev.record },
					},
					wake ? { triggerTurn: true, deliverAs: "followUp" } : { triggerTurn: false },
				);
			}
		});
	};

	const send = (to: string, body: string) =>
		new Promise<string>((ok, fail) => {
			if (!adapter?.stdin?.writable) return fail(new Error(why));
			pending.push({ ok, fail });
			adapter.stdin.write(JSON.stringify({ type: "send", to, body }) + "\n");
		});

	pi.on("session_start", async () => start());
	pi.on("session_shutdown", async () => {
		const child = adapter;
		adapter = undefined;
		why = "roger shim shut down";
		failAll(new Error(why));
		if (!child || child.exitCode !== null) return;
		// The adapter exits when its stdin closes. Wait for it, so a session
		// started right after never overlaps it on the same Store.
		await new Promise<void>((done) => {
			const t = setTimeout(() => (child.kill(), done()), 3000);
			child.once("exit", () => (clearTimeout(t), done()));
			child.stdin?.end();
		});
	});

	pi.registerCommand("roger-send", {
		description: "Send Mail: /roger-send <address> <body>",
		handler: async (args, ctx) => {
			const [to] = args.trim().split(/\s+/);
			const body = args.trim().slice(to?.length ?? 0).trim();
			if (!to || !body) return ctx.ui.notify("Usage: /roger-send <address> <body>", "warning");
			try {
				ctx.ui.notify(`accepted ${await send(to, body)}`, "info");
			} catch (e) {
				ctx.ui.notify(`roger-send failed: ${(e as Error).message}`, "error");
			}
		},
	});

	pi.registerTool(
		defineTool({
			name: "roger_send",
			label: "Roger send",
			description: "Send Mail to another agent session by Address.",
			parameters: Type.Object({
				to: Type.String({ description: "recipient Address (UUID)" }),
				body: Type.String({ description: "mail body, delivered verbatim" }),
			}),
			async execute(_id, params) {
				const mail = await send(params.to, params.body);
				return { content: [{ type: "text", text: `accepted ${mail}` }], details: { mail } };
			},
		}),
	);
}
