// Test support, not shipped behaviour: loads the real Pi shim and adds one
// command that invokes the shim's registered roger_send tool callback
// directly, so the tool path runs in a real Pi runtime without a model call.
// It skips Pi's tool dispatch (argument validation, tool_call hooks).
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import shim from "../roger.ts";

export default function (pi: ExtensionAPI) {
	let tool: any;
	shim(
		new Proxy(pi, {
			get(target, key) {
				if (key === "registerTool")
					return (def: any) => {
						if (def.name === "roger_send") tool = def;
						return target.registerTool(def);
					};
				const v = (target as any)[key];
				return typeof v === "function" ? v.bind(target) : v;
			},
		}),
	);
	pi.registerCommand("call-roger-send", {
		description: "Invoke the roger_send tool callback: /call-roger-send <address> <body>",
		handler: async (args, ctx) => {
			const [to] = args.trim().split(/\s+/);
			const body = args.trim().slice(to.length).trim();
			try {
				const r = await tool.execute("direct", { to, body }, undefined, undefined, ctx);
				ctx.ui.notify(`tool result: ${r.content[0].text}`, "info");
			} catch (e) {
				ctx.ui.notify(`tool failed: ${(e as Error).message}`, "error");
			}
		},
	});
}
