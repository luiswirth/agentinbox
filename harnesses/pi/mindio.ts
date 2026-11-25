// Wires a pi session to its agent's IO, MindIO: takes in the inbox at each step, wakes the agent
// as it is woken, and holds the awake lock while a run lasts and the asleep lock while an
// interactive session is open. A new session is first given `mindio brief`.
// The agent's IO is MINDIO where pi was started with one, else ~/.agents/mindio/pi-<session id>,
// made at the session's start, which then gets a `resume` that brings the session back where it
// has none; the resume finds this extension installed where pi loads it for every session.
// A wake with the file `fresh` in the agent's IO starts a new session, a fresh mind's, where the
// agent is idle; a busy agent takes it in its run, so the file only goes.
// An interrupt cuts off a busy agent's run, keeps the reply as far as it got in what the model
// sees, and starts a run with what is new once it is whole.
// The tool `send` writes an outbox item, streamed into it as the model writes its text.

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
	// The terminal's abort hands pi's queues to the editor, dropping these; any other keeps them
	// queued for the next run.
	pi.on("agent_settled", (event, ctx) => {
		if (event.aborted && ctx.mode !== "tui") steered.length = 0;
		for (const content of steered.splice(0))
			pi.sendMessage({ customType, content, display: true });
		if (interrupted) {
			interrupted = false;
			void continueInterrupted();
		}
	});

	// An interrupt: the run is cut off at once, and the next starts with what is new, once whole.
	let interrupted = false;
	async function continueInterrupted() {
		const content = await take().catch((e) => String(e));
		pi.sendMessage(
			{
				customType,
				content: `You were interrupted: what you were doing was cut off where it ends.${content ? `\n\n${content}` : ""}`,
				display: true,
			},
			{ triggerTurn: true },
		);
	}

	// pi leaves a reply cut off by an abort out of what the model sees; it is kept as far as it got,
	// with a result for each tool call it was writing, which never ran.
	pi.on("context", (event) => {
		let changed = false;
		const messages: typeof event.messages = [];
		for (const message of event.messages) {
			if (message.role !== "assistant" || message.stopReason !== "aborted") {
				messages.push(message);
				continue;
			}
			changed = true;
			const content = message.content.filter(
				(block) =>
					(block.type === "text" && block.text) ||
					block.type === "toolCall" ||
					(block.type === "thinking" && block.thinkingSignature),
			);
			const calls = content.filter((block) => block.type === "toolCall");
			if (!content.some((block) => block.type !== "thinking")) continue;
			messages.push({
				...message,
				content,
				stopReason: calls.length ? "toolUse" : "stop",
			});
			for (const call of calls)
				messages.push({
					role: "toolResult",
					toolCallId: call.id,
					toolName: call.name,
					content: [
						{
							type: "text",
							text:
								call.name === "send"
									? "Cut off while you wrote it: the recipient got the text as far as you wrote it."
									: "Cut off while you wrote it: it never ran.",
						},
					],
					isError: true,
					timestamp: message.timestamp,
				});
		}
		if (changed) return { messages };
	});

	// `send`: each call is a `mindio send`, started once its recipient is whole and fed its text as
	// the model writes it, so the item streams. A call whose text the stream did not carry, or a
	// provider that streams no arguments, is sent whole as it runs.
	type Ended = { code: number | null; path: string; error: string };
	type Sending = {
		child: ChildProcess;
		written: number;
		ended: Promise<Ended>;
	};
	const sendTool = "send";
	const sendings = new Map<string, Sending>();

	function startSending(id: string, recipient: string): Sending {
		const child = spawn("mindio", ["send", recipient], {
			stdio: ["pipe", "pipe", "pipe"],
		});
		let path = "";
		let error = "";
		child.stdout?.on("data", (data) => {
			path += data;
		});
		child.stderr?.on("data", (data) => {
			error += data;
		});
		child.stdin?.on("error", () => {});
		const ended = new Promise<Ended>((resolve) => {
			child.on("close", (code) =>
				resolve({ code, path: path.trim(), error: error.trim() }),
			);
			child.on("error", (e) =>
				resolve({ code: null, path: "", error: String(e) }),
			);
		});
		const sending = { child, written: 0, ended };
		sendings.set(id, sending);
		return sending;
	}

	// Writes what is new of the text, holding back half of a surrogate pair until its other half.
	function feed(sending: Sending, text: string, whole: boolean) {
		let piece = text.slice(sending.written);
		const last = piece.charCodeAt(piece.length - 1);
		if (!whole && last >= 0xd800 && last < 0xdc00) piece = piece.slice(0, -1);
		if (!piece) return;
		sending.child.stdin?.write(piece);
		sending.written += piece.length;
	}

	pi.on("message_update", (event, ctx) => {
		const update = event.assistantMessageEvent;
		if (update.type !== "toolcall_delta" && update.type !== "toolcall_end")
			return;
		// A few deltas still come after an abort: the item ends where the abort cut it.
		if (ctx.signal?.aborted) return;
		const end = update.type === "toolcall_end";
		const call = end
			? update.toolCall
			: update.partial.content[update.contentIndex];
		if (call?.type !== "toolCall" || call.name !== sendTool) return;
		const args = (call.arguments ?? {}) as Record<string, unknown>;
		let sending = sendings.get(call.id);
		if (!sending) {
			// The recipient is whole once another argument follows it, or the call is.
			const keys = Object.keys(args);
			const whole = end || keys.indexOf("recipient") < keys.length - 1;
			if (typeof args.recipient !== "string" || !args.recipient || !whole)
				return;
			sending = startSending(call.id, args.recipient);
		}
		if (typeof args.text === "string") feed(sending, args.text, end);
		if (end) sending.child.stdin?.end();
	});

	// The reply has ended, also cut off: every item it was writing ends as far as it got.
	pi.on("message_end", (event) => {
		if (event.message.role !== "assistant") return;
		for (const sending of sendings.values()) sending.child.stdin?.end();
	});

	pi.registerTool({
		name: sendTool,
		label: "send",
		description:
			"Says something to someone outside: writes it as an item into your outbox, for a recipient, which a subscriber carries to them. It streams: the recipient gets the text as you write it. Answering an inbox item is sending to the name it came from.",
		promptSnippet: "Say something to someone outside, through your outbox",
		parameters: {
			type: "object",
			properties: {
				recipient: {
					type: "string",
					description:
						"Whom it is for: the name an inbox item came from, to answer it.",
				},
				text: { type: "string", description: "What to say, in Markdown." },
			},
			required: ["recipient", "text"],
		} as never,
		async execute(id, params: { recipient: string; text: string }) {
			let sending = sendings.get(id);
			if (!sending) {
				sending = startSending(id, params.recipient);
				feed(sending, params.text, true);
				sending.child.stdin?.end();
			}
			const { code, path, error } = await sending.ended;
			sendings.delete(id);
			if (code !== 0)
				throw new Error(`mindio send: ${error || `exit ${code}`}`);
			return {
				content: [{ type: "text", text: `Sent to ${params.recipient}.` }],
				details: { path },
			};
		},
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
		const io = mindio;
		if (stopped || !io) return;
		const child = spawn("mindio", ["wait"], {
			stdio: ["ignore", "pipe", "ignore"],
		});
		waiter = child;
		let verdict = "";
		child.stdout?.on("data", (data) => {
			verdict += data;
		});
		child.on("close", async (code) => {
			waiter = undefined;
			if (stopped || code !== 0) return;
			if (verdict.trim() === "interrupt" && current && !current.isIdle()) {
				interrupted = true;
				current.abort();
				wait();
				return;
			}
			const marker = join(io, "fresh");
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
