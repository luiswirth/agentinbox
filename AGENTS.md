---
description: "MindIO: the boundary between an agent's mind and its world, as input, output and waking, on the agent's machine, part of lwirth-agentics (`~/dev/agentics`). Use when working on MindIO, or on how anything reaches an agent or leaves it."
---

# MindIO

MindIO is a mind's IO: input, output and waking.
A brain has the first two, perception and expression, but is never woken; IO in hardware or an OS has all three, with interrupts as the waking.
It is for thinking about minds at the low level, in the style of Cogicorpus, without a harness of its own: whatever the harness, the mind's boundary with its world is MindIO.
Modalities build on it, each doing all three for its own, such as VoiceIO (`~/dev/agentics/voiceio`).

Written MindIO in prose; the repository, crate and command are `mindio`, whose `--help` documents the command line and whose doc comments the library.

## The agent's IO

An agent's IO is one directory on its machine, `~/.agents/mindio/<agent>/`, named after the agent, and `MINDIO` names it to whatever the agent runs.
It persists across the agent's minds.

- `inbox/`, the input: what reaches the agent from outside, taken in at its next step.
- `outbox/`, the output: what the agent says to someone outside, carried by a subscriber.
- `wake`: present from a wake until a mind answers it, and `interrupt` beside it where the wake also cuts off the running step.
- `awake`, locked while a mind is in a turn, and `asleep`, shared-locked while a mind waits and wakes itself, such as an open interactive session; with neither held, the agent is offline.
- `resume`: the command that brings the agent's mind back, run with `sh -c` and `MINDIO`; whoever starts the agent writes it, through the harness wiring, which writes one only where there is none.
- `fresh`: asks for a fresh mind: the next wake starts a new session where the agent is idle; whoever triggers the agent writes it, and whatever brings the mind up removes it.
- `waking`: the agent's rules on what wakes it.
- `alarms/`: the alarms.
- `status`: one line on what the agent is doing and where.

An agent that won't come back removes its directory.
Every agent is reachable and wakeable from its start, and a daemon is only an agent with a standing job, which is its setup's, such as dothome's `agent`.

### Input and output

The inbox and the outbox are one pair, the same shape mirrored: `new/` (not yet taken), `cur/` (taken), `tmp/` (being written), one file per item.

- An item is `<time>.<name>.<ext>`: the time in UTC to the microsecond, so names sort by time; the name the inbox's sender or the outbox's recipient, encoded one to one, `/` as `_` and every other byte but letters, digits and `-` as `%XX`, so no name passes for another; the extension its type.
- Writing goes into `tmp/` and hard-links into `new/`, so none overwrites another, with no lock among many writers.
- An item may stream, for a seamless real-time exchange: its writer takes an exclusive `flock` on it in `tmp/` and links it into `new/` as it starts, at its first byte, and it grows there; it is whole once no writer holds the lock, also where the writer crashed, which leaves it as far as it got. The lock is on the file, so it goes along into `cur/`. Streaming is an option for whoever writes, never a requirement (Luis, 2026-11-25), so an item written without the lock is whole at once. `mindio send` and `deliver` stream stdin.
- A reader never reads an item before it is whole: it follows it (`mindio follow`), with filesystem notifications for what is written and the lock for the end, never polling. `take` gives each item once whole.
- Taking moves to `cur/` first, so of concurrent takers exactly one gets each.
- Answering an inbox item is writing an outbox item for the name it came from, such as `telegram:luis`.
- A channel names its people as `<channel>:<name>`, such as `telegram:luis`, so no name passes for another channel's.
- A subscriber serves a prefix of recipients, such as messagent `telegram:`, watches `outbox/new/` with filesystem notifications, and takes each item it serves; an item none serves stays.
  Only a regular file is an item: a subscriber follows no link.
  What it can't carry it tells the agent as an inbox item from its own name.

### Waking

Waking is not built into any provider: a provider only delivers events, and each agent sets what wakes it.
Luis: "Mind I O sollte es möglich machen, für jeden Agent selber komplett customizen zu können. Super simpel, aber super expressive einzustellen, wie er geweckt wird."

- An item has fields: `from`, `kind` (`event` or `reading`), and those its provider gives, such as an alarm's `alarm` and `by`.
- The agent's rules, the file `waking`, come after the defaults, `kind=event wake` and `kind=reading quiet`: each line fields to match and `quiet`, `wake` or `interrupt`, the last match deciding.
  So an agent that sets nothing is woken by every event: never being woken is the worst case, and a needless wake costs little, since the mind ends that turn at once.
- They match at delivery, so an item that streams wakes or interrupts at its first byte.
- An interrupt, Luis: "mind/input interrupts (like hardware). i love it.": it cuts off the running step, keeps the reply as far as it got, and continues with the input at once, once whole; `mindio wait` says which it was. An agent with no mind running is woken.
- A harness gives a new session `mindio brief`, so every agent knows its IO and that its waking is its own.
- Waking uses filesystem notifications, never polling.

### The waker

One waker per machine, `mindio waker`, watches every agent's IO under `~/.agents/mindio/` and answers a wake only while the agent is offline, by running `resume` while something is untaken, holding `awake` meanwhile.
A wake while a mind is awake or asleep is that mind's; a run that fails or takes nothing in ends the answer, so a broken command never loops.
Without the asleep lock, it would resume an idle interactive session in a second process on the same transcript.

### Alarms

An alarm is a command that waits and a note, `alarms/<name>/`, so it outlasts restarts; anyone sets one for any agent.
The waker runs each alarm's command in a process group of its own, and once it ends delivers the note as an event from `alarm`; a once alarm is then dismissed, a repeating one runs again, and one that fails is told and dismissed.
Its command runs again whenever the waker starts again, so it only waits, and a time is absolute: `--at` and `--in` are `mindio until <time>`.
Stopping the waker stops the commands and keeps the alarms.

