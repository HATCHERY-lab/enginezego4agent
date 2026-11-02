// Reference host for pi-durable 1.5.0. The key is read from E4A_KEY_FILE into
// the process environment and never written to the log, argv, or stderr.
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { createModels } from "@earendil-works/pi-ai/models";
import { openrouterProvider } from "@earendil-works/pi-ai/providers/openrouter";
import { createRegistry, defineExtension, Harness, UsageDoc } from "@earendil-works/pi-durable";
import { NodeExecutionEnv } from "@earendil-works/pi-durable/env/node";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";
import { createPowerShellTool, createReadTool } from "@earendil-works/pi-durable/tools";
import { appendFileSync, readFileSync } from "node:fs";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const tree = dirname(here);
const context = BACKGROUND_CONTEXT;
const scenario = process.env.E4A_SCENARIO || "turn";
const modelId = process.env.E4A_MODEL;
const effort = process.env.E4A_EFFORT || "off";
const db = process.env.E4A_DB;
const logPath = process.env.E4A_LOG;
const cwd = process.env.E4A_CWD || process.cwd();
const key = readFileSync(process.env.E4A_KEY_FILE, "utf8").trim();
if (!key) {
	appendFileSync(process.env.E4A_LOG, "{\"kind\":\"error\",\"value\":\"empty key file\"}\n");
	process.exit(1);
}
process.env[["OPENROUTER", "API", "KEY"].join("_")] = key;

const envName = ["OPENROUTER", "API", "KEY"].join("_");
const scrub = (value) => {
	const text = typeof value === "string" ? value : JSON.stringify(value, (_k, v) => (typeof v === "bigint" ? String(v) : v));
	const win = tree.replaceAll("/", "\\");
	const fwd = win.replaceAll("\\", "/");
	const two = win.replaceAll("\\", "\\\\");
	const four = win.replaceAll("\\", "\\\\\\\\");
	return text
		.split(key).join("[REDACTED]")
		.replace(/Bearer\s+\S+/g, "Bearer [REDACTED]")
		.replace(new RegExp(envName + "\\s*[:=]\\s*\\S+", "g"), envName + "=[REDACTED]")
		.split(four).join("<TREE>").split(two).join("<TREE>").split(win).join("<TREE>").split(fwd).join("<TREE>");
};
const log = (kind, value) => appendFileSync(logPath, scrub({ t: Date.now(), kind, value }) + "\n");

if (process.env.E4A_SSE) {
	const orig = globalThis.fetch;
	const ssePath = process.env.E4A_SSE;
	const hold = 512;
	let pending = "";
	const push = (chunk, final) => {
		pending += chunk;
		const keep = final ? 0 : hold;
		if (pending.length <= keep) return;
		const emit = pending.slice(0, pending.length - keep);
		pending = pending.slice(pending.length - keep);
		appendFileSync(ssePath, scrub(emit));
	};
	globalThis.fetch = async (input, init) => {
		const res = await orig(input, init);
		const type = res.headers.get("content-type") || "";
		if (!type.includes("text/event-stream") || !res.body) return res;
		const [a, b] = res.body.tee();
		b.pipeTo(new WritableStream({
			write(chunk) {
				push(Buffer.from(chunk).toString("utf8"), false);
			},
			close() {
				push("", true);
			},
		})).catch(() => {});
		return new Response(a, { status: res.status, statusText: res.statusText, headers: res.headers });
	};
}

const cleanEnv = {
	SystemRoot: process.env.SystemRoot || "C:\\Windows",
	PATH: process.env.PATH || "",
	PATHEXT: process.env.PATHEXT || "",
	COMSPEC: process.env.COMSPEC || "",
};
const readTool = createReadTool();
const psTool = createPowerShellTool({
	prepare(execution) {
		execution.inheritEnv = false;
		execution.env = cleanEnv;
	},
});
const toolFor = { none: [], read: [readTool], powershell: [psTool] };
const tools = toolFor[process.env.E4A_TOOLS || "none"];
const models = createModels();
models.setProvider(openrouterProvider());
const registry = createRegistry();
registry.install(defineExtension({ name: "e4a-tools", tools: [readTool, psTool] }));

const agent = { model: { provider: "openrouter", modelId }, thinkingLevel: effort, tools, cwd };
const storage = await openNodeSqliteStorage(db);
const harness = await Harness.open(storage, {
	models,
	registry,
	env: (target) => new NodeExecutionEnv({ cwd: target.cwd || cwd }),
}, context);
harness.observeCommits({
	advance(publication) { log("commit", publication); },
	end(end) { log("commits-end", end); },
});

const root = await harness.root(context, { agent });
let abortArmed = scenario === "abort";
const attach = async (conversation, label) => {
	const watch = await conversation.watch(context);
	watch.start(async (call) => {
		const value = call.kind === "value" ? call.value : { kind: call.kind };
		log("watch", { label, value });
		if (call.kind !== "value") return;
		const live = call.value.docs?.["pi.live"];
		const generation = live?.generation;
		if (generation?.message) log("mark", { label, kind: "assistant" });
		for (const slot of live?.tools || []) {
			if (slot.status === "running") log("mark", { label, kind: "tool", name: slot.name });
		}
		if (abortArmed && generation?.message) {
			abortArmed = false;
			setImmediate(() => root.abort(context).catch((error) => log("error", error?.message || error)));
		}
	});
};
await attach(root, "root");

const ask = async (conversation, content, requestId) => {
	const submission = await conversation.submit({ type: "input", content, requestId }, context);
	const settled = await submission.wait(context);
	log("settled", { id: submission.id, requestId, settled });
	return settled;
};

try {
	if (scenario === "fork") {
		const first = await ask(root, process.env.E4A_PROMPT, process.env.E4A_REQ);
		if (first.status !== "done") throw new Error("fork parent " + first.status);
		const view = await root.context(context);
		const at = view.entries.at(-1)?.id;
		if (!at) throw new Error("fork has no entry");
		log("fork-at", at);
		const child = await root.fork(at, { ownership: { kind: "ownerless" } }, context);
		await attach(child, "child");
		await ask(child, process.env.E4A_PROMPT2, process.env.E4A_REQ + "-child");
	} else if (scenario === "switch") {
		const nextModel = process.env.E4A_MODEL2 || "";
		if (!nextModel || nextModel === modelId) throw new Error("switch needs a second model");
		await ask(root, process.env.E4A_PROMPT, process.env.E4A_REQ);
		await root.configure({
			model: { provider: "openrouter", modelId: nextModel },
			thinkingLevel: process.env.E4A_EFFORT2 || "low",
		}, context);
		log("configured", { model: nextModel, effort: process.env.E4A_EFFORT2 || "low" });
		await ask(root, process.env.E4A_PROMPT2, process.env.E4A_REQ + "-2");
	} else {
		await ask(root, process.env.E4A_PROMPT, process.env.E4A_REQ);
	}
	log("usage", await harness.snapshot(UsageDoc, root.id, context));
	await harness.close(context);
} catch (error) {
	log("error", error?.stack || error?.message || error);
	try { await harness.close(context); } catch {}
	process.exitCode = 1;
}
