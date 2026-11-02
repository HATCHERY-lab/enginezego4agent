// Drives the E0 reference scenarios. The key stays in the host process.
// This file prints only scenario names, model ids, statuses, and counts.
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const tree = dirname(here);
const work = join(here, "work");
const fixtures = join(tree, "fixtures");
const keyFile = process.env.E4A_KEY_FILE;
if (!keyFile) {
	console.log("FAILED key-file-unset");
	process.exit(1);
}
const key = readFileSync(keyFile, "utf8").trim();
if (!key) {
	console.log("FAILED empty-key-file");
	process.exit(1);
}

const FREE = [
	"thinkingmachines/inkling-small:free",
	"thinkingmachines/inkling:free",
	"cohere/north-mini-code:free",
	"recursive/flint-v1.0:free",
	"liquid/lfm-2.5-2.6b:free",
	"nvidia/nemotron-3.5-lightning:free",
	"google/gemma-4-26b-a4b-it:free",
	"google/gemma-4-31b-it:free",
	"xingchen-agi/xing4.0-29b-a4b:free",
	"dots-studio/dots-3-note-preview:free",
];
const PAID = "inclusionai/ling-3.0-flash";

const KEEP = [
	"PATH", "PATHEXT", "SystemRoot", "COMSPEC", "TEMP", "TMP",
	"HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY",
	"http_proxy", "https_proxy", "no_proxy",
];

const envName = ["OPENROUTER", "API", "KEY"].join("_");

function replaceRoot(text) {
	const win = tree.replaceAll("/", "\\");
	const fwd = win.replaceAll("\\", "/");
	const two = win.replaceAll("\\", "\\\\");
	const four = win.replaceAll("\\", "\\\\\\\\");
	return text.split(four).join("<TREE>").split(two).join("<TREE>").split(win).join("<TREE>").split(fwd).join("<TREE>");
}

const scrub = (text) => replaceRoot(text
	.split(key).join("[REDACTED]")
	.replace(/Bearer\s+\S+/g, "Bearer [REDACTED]")
	.replace(new RegExp(envName + "\\s*[:=]\\s*\\S+", "g"), envName + "=[REDACTED]"));

const childEnv = (extra) => {
	const env = { E4A_KEY_FILE: keyFile };
	for (const name of KEEP) {
		if (process.env[name] && name !== envName) env[name] = process.env[name];
	}
	return { ...env, ...extra };
};

const slug = (value) => value.replace(/[^a-z0-9]+/gi, "_").replace(/^_|_$/g, "");

function killPid(pid) {
	spawn("taskkill.exe", ["/F", "/T", "/PID", String(pid)], { stdio: "ignore", windowsHide: true });
}

function runHost(extra, { timeoutMs, killOn }) {
	return new Promise((resolve) => {
		const child = spawn(process.execPath, ["host.mjs"], {
			cwd: here,
			env: childEnv(extra),
			stdio: ["ignore", "ignore", "pipe"],
			windowsHide: true,
		});
		let stderr = "";
		let settled = false;
		const finish = (result) => {
			if (settled) return;
			settled = true;
			clearTimeout(timer);
			if (poll) clearInterval(poll);
			const errPath = extra.E4A_LOG + ".err";
			writeFileSync(errPath, scrub(stderr).slice(0, 200_000));
			resolve(result);
		};
		child.stderr.on("data", (chunk) => {
			if (stderr.length < 200_000) stderr += chunk.toString("utf8");
		});
		const timer = setTimeout(() => {
			killPid(child.pid);
			finish({ code: null, killed: "timeout" });
		}, timeoutMs);
		let poll;
		if (killOn) {
			poll = setInterval(() => {
				try {
					const text = readFileSync(extra.E4A_LOG, "utf8");
					if (text.includes(killOn)) {
						clearInterval(poll);
						poll = undefined;
						killPid(child.pid);
						finish({ code: null, killed: "mark" });
					}
				} catch {}
			}, 250);
		}
		child.on("exit", (code) => finish({ code, killed: null }));
		child.on("error", () => finish({ code: null, killed: "spawn" }));
	});
}

function readLog(path) {
	return existsSync(path) ? scrub(readFileSync(path, "utf8")) : "";
}

function settledStatuses(text) {
	const found = [];
	for (const line of text.split("\n")) {
		if (!line.includes("\"kind\":\"settled\"") && !line.includes("\"kind\": \"settled\"")) continue;
		const match = line.match(/"status":"([^"]+)"/);
		if (match) found.push(match[1]);
	}
	return found;
}

function hasError(text) {
	return text.includes("\"kind\":\"error\"") || text.includes("\"kind\": \"error\"");
}

