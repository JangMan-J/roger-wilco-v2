// The Pi user surface over the adapter: the /roger command (Board, picker,
// direct send, broadcast, relabel) and the roger_board, roger_label,
// roger_claim and roger_release tools. Everything goes through the adapter
// request lines that roger.ts owns; nothing here touches delivery or acks.
//
// Presence: the Board gives each session's last_seen_ms (its last register,
// send, ack or Board read). Which sessions count as recent is decided here,
// at read time, against ROGER_PRESENCE_WINDOW_MS (default 10 minutes).
// `/roger all` and roger_board {all: true} skip the filter.
import { Type } from "@earendil-works/pi-ai";
import { defineTool, type ExtensionAPI, type ExtensionContext } from "@earendil-works/pi-coding-agent";

type Reply = Record<string, unknown>;
export type Core = {
	me: () => string;
	request: (req: Reply) => Promise<Reply>;
	send: (to: string, body: string) => Promise<string>;
};
type Row = { session: string; host: string; label: string; last_seen_ms: number };
type ClaimRow = { key: string; holder?: string; remaining_ms?: number };
type Board = { sessions: Row[]; claims: ClaimRow[] };

const BROADCAST = "*";
const WINDOW_MS = Number(process.env.ROGER_PRESENCE_WINDOW_MS) || 10 * 60_000;
const USAGE =
	"Usage: /roger [all] | /roger <address|prefix|label|*> [message] | /roger label [address|prefix|label] <text> | /roger help";

const recent = (r: Row, now: number) => r.last_seen_ms > 0 && now - r.last_seen_ms <= WINDOW_MS;

const ago = (r: Row, now: number) =>
	r.last_seen_ms > 0
		? `last seen ${new Date(r.last_seen_ms).toISOString()} (${Math.max(0, Math.floor((now - r.last_seen_ms) / 1000))}s ago)`
		: "last seen unknown";

/** A target is `*`, an Address, a label one session shows, or a unique Address prefix (3+ chars). */
type Target = { to: string; row?: Row; rest: string };
function resolve(rows: Row[], text: string): Target | string {
	const first = text.split(/\s+/, 1)[0] ?? "";
	const after = (n: number) => text.slice(n).trim();
	if (first === BROADCAST) return { to: BROADCAST, rest: after(1) };
	const exact = rows.find((r) => r.session === first);
	if (exact) return { to: exact.session, row: exact, rest: after(first.length) };
	// A label may hold spaces: take the longest label the text starts with.
	// Labels are compared as typed text; nothing reads the integers in one.
	const fits = rows.filter((r) => r.label && (text === r.label || text.startsWith(r.label + " ")));
	const longest = Math.max(0, ...fits.map((r) => r.label.length));
	const byLabel = fits.filter((r) => r.label.length === longest);
	if (byLabel.length === 1) return { to: byLabel[0]!.session, row: byLabel[0], rest: after(longest) };
	if (byLabel.length > 1)
		return `ambiguous label ${JSON.stringify(byLabel[0]!.label)}: ${byLabel.map((r) => r.session).join(", ")}`;
	const byPrefix = first.length >= 3 ? rows.filter((r) => r.session.startsWith(first)) : [];
	if (byPrefix.length === 1) return { to: byPrefix[0]!.session, row: byPrefix[0], rest: after(first.length) };
	if (byPrefix.length > 1)
		return `ambiguous target ${JSON.stringify(first)}: ${byPrefix.map((r) => r.session).join(", ")}`;
	return `no session matches ${JSON.stringify(first)}`;
}

