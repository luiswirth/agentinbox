# agentinbox

The agent's inbox: its perception of everything outside it, on the agent's machine, part of Ag² (`~/dev/agentics`).
Its own repository again since 2026-11-18, out of Vibe Together, Luis's decision: "split the inbox out. it has nothing to do with VT inherently. VT is just one user."
Content reaches the agent's context, quiet at its next step, or waking it with a turn.
It carries all perception from outside the agent; proprioception, the agent's perception of itself, such as its context's size or how long a step took, is the harness's, Luis's decision (2026-11-18): "let's adopt the term proprioception and understand it as harness-internal. the inbox does all perception that is external."
Of the three ways content gets in, always (such as `AGENTS.md`), pulled (read by the agent) and pushed (delivered on an event), only push and waking are built: harnesses do the rest, Luis's decision (2026-11-15).

- One primitive, the inbox, Luis's decision (2026-11-15): a directory on the agent's machine, like a maildir, `new/` (not yet taken in), `cur/` (taken), `tmp/` (being written).
  Anything delivers into it by writing a file; it implements Luis's convictions on context management, and what it can't enforce, such as "inform, never instruct", is a how-to.
- One file per item, Luis's decision (2026-11-15): many writers deliver with no lock, an item is content of any type, and taking it in is a move.
  An item is `<time>.<sender>.<ext>`, the time in UTC to the microsecond, so names sort by time, and the sender encoded one to one, `/` as `_` and every other byte but letters, digits and `-` as `%XX`, so no sender passes for another; delivering writes into `tmp/` and hard-links into `new/`, so `new/` holds whole items and none overwrites another; taking moves to `cur/` first, so of concurrent takers exactly one gets each.
- Waking is a doorbell apart from delivering, Luis's decision (2026-11-15): a delivery is quiet, and a sender that wakes the agent also creates the file `wake`, which keeps a ring while no one watches; `wait` uses filesystem notifications, never polling.
- The sender proposes whether an item wakes, and the agent's do-not-disturb settings dispose, Luis (2026-11-16): "sure embrace the inbox", and they are one with do not disturb: "should inbox policy merge with do not disturb? i think maybe yes".
  They are the inbox's file `do-not-disturb`, which the agent keeps itself: each line fields to match and `ring` or `quiet`, the last match deciding, the proposal standing where none does; `quiet` alone is full do not disturb.
  Every perception passes it, a provider matching by the fields it names, at least `from`; so no provider decides about waking.