function pathForms(abs) {
	if (!abs) return [];
	const win = String(abs).replaceAll("/", "\\");
	return [win.replaceAll("\\", "\\\\\\\\"), win.replaceAll("\\", "\\\\"), win, win.replaceAll("\\", "/")];
}

function leakKind(text) {
	const hit = (values) => values.some((value) => value && value.length >= 3 && text.includes(value));
	if (hit(pathForms(tree))) return "tree";
	if (hit(pathForms(process.env.USERPROFILE))) return "profile";
	const user = process.env.USERNAME || "";
	if (user.length >= 3 && text.includes(user)) return "user";
	const prefix = key.slice(0, 12);
	if (prefix.length >= 12 && text.includes(prefix)) return "prefix";
	return "";
}

async function once(name, env, opts) {
	mkdirSync(dirname(env.E4A_LOG), { recursive: true });
	const result = await runHost(env, opts);
	const text = readLog(env.E4A_LOG);
	return { name, ...result, statuses: settledStatuses(text), error: hasError(text), bytes: text.length };
}

function saveFixture(name, logPath, ssePath) {
	const logText = readLog(logPath);
	const sseText = ssePath && existsSync(ssePath) ? readLog(ssePath) : "";
	const kind = leakKind(logText + "\n" + sseText);
	if (kind) {
		console.log(`SCENARIO ${name} pass=false leak=${kind}`);
		return false;
	}
	mkdirSync(fixtures, { recursive: true });
	writeFileSync(join(fixtures, `${name}.jsonl`), logText);
	if (ssePath && existsSync(ssePath)) writeFileSync(join(fixtures, `${name}.sse`), sseText);
	return true;
}

async function probe(model) {
	const dir = join(work, "probe", slug(model));
	mkdirSync(dir, { recursive: true });
	const log = join(dir, "run.jsonl");
	writeFileSync(log, "");
	const result = await once("probe", {
		E4A_SCENARIO: "turn",
		E4A_MODEL: model,
		E4A_EFFORT: "off",
		E4A_DB: join(dir, "session.sqlite"),
		E4A_LOG: log,
		E4A_CWD: dir,
		E4A_TOOLS: "none",
		E4A_PROMPT: "Reply with exactly the word pong.",
		E4A_REQ: "probe",
	}, { timeoutMs: 70_000 });
	const ok = result.statuses.includes("done") && !result.killed;
	console.log(`PROBE ${model} ok=${ok} code=${result.code} killed=${result.killed || "-"} statuses=${result.statuses.join(",") || "-"}`);
	return ok;
}

async function pickModels(needSecond) {
	const known = new Map();
	const works = async (model) => {
		if (known.has(model)) return known.get(model);
		const ok = await probe(model);
		known.set(model, ok);
		return ok;
	};
	let model = process.env.E4A_MODEL || "";
	if (!model) {
		for (const candidate of FREE) {
			if (await works(candidate)) {
				model = candidate;
				break;
			}
		}
		if (!model && process.env.E4A_ALLOW_PAID === "1" && await works(PAID)) model = PAID;
	}
	if (!model) return null;
	if (!needSecond) return { model, model2: "" };
	let model2 = process.env.E4A_MODEL2 || "";
	if (!model2 || model2 === model) {
		model2 = "";
		for (const candidate of FREE) {
			if (candidate === model) continue;
			if (await works(candidate)) {
				model2 = candidate;
				break;
			}
		}
	}
	if (!model2 || model2 === model) return { model, model2: "" };
	return { model, model2 };
}

function scenarioDir(name) {
	const dir = join(work, name);
	mkdirSync(dir, { recursive: true });
	return dir;
}

async function runTurn(spec) {
	const dir = scenarioDir(spec.name);
	for (const name of ["session.sqlite", "session.sqlite-wal", "session.sqlite-shm"]) {
		rmSync(join(dir, name), { force: true });
	}
	const log = join(dir, "run.jsonl");
	const sse = spec.sse ? join(dir, "provider.sse") : "";
	writeFileSync(log, "");
	if (sse) writeFileSync(sse, "");
	if (spec.note) writeFileSync(join(dir, "note.txt"), spec.note);
	const env = {
		E4A_SCENARIO: spec.scenario,
		E4A_MODEL: spec.model,
		E4A_EFFORT: spec.effort || "off",
		E4A_DB: join(dir, "session.sqlite"),
		E4A_LOG: log,
		E4A_CWD: dir,
		E4A_TOOLS: spec.tools,
		E4A_PROMPT: spec.prompt,
		E4A_PROMPT2: spec.prompt2 || "",
		E4A_REQ: spec.name,
		E4A_MODEL2: spec.model2 || "",
		E4A_EFFORT2: spec.effort2 || "",
		E4A_SSE: sse,
	};
	const first = await once(spec.name, env, { timeoutMs: spec.timeoutMs, killOn: spec.killOn });
	let second = null;
	if (spec.killOn) {
		writeFileSync(log, readLog(log) + JSON.stringify({ t: Date.now(), kind: "driver", value: { event: "restart", killed: first.killed, code: first.code } }) + "\n");
		second = await once(spec.name + "-resume", { ...env, E4A_SCENARIO: "turn", E4A_SSE: "" }, { timeoutMs: spec.timeoutMs });
	}
	if (!saveFixture(spec.name, log, sse)) return false;
	const text = readLog(log);
	const statuses = settledStatuses(text);
	const pass = spec.judge(first, second, text, statuses);
	console.log(`SCENARIO ${spec.name} pass=${pass} killed=${first.killed || "-"} code=${first.code} resume=${second ? second.code : "-"} statuses=${statuses.join(",") || "-"}`);
	return pass;
}

