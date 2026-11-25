// Wires a pi session to its agent's IO, MindIO: takes in the inbox at each step, wakes the agent
// as it is woken, and holds the awake lock while a run lasts and the asleep lock while an
// interactive session is open. A new session is first given `mindio brief`.
// The agent's IO is MINDIO where pi was started with one, else ~/.agents/mindio/pi-<session id>,
// made at the session's start, which then gets a `resume` that brings the session back where it
// has none; the resume finds this extension installed where pi loads it for every session.
// A wake with the file `fresh` in the agent's IO starts a new session, a fresh mind's, where the
// agent is idle; a busy agent takes it in its run, so the file only goes.

import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import {
	existsSync,
	mkdirSync,
	renameSync,
	rmSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import type {
	ExtensionAPI,
	ExtensionContext,
} from "@earendil-works/pi-coding-agent";

const customType = "mindio";

const quote = (s: string) => `'${s.replaceAll("'", "'\\''")}'`;

export default function (pi: ExtensionAPI) {
	let mindio = process.env.MINDIO;
	// What a new session is given first: the brief.
	let brief: string | undefined;
	// The session's context, current across a replacement of the session.
	let current: ExtensionContext | undefined;

	async function take(): Promise<string> {
		if (!mindio) return "";
		const result = await pi.exec("mindio", ["take"]);
		if (result.code !== 0) throw new Error(`mindio take: ${result.stderr}`);
		return result.stdout;
	}

	// What was steered into a run and not yet delivered: pi drops queued steers when the user
	// aborts the run, but the take has already moved them to cur/, so they go into the session
	// once it settles, quietly, without starting a turn the user just stopped.
	const steered: string[] = [];
	function steer(content: string, triggerTurn = false) {
		steered.push(content);
		pi.sendMessage(
			{ customType, content, display: true },
			{ deliverAs: "steer", triggerTurn },
		);
	}
	pi.on("message_end", (event) => {
		const message = event.message;
		if (message.role !== "custom" || message.customType !== customType) return;
		const i = steered.indexOf(message.content as string);
		if (i >= 0) steered.splice(i, 1);
	});
	pi.on("agent_settled", () => {
		for (const content of steered.splice(0))
			pi.sendMessage({ customType, content, display: true });
	});

	pi.on("before_agent_start", async () => {
		const content = [brief, await take()].filter(Boolean).join("\n\n");
		brief = undefined;
		if (content) return { message: { customType, content, display: true } };
	});

	// While the agent streams, a steer lands before its next model call.
	pi.on("tool_execution_end", async () => {
		const content = await take();
		if (content) steer(content);
	});

	// The wait runs for the whole session. A wake while the agent is busy steers what is new into its run;
	// a wake while it is idle starts a turn, also when nothing is new, since a wake alone means "wake up".
	// A one-shot run (print or json) waits for no wake: it ends once settled, so what a wake took after its
	// last step would be lost, and whoever started the run answers the wake instead.
	let waiter: ChildProcess | undefined;
	let stopped = false;
	let oneShot = false;

	function wait() {
		if (stopped || !mindio) return;
		waiter = spawn("mindio", ["wait"], { stdio: "ignore" });
		waiter.on("exit", async (code) => {
			waiter = undefined;
			if (stopped || code !== 0) return;
			const marker = join(mindio, "fresh");
			const fresh = existsSync(marker);
			rmSync(marker, { force: true });
			const content = (await take()) || "You were woken, with nothing new.";
			if (fresh && current?.isIdle()) {
				wake = content;
				pi.sendUserMessage(`/${freshCommand}`, { expandPromptTemplates: true });
			} else steer(content, true);
			wait();
		});
	}

	// A fresh mind's wake: what the wake brought starts a new session, unless this one has no
	// message yet. Only a command may replace the session.
	const freshCommand = "mindio-wake-fresh";
	let wake: string | undefined;
	pi.registerCommand(freshCommand, {
		description:
			"Starts a new session with what a wake brought: a fresh mind's wake",
		handler: async (_args, ctx) => {
			const content = wake;
			wake = undefined;
			if (content === undefined) return;
			const message = { customType, content, display: true };
			if (
				!ctx.sessionManager
					.getBranch()
					.some((entry) => entry.type === "message")
			) {
				pi.sendMessage(message, { triggerTurn: true });
				return;
			}
			await ctx.newSession({
				withSession: async (ctx) => {
					await ctx.sendMessage(message, { triggerTurn: true });
				},
			});
		},
	});

	// The agent is awake from the start of a run until it has settled, retries and continuations included.
	// The lock lives in a child that holds it until its stdin closes, so it goes when pi dies too.
	// A one-shot run holds none: whoever started it holds the lock for it, as it answers the wake, such as
	// the waker, which a holder of the run's own would wait on while it waits for the run.
	let awake: ChildProcess | undefined;

	pi.on("agent_start", () => {
		if (stopped || awake || oneShot || !mindio) return;
		const holder = spawn("mindio", ["awake"], {
			stdio: ["pipe", "ignore", "ignore"],
		});
		const gone = () => {
			if (awake === holder) awake = undefined;
		};
		holder.on("exit", gone);
		holder.on("error", gone);
		holder.stdin?.on("error", gone);
		awake = holder;
	});

	function sleep() {
		awake?.stdin?.end();
		awake = undefined;
	}

	pi.on("agent_settled", sleep);

	// An interactive session waits for its wakes itself, so the machine's waker leaves them to it, for
	// as long as it is open.
	let asleep: ChildProcess | undefined;

	function stop() {
		stopped = true;
		waiter?.kill();
		sleep();
		asleep?.stdin?.end();
		asleep = undefined;
	}

	pi.on("session_start", (_event, ctx) => {
		current = ctx;
		// A replaced session starts again what the old one's end stopped.
		stopped = false;
		oneShot = ctx.mode === "print" || ctx.mode === "json";
		const session = ctx.sessionManager;
		if (!mindio) {
			mindio = join(
				homedir(),
				".agents",
				"mindio",
				`pi-${session.getSessionId()}`,
			);
			process.env.MINDIO = mindio;
		}
		for (const box of ["inbox", "outbox"])
			for (const dir of ["new", "cur", "tmp"])
				mkdirSync(join(mindio, box, dir), { recursive: true, mode: 0o700 });
		if (!session.getBranch().some((entry) => entry.type === "message")) {
			const result = spawnSync("mindio", ["brief"], { encoding: "utf8" });
			if (result.status === 0) brief = result.stdout.trimEnd();
		}
		const file = session.getSessionFile();
		const resume = join(mindio, "resume");
		if (file && !statSync(resume, { throwIfNoEntry: false })?.size) {
			writeFileSync(
				`${resume}.tmp`,
				`cd ${quote(session.getCwd())} && pi -p --session ${quote(file)} 'You were woken: what is new is given to you as inbox items.' </dev/null\n`,
				{ mode: 0o600 },
			);
			renameSync(`${resume}.tmp`, resume);
		}
		if (oneShot) return;
		asleep = spawn("mindio", ["asleep"], {
			stdio: ["pipe", "ignore", "ignore"],
		});
		asleep.stdin?.on("error", () => {});
		wait();
	});
	pi.on("session_shutdown", stop);
	process.on("exit", stop);
}
