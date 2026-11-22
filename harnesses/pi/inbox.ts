// Takes in the agent's inbox at each step, wakes the agent when the bell rings, and holds the
// awake lock while a run lasts and the asleep lock while an interactive session is open.
// The inbox is AGENT_INBOX where pi was started with one, else ~/.agents/inbox/pi-<session id>,
// made at the session's start, which then gets a `resume` that brings the session back where it
// has none, and the outbox is AGENT_OUTBOX, else ~/.agents/outbox/<the inbox's name>; the resume finds this extension installed where pi loads it for every session.
// A ring with the file `fresh` in the inbox starts a new session, a fresh mind's, where the
// agent is idle; a busy agent takes it in its run, so the file only goes.

import { type ChildProcess, spawn } from "node:child_process";
import {
	existsSync,
	mkdirSync,
	renameSync,
	rmSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { basename, join } from "node:path";
import type {
	ExtensionAPI,
	ExtensionContext,
} from "@earendil-works/pi-coding-agent";

const customType = "inbox";

const quote = (s: string) => `'${s.replaceAll("'", "'\\''")}'`;

export default function (pi: ExtensionAPI) {
	let inbox = process.env.AGENT_INBOX;
	// The session's context, current across a replacement of the session.
	let current: ExtensionContext | undefined;

	async function take(): Promise<string> {
		if (!inbox) return "";
		const result = await pi.exec("agentinbox", ["take"]);
		if (result.code !== 0) throw new Error(`agentinbox take: ${result.stderr}`);
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
		const content = await take();
		if (content) return { message: { customType, content, display: true } };
	});

	// While the agent streams, a steer lands before its next model call.
	pi.on("tool_execution_end", async () => {
		const content = await take();
		if (content) steer(content);
	});

	// The wait runs for the whole session. A ring while the agent is busy steers what is new into its run;
	// a ring while it is idle starts a turn, also when nothing is new, since the bell alone means "wake up".
	// A one-shot run (print or json) waits for no bell: it ends once settled, so what a ring took after its
	// last step would be lost, and whoever started the run hears the bell instead.
	let waiter: ChildProcess | undefined;
	let stopped = false;
	let oneShot = false;

	function wait() {
		if (stopped || !inbox) return;
		waiter = spawn("agentinbox", ["wait"], { stdio: "ignore" });
		waiter.on("exit", async (code) => {
			waiter = undefined;
			if (stopped || code !== 0) return;
			const marker = join(inbox, "fresh");
			const fresh = existsSync(marker);
			rmSync(marker, { force: true });
			const content =
				(await take()) || "The inbox bell rang, with nothing new.";
			if (fresh && current?.isIdle()) {
				wake = content;
				pi.sendUserMessage(`/${freshCommand}`, { expandPromptTemplates: true });
			} else steer(content, true);
			wait();
		});
	}

	// A fresh mind's wake: what the bell brought starts a new session, unless this one has no
	// message yet. Only a command may replace the session.
	const freshCommand = "inbox-wake-fresh";
	let wake: string | undefined;
	pi.registerCommand(freshCommand, {
		description:
			"Starts a new session with what the inbox bell brought: a fresh mind's wake",
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
	// A one-shot run holds none: whoever started it holds the lock for it, as it hears the bell, such as
	// the waker, which a holder of the run's own would wait on while it waits for the run.
	let awake: ChildProcess | undefined;

	pi.on("agent_start", () => {
		if (stopped || awake || oneShot || !inbox) return;
		const holder = spawn("agentinbox", ["awake"], {
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

	// An interactive session waits for the bell itself, so the machine's waker leaves its rings to it,
	// for as long as it is open.
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
		if (!inbox) {
			inbox = join(
				homedir(),
				".agents",
				"inbox",
				`pi-${session.getSessionId()}`,
			);
			process.env.AGENT_INBOX = inbox;
		}
		process.env.AGENT_OUTBOX ??= join(
			homedir(),
			".agents",
			"outbox",
			basename(inbox),
		);
		for (const dir of ["new", "cur", "tmp"])
			mkdirSync(join(inbox, dir), { recursive: true, mode: 0o700 });
		const file = session.getSessionFile();
		const resume = join(inbox, "resume");
		if (file && !statSync(resume, { throwIfNoEntry: false })?.size) {
			writeFileSync(
				`${resume}.tmp`,
				`cd ${quote(session.getCwd())} && pi -p --session ${quote(file)} 'Your inbox rang: what is new is given to you as inbox items.' </dev/null\n`,
				{ mode: 0o600 },
			);
			renameSync(`${resume}.tmp`, resume);
		}
		if (oneShot) return;
		asleep = spawn("agentinbox", ["asleep"], {
			stdio: ["pipe", "ignore", "ignore"],
		});
		asleep.stdin?.on("error", () => {});
		wait();
	});
	pi.on("session_shutdown", stop);
	process.on("exit", stop);
}
