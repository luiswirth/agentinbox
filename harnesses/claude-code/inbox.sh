#!/bin/sh
# Wires a Claude Code session to its inbox, as the hook of SessionStart, UserPromptSubmit and
# PostToolUse (settings.json).
#
# The inbox is AGENT_INBOX where the session was started with one, else
# ~/.agents/inbox/claude-code-<session id>, made at the session's start, and the outbox is
# AGENT_OUTBOX, else ~/.agents/outbox/<the inbox's name>. At its start, the session's commands get
# both, the inbox gets a `resume` that brings the session back where it has none, and the session
# holds the asleep lock for as long as its process lives, so that the machine's waker leaves its
# rings to it. At each prompt and after each tool call, what is new is
# taken in.

set -eu

input=$(cat)
field() { printf '%s' "$input" | jq -r ".$1 // \"\""; }
event=$(field hook_event_name)
session=$(field session_id)
[ -n "$session" ] || exit 0
inbox=${AGENT_INBOX:-$HOME/.agents/inbox/claude-code-$session}
outbox=${AGENT_OUTBOX:-$HOME/.agents/outbox/$(basename "$inbox")}
export AGENT_INBOX="$inbox"

case "$event" in
SessionStart)
  [ -z "${CLAUDE_ENV_FILE:-}" ] ||
    printf 'export AGENT_INBOX=%s AGENT_OUTBOX=%s\n' "'$inbox'" "'$outbox'" >>"$CLAUDE_ENV_FILE"
  (umask 077 && mkdir -p "$inbox/new" "$inbox/cur" "$inbox/tmp")
  cwd=$(field cwd)
  # A session's first start, unless whoever started it wrote its own.
  if [ ! -s "$inbox/resume" ]; then
    printf "cd '%s' && echo 'Your inbox rang: what is new is given to you as inbox items.' | claude -p --resume '%s'\n" \
      "$cwd" "$session" >"$inbox/resume.tmp"
    mv "$inbox/resume.tmp" "$inbox/resume"
  fi
  # The nearest ancestor that is Claude Code itself, whose end ends the session.
  pid=$PPID
  while [ "$pid" -gt 1 ]; do
    case "$(ps -o comm= -p "$pid")" in
    claude | */claude) break ;;
    esac
    pid=$(ps -o ppid= -p "$pid" | tr -d ' ')
  done
  if [ "$pid" -gt 1 ]; then
    agentinbox asleep --pid "$pid" </dev/null >/dev/null 2>&1 &
  fi
  jq -nc --arg inbox "$inbox" '{hookSpecificOutput: {hookEventName: "SessionStart", additionalContext: "Your inbox is \($inbox), named by AGENT_INBOX in your commands. What reaches you through it is given to you at each prompt and after each tool call. While you idle, `agentinbox wait && agentinbox take` as a background command wakes you when its bell rings; run it again after it ends. Once your session has ended, a ring resumes it."}}'
  ;;
*)
  agentinbox take | jq -Rsc --arg event "$event" 'select(length > 0) | {hookSpecificOutput: {hookEventName: $event, additionalContext: .}}'
  ;;
esac
