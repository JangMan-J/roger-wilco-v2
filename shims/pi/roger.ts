// Pi shim: the one place that knows Pi runs this session. It starts the
// harness-free `roger adapter`, renders each delivered Mail into the
// transcript without starting a turn, and forwards send calls as lines.
// It never acknowledges a delivery: the adapter acks from its own Store.
//
// Env: ROGER_SOCKET (host daemon socket) and ROGER_STORE (Store path) are
// required; ROGER_SESSION is the Address (adapter generates one if unset);
// ROGER_BIN is the adapter binary (default `roger` on PATH).
import { spawn, type ChildProcess } from "node:child_process";
import { createInterface } from "node:readline";
import { Type } from "@earendil-works/pi-ai";
import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
	let adapter: ChildProcess | undefined;
	const seen = new Set<string>(); // a redelivery after a crash shares its record id
	const pending: { ok: (m: string) => void; fail: (e: Error) => void }[] = [];

	const start = () => {
		const socket = process.env.ROGER_SOCKET;
		const store = process.env.ROGER_STORE;
		if (!socket || !store) throw new Error("ROGER_SOCKET and ROGER_STORE are required");
		const args = ["adapter", "--socket", socket, "--store", store];
		if (process.env.ROGER_SESSION) args.push("--id", process.env.ROGER_SESSION);
		const child = spawn(process.env.ROGER_BIN ?? "roger", args, { stdio: ["pipe", "pipe", "inherit"] });
		adapter = child;
		createInterface({ input: child.stdout! }).on("line", (line) => {
			const ev = JSON.parse(line);
			if (ev.type === "ready") return;
			else if (ev.type === "accepted") pending.shift()?.ok(ev.mail);
			else if (ev.type === "error") pending.shift()?.fail(new Error(ev.error));
			else if (ev.type === "mail" && !seen.has(ev.record)) {
				seen.add(ev.record);
				pi.sendMessage(
					{
						customType: "roger-mail",
						content: `Mail ${ev.mail} from ${ev.from}:\n${ev.body}`,
						display: true,
						details: { mail: ev.mail, from: ev.from, record: ev.record },
					},
					{ triggerTurn: false },
				);
			}
		});
	};

	const send = (to: string, body: string) =>
		new Promise<string>((ok, fail) => {
			if (!adapter?.stdin?.writable) return fail(new Error("roger adapter is not running"));
			pending.push({ ok, fail });
			adapter.stdin.write(JSON.stringify({ type: "send", to, body }) + "\n");
		});

	pi.on("session_start", async () => start());
	pi.on("session_shutdown", async () => {
		adapter?.stdin?.end(); // adapter exits when its stdin closes
		adapter = undefined;
	});

	pi.registerCommand("roger-send", {
		description: "Send Mail: /roger-send <address> <body>",
		handler: async (args, ctx) => {
			const [to] = args.trim().split(/\s+/);
			const body = args.trim().slice(to?.length ?? 0).trim();
			if (!to || !body) return ctx.ui.notify("Usage: /roger-send <address> <body>", "warning");
			const mail = await send(to, body);
			ctx.ui.notify(`accepted ${mail}`, "info");
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