const judgeDone = (_first, _second, _text, statuses) => statuses.includes("done");
const judgeKill = (first, second, text, statuses) => first.killed === "mark" && second?.code === 0 && statuses.includes("done") && text.includes("\"event\":\"restart\"");
const judgeAbort = (_first, second, text, statuses) => text.includes("\"kind\":\"assistant\"") && (statuses.includes("unanswered") || text.includes("aborted"));
const judgeFork = (_first, _second, text, statuses) => text.includes("\"kind\":\"fork-at\"") && statuses.filter((s) => s === "done").length >= 2;

async function main() {
	mkdirSync(work, { recursive: true });
	const only = process.argv[2];
	const needSecond = !only || only === "all" || only === "switch";
	const picked = await pickModels(needSecond);
	if (!picked) {
		console.log("FAILED no-model");
		process.exit(1);
	}
	const { model } = picked;
	const model2 = picked.model2;
	if (needSecond && !model2) {
		console.log("FAILED no-model");
		process.exit(1);
	}
	console.log(`MODEL ${model}`);
	if (model2) console.log(`MODEL2 ${model2}`);
	const judgeSwitch = (_first, _second, text, statuses) => text.includes("\"kind\":\"configured\"")
		&& statuses.filter((s) => s === "done").length >= 2
		&& model !== model2
		&& text.includes(model)
		&& text.includes(model2);
	const specs = [
		{ name: "qa", scenario: "turn", model, tools: "none", sse: true, timeoutMs: 120_000, prompt: "Reply with exactly the word pong.", judge: judgeDone },
		{ name: "read", scenario: "turn", model, tools: "read", sse: true, timeoutMs: 150_000, note: "e4a-note-ok\n", prompt: "Use the read tool on note.txt. Reply with only the file contents.", judge: (a, b, text, statuses) => statuses.includes("done") && text.includes("e4a-note-ok") },
		{ name: "kill-stream", scenario: "turn", model, tools: "none", timeoutMs: 180_000, killOn: "\"kind\":\"assistant\"", prompt: "Write four long paragraphs about rivers. Do not stop early.", judge: judgeKill },
		{ name: "kill-tool", scenario: "turn", model, tools: "powershell", timeoutMs: 180_000, killOn: "\"kind\":\"tool\"", prompt: "Call the powershell tool once with this exact command and then reply slept: Start-Sleep -Seconds 20; Write-Output slept", judge: judgeKill },
		{ name: "fork", scenario: "fork", model, tools: "none", timeoutMs: 180_000, prompt: "Reply with exactly the word alpha.", prompt2: "Reply with exactly the word beta.", judge: judgeFork },
		{ name: "abort", scenario: "abort", model, tools: "none", timeoutMs: 120_000, prompt: "Write four long paragraphs about mountains. Do not stop early.", judge: judgeAbort },
		{ name: "switch", scenario: "switch", model, model2, tools: "none", timeoutMs: 180_000, effort: "off", effort2: "low", prompt: "Reply with exactly the word one.", prompt2: "Reply with exactly the word two.", judge: judgeSwitch },
		{ name: "powershell", scenario: "turn", model, tools: "powershell", timeoutMs: 150_000, prompt: "Call the powershell tool once with this exact command and then stop: Write-Output e4a-ps-ok", judge: (a, b, text, statuses) => statuses.includes("done") && text.includes("e4a-ps-ok") },
	];
	const selected = only && only !== "all" ? specs.filter((spec) => spec.name === only) : specs;
	let failed = 0;
	for (const spec of selected) {
		const pass = await runTurn(spec);
		if (!pass) failed += 1;
	}
	console.log(failed ? `FAILED scenarios=${failed}` : "DONE");
	process.exit(failed ? 1 : 0);
}

main().catch((error) => {
	console.log(`FAILED driver ${scrub(String(error?.message || error)).slice(0, 200)}`);
	process.exit(1);
});
