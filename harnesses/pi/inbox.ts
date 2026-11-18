// Takes in the agent's inbox at each step, wakes the agent when the bell rings, and holds the
// awake lock while a run lasts.
// Does nothing unless AGENT_INBOX is set.

import { type ChildProcess, spawn } from "node:child_process";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const customType = "inbox";

export default function (pi: ExtensionAPI) {
	if (!process.env.AGENT_INBOX) return;

	async function take(): Promise<string> {
		const result = await pi.exec("agentinbox", ["take"]);
		if (result.code !== 0) throw new Error(`agentinbox take: ${result.stderr}`);
		return result.stdout;
	}

	pi.on("before_agent_start", async () => {
		const content = await take();
		if (content) return { message: { customType, content, display: true } };
	});

	// While the agent streams, a steer lands before its next model call.
	pi.on("tool_execution_end", async () => {
		const content = await take();
		if (content)
			pi.sendMessage(
				{ customType, content, display: true },
				{ deliverAs: "steer" },
			);
	});

	// The wait runs for the whole session. A ring while the agent is busy steers what is new into its run;
	// a ring while it is idle starts a turn, also when nothing is new, since the bell alone means "wake up".
	// A one-shot run (print or json) waits for no bell: it ends once settled, so what a ring took after its
	// last step would be lost, and whoever started the run hears the bell instead.
	let waiter: ChildProcess | undefined;
	let stopped = false;
	let oneShot = false;

	function wait() {
		if (stopped) return;
		waiter = spawn("agentinbox", ["wait"], { stdio: "ignore" });
		waiter.on("exit", async (code) => {
			waiter = undefined;
			if (stopped || code !== 0) return;
			const content =
				(await take()) || "The inbox bell rang, with nothing new.";
			pi.sendMessage(
				{ customType, content, display: true },
				{ deliverAs: "steer", triggerTurn: true },
			);
			wait();
		});
	}

	// The agent is awake from the start of a run until it has settled, retries and continuations included.
	// The lock lives in a child that holds it until its stdin closes, so it goes when pi dies too.
	// A one-shot run holds none: whoever started it holds the lock for it, as it hears the bell, such as
	// the waker, which a holder of the run's own would wait on while it waits for the run.
	let awake: ChildProcess | undefined;

	pi.on("agent_start", () => {
		if (stopped || awake || oneShot) return;
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

	function stop() {
		stopped = true;
		waiter?.kill();
		sleep();
	}

	pi.on("session_start", (_event, ctx) => {
		oneShot = ctx.mode === "print" || ctx.mode === "json";
		if (!oneShot) wait();
	});
	pi.on("session_shutdown", stop);
	process.on("exit", stop);
}