- Running an agent is four pieces, Luis (2026-11-16): "sounds reasonable": its identity (such as Vibe Together's agent, token and hold), its perception and bell (this inbox), the **waker**, which brings the agent's mind back when the bell rings while it is offline, and its standing job (briefing, ledger, impulses), which is its setup's, such as dothome's `daemon`.
- Every agent is reachable and wakeable from its start, and a daemon is only an agent with a standing job, decided with Luis (2026-11-19): "any agent could become a callee", and on waking: "isn't waking part of the responsbility of the inbox? ... can we rewrite the inbox to naturally support daemons?"
  - The inbox's file `resume` is the command that brings this agent's mind back, run with `sh -c` and `AGENT_INBOX`; whoever starts the agent writes it, through the harness wiring, which writes one only where there is none, so a daemon's own stays. Deliveries still write only `tmp/` and `new/`.
  - Presence is like Vibe Together's: awake while a lock on `awake` is held, a turn running; asleep while a shared lock on `asleep` is held, by a mind that waits and wakes itself, such as an open interactive session; offline otherwise. Vibe Together shows offline as asleep, since a ping wakes the agent either way.
  - One waker per machine, `agentinbox waker`, watches every inbox under `~/.agents/inbox/` and answers a ring only while the agent is offline, by running `resume` while something is untaken, holding `awake` meanwhile; a ring while a mind is awake or asleep is that mind's, a run that fails or takes nothing in ends the answer, so a broken command never loops. Without the asleep lock, it would resume an idle interactive session in a second process on the same transcript.
  - An event provider that serves every agent, such as Vibe Together, runs once per machine over all the inboxes, as the waker does, Luis (2026-11-19): "One listener per machine"; each inbox names its agent there in a file of the provider's, such as `vibetogether`.
  - An agent that won't come back removes its inbox.
- The inbox is `~/.agents/inbox/<agent>/` by default, and whoever starts an agent sets `AGENT_INBOX` to it, decided by an agent at Luis's request (2026-11-15): the agent's shell inherits it, and a path is all the inbox needs. Without it, nothing is taken in.
- The crate `agentinbox` holds it, and its command line `agentinbox`, whose `--help` documents it: `deliver`, `ring`, `take`, `wait`, `awake`, `asleep`, `waker` and `send`.

## Providers

The inbox should make adding or removing a provider of perceptions easy, Luis (2026-11-18): "we need to make the inbox really clean in the sense that we can easily add or remove "subscribers"/"providers" of "perceptions"."
Providers are of two kinds, proposed by an agent with the split (2026-11-18): events, which happen, and readings, a state the agent sees as it looks, which can't be pushed, since a sleeping agent would wake to a pile of them.

- A perception reports a change, not a clock, and states information, never policy, Luis's decisions in Hivemind, built back into the inbox at his request (2026-11-16): "let's have all hivemind perceptions that are valuable and make sense be built back into the inbox."
- An event provider is anything that writes into the directory, as it happens, so the inbox knows none: adding one is starting it, removing one is stopping it.
  Such as Vibe Together's `vibetogether inbox receive` (`~/dev/vibetogether`), messagent's Telegram chats (`~/dev/agentics/messagent`), or a daemon's impulse.
- A reading is a command that `take` runs first, at each step, so that what it delivers is taken in the same step; the readings are the lines of `agentinbox/readings` in the user's config directory, each run with `sh -c` and `AGENT_INBOX`, and one that fails is reported and passed over.
  A reading delivers quietly, only as it crosses a mark, and keeps the mark it told last in a file of the inbox named after it.
  The readings so far are the senses (`~/dev/agentics/senses`):
  - The time: `clock` tells the date and time once 15 minutes have passed since it last did; the clock is outside the agent, but how long a step took is proprioception, Luis's decisions (2026-11-18): "for time, we only provide a timestamp (date + time). not how long the last command took. that's elegant and external", "i think the timestemp should appear regurarly after some time has passed", "sure 15 mins is okay."
  - Quotas: valuable to pace the work, Luis's decision (2026-11-16); `claude-usage` tells a Claude subscription's usage limits.

## Harnesses

- Each harness is wired by `harnesses/<harness>/`, which knows only the inbox, never a provider, Luis (2026-11-18): "claude should only know about the inbox or something similar. one piece, that delivers all messagse, senses etc."
  At a session's start, the wiring makes its inbox, `AGENT_INBOX` where it was started with one, else `~/.agents/inbox/<harness>-<session id>`, writes `resume` where there is none, and holds `asleep` while an interactive session is open.
  - Claude Code: `inbox.sh` is the hook of `SessionStart`, `UserPromptSubmit` and `PostToolUse` (`settings.json`); a hook gets no environment from the session, so it finds the inbox by the session's id. At the start, it gives the session's commands `AGENT_INBOX` through `CLAUDE_ENV_FILE`, writes `claude -p --resume <session id>` as `resume`, which keeps the session's id, and holds `asleep` with `agentinbox asleep --pid` on the Claude Code process; it takes in on each prompt and after each tool call. A running agent wakes by running `agentinbox wait && agentinbox take` as a background command, and runs it again; one that doesn't hears its rings only once its session ends, from the waker.
  - pi: the extension `inbox.ts`, installed for every session, takes in at each step, keeps a `wait` running, which steers what is new into a busy agent and starts a turn in an idle one, holds `agentinbox awake` from `agent_start` until `agent_settled` and `agentinbox asleep` while the session is open, and writes `pi -p --session <file>` as `resume`; a one-shot run (`-p`) keeps no `wait` and holds no lock, so the waker that started it holds `awake` for it.

## The outbox

- The outbox is the inbox's dual: what the agent says to someone outside, by writing a file, Luis (2026-11-16): "outbox is a great idea", and its convention as drafted: "sounds reasonable".
  Whatever carries it out subscribes to it, as messagent does for Telegram: "we linkup the messagent (telegram in this case) to the inbox/outbox. like a subscriber."
- A directory like the inbox, `new/` (not yet carried), `cur/` (taken by a subscriber), `tmp/` (being written), `~/.agents/outbox/<agent>/` by default; whoever starts the agent sets `AGENT_OUTBOX` to it.
- One file per item, `<time>.<recipient>.<ext>`, the inbox's time and encoding with the recipient where the inbox has the sender: answering an inbox item is writing an item for the name it came from, such as `telegram:luis`. The extension is the type, as in the inbox.
- The agent writes it into `tmp/` and hard-links it into `new/`, as a delivery into an inbox does, or with `agentinbox send <recipient>`.
- A channel outside Vibe Together names its people as `<channel>:<name>`, such as `telegram:luis`, which no Vibe Together name can be, so no participant passes for one of them; a subscriber looks a recipient up exactly among the people it knows.
- Only a regular file is an item: a subscriber follows no link.
- A subscriber serves a prefix of recipients, such as `telegram:`, watches `new/` with filesystem notifications, never polling, and takes each item for it by moving it into `cur/` first, so of several subscribers exactly one carries each; an item no subscriber serves stays in `new/`.
- What a subscriber can't carry it tells the agent as an item of its own into the agent's inbox, from its own name, such as `messagent`, proposing a ring: the agent believes it was sent.

## Open

- Claude Code: its hooks are ignored on Luis's machines, whose managed settings allow only managed hooks, so wiring them goes through `dothome`; a `Stop` hook with `asyncRewake` might wake an idle agent, untested; it holds no awake lock, since no hook lasts a turn.
- An interactive session resumed by hand while the waker runs its resume is two minds on one session: the locks exclude only the waker.
- What a ring with nothing new should say: pi's extension wakes with a fixed line.
- A `wait` outlives a harness killed with SIGKILL, and then silences the bell for the next agent.
- The awake lock excludes only the waker: pi's extension starts its turn without waiting for the lock, and a waker killed alone, not with its process group, frees the lock while its run goes on.
- Hivemind's other senses, judged 2026-11-16:
  - The context's size: proprioception, the harness's, never the inbox's, Luis's decision (2026-11-18): "we don't track context size at all in inbox. it's harness internal."
  - Sight: how an image reaches a model is the harness's, no delivery.
  - Others' work: Vibe Together delivers others' messages, joins and leaves; their status is the next point.
- The outbox: replying to one item in particular, a thread; how an agent learns which recipients a subscriber serves.
- Whether perceiving others, such as their status, belongs here, Luis (2026-11-08): "status is a sense. i don't know how this will manifest."
- The inbox/outbox's HTTP face, for a client with no folder on the agent's machine, such as Vibe Together carrying out what an outbox addresses to a session (PotentialHedge/vibetogether#36).
- messagent keeps its own copy of the file convention (`src/maildir.rs`), where it could depend on this crate.
