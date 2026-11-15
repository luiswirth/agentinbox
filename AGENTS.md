# agentinbox

An inbox per agent: anything delivers content to an agent by writing a file into it, and the agent's harness takes it in, harness-agnostic.
It goes on from Hivemind's senses (`~/dev/hivemind`, its design's "Senses"),
one of the small projects Hivemind's ideas go on in (`~/dev/hivemind/project/direction.md`).
The concepts behind it are in `~/Documents/understanding/agents/glossary.md`, under "Perception".

## Decided

- Context injection and waking are concepts to think with, not programs, Luis's decision (2026-11-15).
  Together they are the agent's perception, in Luis's words: "i like the word perception. it's exactly what this is all about. agent's perception."
  - A perception is content that reaches an agent's context.
    It is quiet, landing at the agent's next step, or waking, starting a turn.
  - Content gets in three ways, Luis's decision (2026-11-14):
    always, loaded into every context, such as `AGENTS.md`;
    pulled, read by the agent itself;
    pushed, delivered on an event.
- Only push and waking get built, Luis's decision (2026-11-15):
  Harnesses already do always and pulled.
- What gets built is one primitive, the inbox, and the project is named after it, Luis's decision (2026-11-15): "let's call it inbox. that name is clearer."
  There are no programs called senses or impulses.
  - Each agent has an inbox, a directory on the machine where it runs, like a maildir.
  - Anything delivers a perception by writing a file into it.
  - The harness takes in what is new at each step, by one command run from its hook.
  - The harness waits on it to wake, by a blocking command:
    run as a background command of a running agent, or by an outside watcher that starts a mind.
- inbox implements Luis's convictions on context management, Luis's decision (2026-11-14).
  What it cannot enforce becomes a how-to, such as "inform, never instruct".

- One file per item, Luis's decision (2026-11-15).
  Many writers deliver at once with no lock, the file is the content of any type, such as an image, and taking it in is a move.

- Waking is a doorbell of its own, apart from delivering, Luis's decision (2026-11-15):
  dropping a file is always quiet, and a sender that wants to wake the agent also creates the file `wake` beside the item folders.
  A file, since only a file keeps a ring while no one watches, and anything rings it with `touch`.
  An agent's inbox is `new/` (not yet seen), `cur/` (seen), `tmp/` (being written) and `wake`.

- An agent is awake while its harness holds a lock on the file `awake` beside `wake`, for as long as a turn runs, Luis's decision (2026-11-15).
  State is held, never declared: the system releases the lock when the turn ends or the harness dies, as a destructor runs when a value leaves its scope, so it never goes stale.
  Anything that wants to know, such as a bridge to Vibe Together, looks at whether the lock is held.

- An agent's inbox is `~/.agents/inbox/<agent>/` by default, and whoever starts an agent sets `AGENT_INBOX` to it, decided by an agent at Luis's request (2026-11-15):
  a variable is how a process tree knows its own, as `HOME` is; the agent's shell inherits it, so the agent knows its inbox too; and a path, not a name, is all the inbox needs, so a setup may put it anywhere.
  Without it, nothing is taken in.

## Built

- The command line `agentinbox` (`src/`), whose `--help` documents it:
  `deliver`, `ring`, `take`, `wait` and `awake`, all acting on `$AGENT_INBOX` or `--inbox`.
  - An item is named `<time>.<sender>.<ext>`, such as `20261115T153012.123456Z.luis.md`, the time in UTC, so names sort by time.
  - Delivering writes into `tmp/` and hard-links into `new/`, so `new/` holds only whole items and no delivery overwrites another.
  - Taking moves each item to `cur/` before reading it, so of concurrent takers exactly one gets it.
  - `take` prints each item as `<inbox-item from=".." time="..">` with its text, or, if it is not UTF-8, as its path and type.
  - `wait` uses filesystem notifications, never polling.
  - `awake` holds the lock on `awake` until its stdin closes, so it goes when the harness dies.
- The bridge `agentinbox-vibetogether` (`bridges/vibetogether/`), whose `--help` documents it:
  it keeps one receive of the agent open, with `state` from the awake lock, delivers each item into the inbox, rings for a ping or a session's message, and acknowledges an item once it has left `new/`.
  - Each item's first line, `Vibe Together item <n> (<reach>), session <name> (<id>):`, says where it is from, and after a restart the bridge recognises its items still in `new/` by it.
  - The repository is a Cargo workspace, and the flake's package holds both binaries.
- Each harness is wired by what is in `harnesses/<harness>/`, so far:
  - Claude Code: `settings.json` holds the hooks that take in on each prompt and after each tool call.
    A running agent wakes by running `agentinbox wait && agentinbox take` as a background command,
    whose completion wakes it with what is new, and then runs it again.
  - pi: the extension `agentinbox.ts` takes in at each step and keeps a `wait` running, which steers what is new into a busy agent and starts a turn in an idle one.
    It holds `agentinbox awake` from `agent_start` until `agent_settled`.

## Open

- An interactive session no one started with `AGENT_INBOX`, such as a plain Claude Code window.

- How each harness takes in at each step, such as Claude Code, Codex and pi:
  Each lets a hook add context only at some of its events.
- Claude Code hooks of `harnesses/claude-code` are ignored on Luis's machines,
  since the managed settings allow only managed hooks: wiring them in goes through `dothome`.
- Claude Code may wake an idle agent without its help:
  a hook with `asyncRewake` wakes the model when it exits with 2, so a `Stop` hook could run `wait` and `take`.
  Untested.
- Claude Code holds no awake lock: its hooks are short-lived processes, and none lasts a turn.

- Vibe Together's receive answers at once while unacknowledged items are left, so no receive can wait while an item sits untaken in `new/`:
  the bridge then receives every 10 seconds, which delays new items by as much.
  A receive that names what it already has, apart from what it acknowledges, would end it.
- An item taken in but not yet acknowledged when the bridge dies comes again after a restart, as a duplicate:
  the bridge recognises only what is still in `new/`.

- What a ring with nothing new should say to an agent:
  The pi extension wakes it with a fixed line.
- A `wait` outlives a harness that is killed with SIGKILL, and then silences the bell for the next agent.
- Whether sender names need more than `[A-Za-z0-9_-]`, which is all a name keeps.

- Whether perceiving other agents and people, such as their status, belongs here, Luis (2026-11-08): "status is a sense. i don't know how this will manifest. but we should sense the status of other agents/people. but it's unclear if the thing we want to build, which we currently call senses should support that or related to that."

## Related work

Searched 2026-11-14.

- Claude Code: hooks with `additionalContext` push quietly; the Monitor tool, background commands and channels wake.
- pi: `sendMessage` with `triggerTurn` makes waking a parameter.
- Agent Situations (`dave1010/agent-situations`): `SITUATION.yaml` with a check and a context, injected while the check passes.
- Evidence that pulling fails: Vercel's evals (skills unused in 56% of cases, an always-loaded index at 100%), and "Delivery, Not Storage" (arXiv 2607.20972).
