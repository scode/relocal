# TODO

Items here are planned but not yet implemented.

## `.claude/` selective sync

Currently the entire `.claude/` directory is excluded from rsync. A future version should selectively sync `.claude/`
subdirectories (skills, commands, plugins) while keeping settings and MCP configs managed independently per side.

## Daemon sync is pinned to the first caller's repo_root

The daemon binds all sync activity (background pulls, final pull) to the `repo_root` of the process that spawned it. If
a second local checkout with the same `(session, remote)` attaches to an already-running daemon, that second checkout
never receives background pulls or the final pull — syncs go to the first checkout's directory.

The spec (SPEC.md, "Session Daemon" section) says two checkouts with the same session name and remote "should share the
same daemon" and that "the initial push comes from whichever checkout spawned the daemon." This is intentionally vague
about sync targets, but the current implementation silently does the wrong thing for the second checkout. Either:

- The daemon should track per-client repo_roots and sync to all of them (adds complexity to the poll loop and IPC
  protocol).
- The spec should explicitly state that only the spawning checkout receives syncs, and other checkouts must use
  `relocal sync pull` manually.
- The daemon identity should include repo_root after all, giving each checkout its own daemon (but then two daemons
  fight over the same remote directory and lock file, which is worse).

## Daemon errors are hard to find

When the daemon fails during startup (e.g., stale lock file), the error is logged to the daemon's log file in `$TMPDIR`
— not to the client's terminal. The client only sees a generic "daemon did not signal readiness" message with no hint
about the root cause. The user has to know to run `relocal log` (or manually find the log file) to see what actually
went wrong. We should surface the daemon's last error in the client-side failure message.
