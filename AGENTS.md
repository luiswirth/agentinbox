# senses

An open standard for adding to an agent's context programmatically, harness-agnostic, as Agent Skills is for knowledge.
It is the senses of Hivemind (`~/dev/agentic-infrastructure/hivemind`, its design's "Senses") made into a piece of their own,
one of the small projects Hivemind's ideas go on in (polylogue-project's `direction.md`).

## Decided

- A sense is a small program that produces content, which is injected into an agent's context, Luis's decision.
  In Luis's words: "a small program/exectuable that produces some text (or any other modality maybe) that gets injected into the context of an agent. super simple."
- All senses can be made into hooks, Luis's realization: a harness runs a sense from a hook and passes its output into the context.
- senses is a standard of its own, as minimal and universal as skills, Luis's decision.
- senses.sh, Luis's domain, is only for the senses standard, Luis's decision.
  polylogue's relay and web clients moved off it, to `agentic.lwirth.com`.

## Open

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
- Prior art, to be searched properly before the standard is presented as new.
  Related, as found so far:
  - The hooks of the CLI harnesses converge on JSON over stdin and stdout, and on control: allowing and blocking.
    Drafts that write this down: AHXP (`danielgerlag/harness-ext-std`), Agent Hooks 0.1 (`responsibleai/agent-hooks`), Agent Control Standard (`GenAI-Security-Project/agent-control-standard`).
    Suggestion: hooks mix control and context, and senses is only the context.
  - MCP's resources, which an agent pulls, where a sense pushes.
