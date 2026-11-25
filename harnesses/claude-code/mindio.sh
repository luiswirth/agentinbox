#!/bin/sh
# Wires a Claude Code session to its agent's IO, MindIO, as the hook of SessionStart,
# UserPromptSubmit and PostToolUse (settings.json).
#
# The agent's IO is MINDIO where the session was started with one, else
# ~/.agents/mindio/claude-code-<session id>, made at the session's start. At its start, the
# session's commands get MINDIO, the agent gets a `resume` that brings the session back where it
# has none, the session holds the asleep lock for as long as its process lives, so that the
# machine's waker leaves its wakes to it, and a new session is given `mindio brief`. At each
# prompt and after each tool call, what is new is taken in.

set -eu

input=$(cat)
field() { printf '%s' "$input" | jq -r ".$1 // \"\""; }
event=$(field hook_event_name)
session=$(field session_id)
[ -n "$session" ] || exit 0
mindio=${MINDIO:-$HOME/.agents/mindio/claude-code-$session}
export MINDIO="$mindio"

case "$event" in
SessionStart)
  [ -z "${CLAUDE_ENV_FILE:-}" ] ||
    printf 'export MINDIO=%s\n' "'$mindio'" >>"$CLAUDE_ENV_FILE"
  (umask 077 && mkdir -p "$mindio/inbox/new" "$mindio/inbox/cur" "$mindio/inbox/tmp" \
    "$mindio/outbox/new" "$mindio/outbox/cur" "$mindio/outbox/tmp")
  cwd=$(field cwd)
  # A session's first start, unless whoever started it wrote its own.
  if [ ! -s "$mindio/resume" ]; then
    printf "cd '%s' && echo 'You were woken: what is new is given to you as inbox items.' | claude -p --resume '%s'\n" \
      "$cwd" "$session" >"$mindio/resume.tmp"
    mv "$mindio/resume.tmp" "$mindio/resume"
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
    mindio asleep --pid "$pid" </dev/null >/dev/null 2>&1 &
  fi
  # Claude Code wakes no idle session itself: a background wait does.
  waiting="While you idle, \`mindio wait && mindio take\` as a background command wakes you as you are woken; run it again after it ends. Once your session has ended, a wake resumes it."
  case "$(field source)" in
  startup) context="$(mindio brief)

$waiting" ;;
  *) context=$waiting ;;
  esac
  jq -nc --arg context "$context" '{hookSpecificOutput: {hookEventName: "SessionStart", additionalContext: $context}}'
  ;;
*)
  mindio take | jq -Rsc --arg event "$event" 'select(length > 0) | {hookSpecificOutput: {hookEventName: $event, additionalContext: .}}'
  ;;
esac
