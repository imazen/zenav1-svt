#!/usr/bin/env python3
"""PreToolUse hook: bound every `exec` command with timeout(1).

Why this exists: the exec tool's own `timeout` parameter only decides when
control returns to the agent — an expired command is *backgrounded*, not
killed. A hung process (an encoder stuck in an infinite loop, a wedged gate)
keeps burning CPU forever. This hook rewrites each exec command to run under
`timeout(1)`, which SIGTERMs the whole process group after the budget and
SIGKILLs it 10 s later, so nothing an agent runs can spin indefinitely.

Budgets (seconds, env-overridable):
  DEVIN_EXEC_TIMEOUT_S        normal commands            (default 1800)
  DEVIN_EXEC_HEAVY_TIMEOUT_S  commands under run-heavy   (default 7200)

A command is NOT wrapped when:
  * `tty` is set (interactive sessions manage their own lifecycle),
  * it is a lone session-state builtin (`cd`, `export`, `source`, ...) —
    inside `bash -c` the effect would silently not persist to the session,
  * it already starts with `timeout(1)` — the agent chose an explicit bound,
    possibly a longer one than the default cap.

If the exec call itself declares a `timeout` (ms) longer than the cap, the
budget is raised to cover it — killing a job the caller explicitly asked to
wait for would be surprising. A killed command exits 124, which is visible
in the tool output and is the detection signal for a hung process.
"""

import json
import os
import re
import shlex
import sys

_DEFAULT_BUDGET_S = 1800
_HEAVY_BUDGET_S = 7200
_KILL_GRACE_S = 10

# Lone builtins that mutate the persistent shell's state — wrapping them in
# `timeout ... bash -c` runs them in a subshell and silently loses the effect.
_SESSION_STATE_RE = re.compile(
    r"^\s*"
    r"(?:cd|export|source|\.|set|unset|alias|unalias|pushd|popd|dirs|history|"
    r"jobs|fg|bg|wait|exec|trap|ulimit|umask|read|disown|logout|exit)\b"
)
_COMPOUND_RE = re.compile(r"[;&|\n]")

# The command already leads with timeout(1), optionally behind VAR=value
# assignments and nice/ionice/env prefixes.
_ALREADY_BOUNDED_RE = re.compile(
    r"^\s*(?:[A-Za-z_][A-Za-z0-9_]*=\S+\s+)*"
    r"(?:(?:nice|ionice|env|command|builtin)\s+(?:-\S+\s+|-[a-zA-Z]\s+\S+\s+)*)*"
    r"timeout\b"
)


def _budget(tool_input: dict, command: str) -> int:
    heavy = "run-heavy" in command
    env_name = "DEVIN_EXEC_HEAVY_TIMEOUT_S" if heavy else "DEVIN_EXEC_TIMEOUT_S"
    fallback = _HEAVY_BUDGET_S if heavy else _DEFAULT_BUDGET_S
    raw = os.environ.get(env_name)
    try:
        budget = int(raw) if raw else fallback
    except ValueError:
        budget = fallback
    # A declared exec wait budget is a hint the agent expects a long run;
    # never wrap shorter than it (plus margin). Absent/0 means "background
    # quickly", not a long wait, so it does not raise the floor.
    try:
        declared_ms = int(tool_input.get("timeout") or 0)
    except (TypeError, ValueError):
        declared_ms = 0
    if declared_ms > 0:
        budget = max(budget, declared_ms // 1000 + 60)
    return budget


def main() -> None:
    try:
        data = json.load(sys.stdin)
    except Exception:
        return  # malformed input: fail open, never block the tool call
    tool_input = data.get("tool_input") or {}
    command = tool_input.get("command") or ""
    stripped = command.strip()
    if not stripped or tool_input.get("tty"):
        return
    if _ALREADY_BOUNDED_RE.match(stripped):
        return
    if _SESSION_STATE_RE.match(stripped) and not _COMPOUND_RE.search(stripped):
        return
    budget = _budget(tool_input, command)
    wrapped = (
        f"timeout --signal=TERM --kill-after={_KILL_GRACE_S}s "
        f"{budget}s bash -c {shlex.quote(command)}"
    )
    print(
        json.dumps(
            {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "updatedInput": {"command": wrapped},
                }
            }
        )
    )


if __name__ == "__main__":
    main()