export function registerRogerCommand(pi: ExtensionAPI, core: Core): void {
	const board = async (): Promise<Board> => {
		const b = await core.request({ type: "board" });
		return { sessions: (b.sessions as Row[]) ?? [], claims: (b.claims as ClaimRow[]) ?? [] };
	};
	const view = (b: Board, all: boolean, now: number): Board => ({
		sessions: all ? b.sessions : b.sessions.filter((r) => recent(r, now)),
		claims: b.claims,
	});
	const relabel = async (label: string, session?: string) =>
		(await core.request({ type: "relabel", label, ...(session ? { session } : {}) })) as {
			session: string;
			label: string;
		};
	const rowText = (r: Row) =>
		`${r.session}${r.session === core.me() ? " (you)" : ""} — host ${r.host}${r.label ? ` — ${r.label}` : ""}`;
	const claimText = (c: ClaimRow) =>
		`${c.key} — ${c.holder ?? "?"}${c.remaining_ms !== undefined ? ` (${Math.ceil(c.remaining_ms / 1000)}s left)` : ""}`;

	const boardText = (full: Board, shown: Board, all: boolean, now: number) => {
		const own = full.sessions.find((r) => r.session === core.me());
		return [
			"Roger-Wilco",
			`Your address: ${core.me() || "(unknown: adapter not ready)"}`,
			`Your host: ${own ? own.host : "(not on the Board)"}`,
			"",
			`${all ? "All registered sessions" : `Recent sessions (seen within ${Math.round(WINDOW_MS / 1000)}s)`} (${shown.sessions.length}):`,
			...shown.sessions.map((r, i) => `${i + 1}. ${rowText(r)} — ${ago(r, now)}`),
			"",
			`Active claims (${shown.claims.length}):`,
			...shown.claims.map(claimText),
			"",
			"Presence is a session's last register, send, ack or Board read, not proof that it is alive.",
			"Labels are display only; the core routes by address.",
			...(all ? [] : ["Use /roger all to include stale sessions."]),
			"Pick a session to message it or set its label, or use /roger <address|prefix|label|*> <message>.",
		].join("\n");
	};

	const sendTo = async (ctx: ExtensionContext, to: string, title: string, body: string) => {
		if (!body && ctx.hasUI) body = (await ctx.ui.editor(title))?.trim() ?? "";
		if (!body) return ctx.ui.notify(USAGE, "warning");
		ctx.ui.notify(`accepted ${await core.send(to, body)} to ${to}`, "info");
	};

	pi.registerCommand("roger", {
		description: "Show the Board, message a session or broadcast, or set a label: /roger help",
		getArgumentCompletions: async (prefix: string) => {
			const words = prefix.split(" ");
			if (words.length > 2 || (words.length === 2 && words[0] !== "label")) return null;
			const last = words[words.length - 1]!;
			let rows: Row[] = [];
			try {
				rows = (await board()).sessions;
			} catch {
				// no adapter: complete keywords only
			}
			const targets = rows.flatMap((r) => [r.session, ...(r.label ? [r.label] : [])]).map((v) => v + " ");
			const values = words.length === 1 ? ["all", "help", "label ", BROADCAST + " ", ...targets] : targets;
			const lead = words.length === 2 ? "label " : "";
			const hits = [...new Set(values)].filter((v) => v.startsWith(last));
			return hits.length ? hits.map((v) => ({ value: lead + v, label: v.trim() })) : null;
		},
		handler: async (args, ctx) => {
			const text = args.trim();
			const words = text.split(/\s+/).filter(Boolean);
			try {
				if (words[0] === "help") return ctx.ui.notify(USAGE, "info");
				if (words[0] === "label") {
					// /roger label <text>                → this session
					// /roger label <target> <text>       → that session
					const rest = text.slice("label".length).trim();
					const hit = words.length > 2 ? resolve((await board()).sessions, rest) : undefined;
					const [session, label] =
						typeof hit === "object" && hit.row ? [hit.row.session, hit.rest] : [undefined, rest];
					if (!label) return ctx.ui.notify(USAGE, "warning");
					const r = await relabel(label, session);
					return ctx.ui.notify(`${r.session} is now ${r.label}`, "info");
				}
				if (words.length && words[0] !== "all") {
					const hit = resolve((await board()).sessions, text);
					if (typeof hit === "string") return ctx.ui.notify(hit, "error");
					const title = hit.row ? `Message to ${rowText(hit.row)}` : "Broadcast to every registered session";
					return await sendTo(ctx, hit.to, title, hit.rest);
				}
				const all = words[0] === "all";
				const now = Date.now();
				const full = await board();
				const shown = view(full, all, now);
				ctx.ui.notify(boardText(full, shown, all, now), "info");
				if (!ctx.hasUI) return;
				const viewOnly = "View only (no changes)";
				const broadcast = "Broadcast message…";
				const labels = shown.sessions.map(rowText);
				const picked = await ctx.ui.select("Choose a Roger session", [viewOnly, broadcast, ...labels]);
				if (picked === broadcast)
					return await sendTo(ctx, BROADCAST, "Broadcast to every registered session", "");
				const target = shown.sessions[labels.indexOf(picked ?? "")];
				if (!target) return;
				const message = `Message ${target.session}`;
				const setLabel = "Set label…";
				const action = await ctx.ui.select(`Action for ${rowText(target)}`, [message, setLabel, viewOnly]);
				if (action === message) return await sendTo(ctx, target.session, `Message to ${rowText(target)}`, "");
				if (action !== setLabel) return;
				const label = (await ctx.ui.input(`Label for ${target.session}`, target.label))?.trim();
				if (!label) return;
				const r = await relabel(label, target.session);
				ctx.ui.notify(`${r.session} is now ${r.label}`, "info");
			} catch (e) {
				ctx.ui.notify(`roger: ${(e as Error).message}`, "error");
			}
		},
	});

	const json = (value: unknown) => ({ content: [{ type: "text" as const, text: JSON.stringify(value) }], details: value });

	pi.registerTool(
		defineTool({
			name: "roger_board",
			label: "Roger board",
			description:
				"Read the Board: registered sessions (address, host, label, last_seen_ms) and active claims. " +
				`By default only sessions seen within ${Math.round(WINDOW_MS / 1000)}s; set all to include stale ones. ` +
				"Presence is a session's last activity, not proof that it is alive.",
			parameters: Type.Object({
				all: Type.Optional(Type.Boolean({ description: "include sessions not seen recently" })),
			}),
			async execute(_id, params) {
				return json({ me: core.me(), ...view(await board(), params.all ?? false, Date.now()) });
			},
		}),
	);

	pi.registerTool(
		defineTool({
			name: "roger_label",
			label: "Roger label",
			description: "Set this session's label, shown on the Board. Display only: the address does not change.",
			parameters: Type.Object({ label: Type.String({ description: "label text, stored verbatim" }) }),
			async execute(_id, params) {
				const r = await relabel(params.label);
				return { content: [{ type: "text", text: `${r.session} is now ${r.label}` }], details: r };
			},
		}),
	);

	pi.registerTool(
		defineTool({
			name: "roger_claim",
			label: "Roger claim",
			description:
				"Acquire, or renew if this session already holds it, an exclusive claim on a key for ttl_seconds " +
				"(default 900). ok is false when another holder has it; the reply names that holder.",
			parameters: Type.Object({
				key: Type.String({ description: "claim key, stored verbatim" }),
				ttl_seconds: Type.Optional(Type.Integer({ minimum: 1, description: "seconds until the claim expires" })),
			}),
			async execute(_id, params) {
				const ttl_ms = (params.ttl_seconds ?? 900) * 1000;
				return json(await core.request({ type: "claim", op: "acquire", key: params.key, ttl_ms }));
			},
		}),
	);

	pi.registerTool(
		defineTool({
			name: "roger_release",
			label: "Roger release",
			description: "Release a claim this session holds, by its exact key.",
			parameters: Type.Object({ key: Type.String({ description: "claim key, exactly as claimed" }) }),
			async execute(_id, params) {
				return json(await core.request({ type: "claim", op: "release", key: params.key }));
			},
		}),
	);
}
