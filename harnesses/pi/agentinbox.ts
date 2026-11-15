// Takes in the agent's inbox at each step and wakes the agent when the bell rings.
// Does nothing unless AGENT_INBOX is set.

import { type ChildProcess, spawn } from "node:child_process";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const customType = "agentinbox";

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
	let waiter: ChildProcess | undefined;
	let stopped = false;

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

	function stop() {
		stopped = true;
		waiter?.kill();
	}

	pi.on("session_start", () => wait());
	pi.on("session_shutdown", stop);
	process.on("exit", stop);
}
