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

- An agent's inbox is `~/.agents/inbox/<agent>/` by default, and whoever starts an agent sets `AGENT_INBOX` to it, decided by an agent at Luis's request (2026-11-15):
  a variable is how a process tree knows its own, as `HOME` is; the agent's shell inherits it, so the agent knows its inbox too; and a path, not a name, is all the inbox needs, so a setup may put it anywhere.
  Without it, nothing is taken in.

## Open

- An interactive session no one started with `AGENT_INBOX`, such as a plain Claude Code window.

- How each harness takes in at each step, such as Claude Code, Codex and pi:
  Each lets a hook add context only at some of its events.
- Whether perceiving other agents and people, such as their status, belongs here, Luis (2026-11-08): "status is a sense. i don't know how this will manifest. but we should sense the status of other agents/people. but it's unclear if the thing we want to build, which we currently call senses should support that or related to that."

## Related work

Searched 2026-11-14.

- Claude Code: hooks with `additionalContext` push quietly; the Monitor tool, background commands and channels wake.
- pi: `sendMessage` with `triggerTurn` makes waking a parameter.
- Agent Situations (`dave1010/agent-situations`): `SITUATION.yaml` with a check and a context, injected while the check passes.
- Evidence that pulling fails: Vercel's evals (skills unused in 56% of cases, an always-loaded index at 100%), and "Delivery, Not Storage" (arXiv 2607.20972).