## Providers

Adding or removing a provider of perceptions is easy: MindIO knows none.
Luis: "we need to make the inbox really clean in the sense that we can easily add or remove "subscribers"/"providers" of "perceptions"."

- A perception reports a change, not a clock, and states information, never policy.
- Providers are of two kinds, the item's `kind`:
  - An event happens, and its provider writes into the inbox as it happens, so adding one is starting it.
    Such as messagent's Telegram chats (`~/dev/agentics/messagent`), or a daemon's impulse.
    A provider that serves every agent runs once per machine over all of them, as the waker does, and each agent names itself there in a file of the provider's.
  - A reading is a state the agent sees as it looks, which can't be pushed, since a sleeping agent would wake to a pile of them.
    It is a command that `take` runs first, at each step, with `MINDIO_KIND=reading`, so what it delivers is taken in the same step; the readings are the lines of `mindio/readings` in the user's config directory, and one that fails is reported and passed over.
    A reading delivers only as it crosses a mark, and keeps the mark it told last in a file of the agent's IO named after it.
    The readings so far are the senses (`~/dev/agentics/senses`):
    - The time: `clock` tells the date and time once 15 minutes have passed since it last did.
    - Quotas: `claude-usage` tells a Claude subscription's usage limits.
- MindIO carries all perception from outside the agent; proprioception, its perception of itself, such as its context's size or how long a step took, is the harness's.
- Of the three ways content gets in, always (such as `AGENTS.md`), pulled (read by the agent) and pushed (delivered on an event), MindIO builds only push and waking: harnesses do the rest.

## Status

An agent's status is one line on what it is doing and where, which it sets itself and which lasts until the next; anyone may set any agent's.
It is there for coordination, Luis: "agents maybe need a sense/perception to understand what the other agents are doing."

- `mindio agents` lists the agents of the machine and of its peers, each with its presence, from the locks, and its status with when it was set.
- An agent's IO is on its machine, so other machines' agents are seen through the peers: a line `<name> <command>` of `mindio/peers` in the user's config directory, whose command lists its agents, such as `ssh <host> mindio agents --here --porcelain`; MindIO knows no machine, and a peer's agents are `<peer>:<agent>`.
  What a peer listed last is cached, in `mindio/peers/` of the user's cache directory, and asked again once a look finds it older than the looker's while; a peer out of reach keeps what it listed last.
- The sense `agent-statuses` tells the statuses of the machine's other agents and of the peers', as they change, quietly; presence changes at every turn, so it is not told.

## Harnesses

Each harness is wired by `harnesses/<harness>/`, which knows only MindIO, never a provider.
At a session's start, the wiring makes the agent's IO, `MINDIO` where it was started with one, else `~/.agents/mindio/<harness>-<session id>`, writes `resume` where there is none, gives a new session `mindio brief`, and holds `asleep` while an interactive session is open.

- Claude Code: `mindio.sh` is the hook of `SessionStart`, `UserPromptSubmit` and `PostToolUse` (`settings.json`); a hook gets no environment from the session, so it finds the agent's IO by the session's id. At the start, it gives the session's commands `MINDIO` through `CLAUDE_ENV_FILE`, writes `claude -p --resume <session id>` as `resume`, and holds `asleep` with `mindio asleep --pid` on the Claude Code process; it takes in on each prompt and after each tool call. A running agent wakes by running `mindio wait && mindio take` as a background command, and runs it again; one that doesn't is woken only once its session ends, by the waker.
- pi: the extension `mindio.ts`, installed for every session, takes in at each step, keeps a `wait` running, which steers what is new into a busy agent and starts a turn in an idle one, holds `mindio awake` from `agent_start` until `agent_settled` and `mindio asleep` while the session is open, and writes `pi -p --session <file>` as `resume`; an interrupt aborts a busy run, keeps the reply cut off in what the model sees, with a result for a tool call it was writing, and starts the next run with what is new once whole; its tool `send` streams the text into an outbox item from the call's arguments as the model writes them; a one-shot run (`-p`) keeps no `wait` and holds no lock, so the waker that started it holds `awake` for it; a wake with `fresh` starts a new session where the agent is idle.

## Open

- Interrupts in Claude Code: nothing cuts off its step, so an interrupt only wakes it; and model-pool, with Claude Code behind it, can't take one cleanly, since Claude Code keeps its own partial reply.
- To be named better: the inbox and the outbox, as one pair. Luis: "Entweder heißen beide Inbox und Outbox oder wir finden für beide bessere Begriffe."
- Claude Code: its hooks are ignored on Luis's machines, whose managed settings allow only managed hooks, so wiring them goes through `dothome`; a `Stop` hook with `asyncRewake` might wake an idle agent, untested; it holds no awake lock, since no hook lasts a turn.
- An interactive session resumed by hand while the waker runs its resume is two minds on one session: the locks exclude only the waker.
- A `wait` outlives a harness killed with SIGKILL, and then answers the next mind's wakes.
- The awake lock excludes only the waker: pi's extension starts its turn without waiting for the lock, and a waker killed alone, not with its process group, frees the lock while its run goes on.
- An item's fields are matched at delivery and not kept, so an agent learns a provider's fields only from the provider's documentation.
- Sight: how an image reaches a model is the harness's.
- Others' last tool calls, working directories and Rex sessions: the harness's or the setup's, never MindIO's.
- MindIO's HTTP face, for a client with no folder on the agent's machine.
- The outbox: replying to one item in particular, a thread; how an agent learns which recipients a subscriber serves.
