# senses

An open standard for adding to an agent's context programmatically, harness-agnostic, as Agent Skills is for knowledge.
It is the senses of Hivemind (`~/dev/hivemind`, its design's "Senses") made into a piece of their own,
one of the small projects Hivemind's ideas go on in (vibetogether-project's `direction.md`).

## Decided

- A sense is a small program that produces content, which is injected into an agent's context, Luis's decision.
  In Luis's words: "a small program/exectuable that produces some text (or any other modality maybe) that gets injected into the context of an agent. super simple."
- All senses can be made into hooks, Luis's realization: a harness runs a sense from a hook and passes its output into the context.
- senses is a standard of its own, as minimal and universal as skills, Luis's decision.
- senses.sh, Luis's domain, is only for the senses standard, Luis's decision.
  Vibe Together's relay and web clients moved off it, to `agentics.lwirth.com`.
- senses is about context management: content reaches an agent's context in three ways, Luis's decision (2026-11-14):
  - always: loaded into every session;
  - pulled: the agent loads it itself;
  - pushed: the harness injects it on an event.
- senses implements Luis's convictions on context management, Luis's decision (2026-11-14).
  What it cannot enforce becomes a how-to on senses.sh, such as "inform, never instruct".
- A project can declare what is always loaded, Luis's decision (2026-11-14), such as `FRAMING.md` in maxwellgp-paper.

## Open

- Whether senses covers sensing other agents and people, such as their status, Luis (2026-11-08): "status is a sense. i don't know how this will manifest. but we should sense the status of other agents/people. but it's unclear if the thing we want to build, which we currently call senses should support that or related to that."
- When a sense runs: at every step, at the start of a context, or as the sense itself declares.
  Each harness lets a hook add context only at some of its events.
  Hint: the first sense in use, the clock of dothome (`home/claude-clock.sh`), runs only when the user sends a message,
  so an agent working on its own sees no time pass.
- A sense reports state, not only changes, such as the current time, Luis's point.
  Suggestion: what to report, and whether to report at all, is the sense's own choice, so the standard says nothing about it,
  and an empty output adds nothing.
  A sense that reports only changes, as Hivemind's senses do, keeps what it last reported itself.
- Modalities other than text, such as images.
- The format of a sense on disk, such as a folder with an executable and metadata, as a skill is a folder with `SKILL.md`.
- The adapters that run senses in each harness, such as Claude Code, Codex and Gemini CLI.
- Whether waking (impulse) joins senses: content that also wakes the agent, or waits for its next turn.
- Suggestion: one sense, one program, run in all three ways: by the harness at the start (always), on an event (pushed), or by the agent itself (pulled).
- Naming: plain, canonical terms for the concepts, and one brandable name for the project, as a suggestion.

## Related work

Searched 2026-11-14.

- Agent Situations (`dave1010/agent-situations`): `SITUATION.yaml` with a check and a context, a command's output among them, injected while the check passes.
  Close to a sense that is always loaded or pushed; no pulled mode, events, waking or harness adapters.
- Claude Code: all three ways, as separate features: hooks with `additionalContext`, `` !`cmd` `` in skills, the Monitor tool and channels, which also wake.
- pi: `sendMessage` with `triggerTurn` makes waking a parameter.
- Cursor rules: one unit whose loading mode is metadata, for static text only.
- Hook drafts: AHXP (`danielgerlag/harness-ext-std`), Agent Hooks 0.1 (`responsibleai/agent-hooks`), Agent Control Standard (`GenAI-Security-Project/agent-control-standard`).
  Hooks mix control and context; senses is only the context.
- MCP resources: pulled.
- Evidence that pulling fails: Vercel's evals (skills unused in 56% of cases, an always-loaded index at 100%), and "Delivery, Not Storage" (arXiv 2607.20972).
