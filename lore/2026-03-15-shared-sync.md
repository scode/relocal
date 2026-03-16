# Shared sync infrastructure for concurrent tool invocations

## Problem

You want to run `relocal claude` and `relocal codex` against the same session simultaneously — multiple agents working
on the same remote directory, with a single sync loop pulling changes back to local. Currently each invocation creates
its own SSH ControlMaster, background sync loop, and remote lock file. The lock file actively prevents a second
invocation from joining.

The core challenge is lifecycle management: who sets up the shared resources, who tears them down, and what happens when
processes crash.

## Alternatives considered

### PID-directory refcounting

Each invocation writes its PID to a shared directory (`$TMPDIR/rlc-<session>/pids/<pid>`). First writer sets up
infrastructure; last remover tears it down. ControlMaster gets a deterministic socket path (keyed on session name, not
PID) so all invocations share it.

The problem is atomicity. "Am I first?" and "am I last?" checks are inherently racy against the filesystem — another
process can appear or disappear between checking the directory contents and acting on them. You can mitigate with flock,
but then you're coordinating flock acquisition across every invocation for every lifecycle event, and you still have
stale PID files from crashes that need cleanup heuristics (is PID 12345 still alive? what if it was recycled?).

### Explicit start/stop commands

Add `relocal session start` and `relocal session stop`. The user manages the lifecycle. `relocal claude` /
`relocal codex` just run the interactive SSH part and assume the session is already up.

Simple to implement but changes the UX. The user has to remember to start and stop, and a crash leaves an orphaned
daemon that needs manual cleanup. The whole point of relocal is that `relocal claude` is the only command you need to
run — adding mandatory setup/teardown steps undermines that.

### Session daemon (chosen)

First invocation for a session spawns a daemon process that owns all shared resources. The daemon listens on a Unix
domain socket. Subsequent invocations connect to the daemon. When the last client disconnects, the daemon does a final
pull and tears down.

## Why the daemon approach wins

The key insight is that the Unix socket gives you kernel-managed client tracking for free. When a client process dies —
whether it exits cleanly, gets killed, or crashes — the kernel closes its file descriptors, including the Unix socket
connection. The daemon sees this as an EOF on that client's fd. No stale PID files, no "is this process still alive?"
heuristics, no cleanup races.

The "am I first?" question becomes "does the daemon socket exist and accept connections?" — a single atomic operation.
The "am I last?" question becomes "did the daemon's client count reach zero?" — tracked by the daemon itself with no
filesystem coordination.

The complexity cost is real: the daemon needs an event loop (poll-based) to multiplex the listener socket, client
connections, and sync timer. But this is a one-time cost in a single module. The alternative approaches spread fragile
coordination logic across every invocation path.

## Trade-offs accepted

- **New dependency**: `nix` crate for `poll(2)`. The daemon's event loop is the natural place for it, and the
  alternative (raw `libc::poll` calls) is just `nix` without the ergonomics.

- **Daemon crash kills ControlMaster**: If the daemon dies while clients are running, they lose their shared SSH
  connection. This is the same failure mode as a network drop — the client prints the dirty shutdown message and the
  user runs `relocal sync pull` to recover. Acceptable.

- **No grace period on last disconnect**: When the last client disconnects, the daemon shuts down immediately after a
  final pull. If the user starts another command a second later, it spawns a fresh daemon. The cost is a few seconds of
  ControlMaster setup plus a no-op push. Not worth the complexity of a grace period timer.

- **No push on second client connect**: The daemon only pushes during initial setup. A second client joining an existing
  session does not trigger a push. This matches the existing design where local edits during a session are not synced to
  the remote.

## Edge cases discovered during implementation

Most of these came out of code review and debugging test hangs. They're worth documenting because the fixes are
non-obvious and easy to accidentally undo during future refactoring.

### Daemon identity is (session, remote), not just session

Two repos can have the same default session name (derived from directory name) but point at different remotes. If the
daemon socket/flock/ControlMaster paths were keyed on session name alone, a client targeting one remote would silently
attach to a daemon managing a completely different remote's working tree. All three path functions hash both `session`
and `remote`.

The local `repo_root` is intentionally _not_ part of the identity. Two local checkouts with the same session name and
remote share the same remote working directory (`~/relocal/<session>/`), so they should share the same daemon. The
initial push comes from whichever checkout spawned the daemon first. Users who need independent remote directories use
different session names.

### The daemon must hold the flock through shutdown

This was the hardest race to get right. The shutdown sequence is: drop listener, final sync pull, remove remote lock,
drop ControlMaster, remove socket file. Between dropping the listener and removing the remote lock, new `connect()`
calls get ECONNREFUSED. A new client sees that, acquires the flock, spawns a fresh daemon — which immediately fails with
StaleSession because the old daemon's remote lock is still there.

The fix: the daemon acquires the flock at the start of shutdown (after the last client disconnects) and holds it through
cleanup. New clients that see ECONNREFUSED block on flock acquisition until the old daemon finishes and exits.

The flock cannot be acquired at daemon _startup_ — that causes a deadlock. The spawning client holds the flock while
waiting for READY. The daemon writes READY, then needs to enter the poll loop to accept the client's connection and send
the control path. If the daemon blocks on flock acquisition before entering the poll loop, the client never gets the
control path, never returns from `connect_or_spawn`, and never releases the flock. Both sides wait on each other
forever. This deadlock was discovered during testing — the daemon would hang silently on launch.

### Blocking streams in the poll loop freeze the entire daemon

The daemon's event loop is single-threaded. When a new client connects, the accepted stream must be set to nonblocking
before it's added to the poll set. If `set_nonblocking` fails and the stream is added anyway, the next `read()` call in
the disconnect-detection path blocks the entire event loop — no other clients get serviced, no syncs run, nothing.

The fix is to reject the client if `set_nonblocking` fails. But `write_all` (sending the control path) is handled
differently: if the write fails, the stream is still added. This sounds wrong but is necessary. If a client connects and
disconnects before the daemon processes the accept, `write_all` fails on the dead stream. If we reject it (don't add to
`clients`), the clients list stays empty and the "last client disconnected" exit condition never fires — the daemon
hangs forever. By adding the dead stream, the next poll iteration detects the EOF and removes it normally, triggering a
clean exit.

This distinction caused a test hang during development. The poll_loop unit test connects a client, drops it, waits 50ms,
then starts the poll loop. When the original code rejected write failures, the poll loop never saw any clients and spun
forever. The fix is subtle enough that both the `write_all` and `set_nonblocking` error handling paths have detailed
inline comments explaining the rationale.

### Spawner death leaves an orphaned daemon

If the process that spawns the daemon dies between writing READY and connecting to the socket, the daemon starts with
zero clients and would run forever — holding the remote lock, preventing any future sessions. The fix is a 30-second
initial-connection timeout: if no client has ever connected within 30 seconds of the daemon starting, it shuts down. The
`ever_had_client` flag ensures this timeout only applies to the initial connection window, not to later gaps between
clients (which are handled by the normal "last client disconnected" exit).

### The handshake read must be line-buffered

The daemon sends the ControlMaster path as a newline-terminated string. The original client code used a single `read()`
into a 256-byte buffer. This works in practice (Unix domain sockets are reliable and the message is ~50 bytes) but
violates the framing protocol — `read()` is not guaranteed to return the complete message in one call. If it returns a
truncated path, the client silently uses a wrong ControlMaster socket path. The fix uses `BufReader::read_line()` which
reads until the newline delimiter.

### Socket and flock paths need length limits

Session names have no length cap, and `$TMPDIR` can be long (especially on macOS: `/var/folders/f7/.../T/`). A naive
`$TMPDIR/rlc-<session>.sock` path overflows the 104-byte Unix socket limit on macOS, causing `UnixListener::bind()` to
fail. All daemon paths (socket, flock, ControlMaster) use the same prefix+hash truncation scheme: up to 20 characters of
the session name for human readability, plus an 8-hex-digit hash for uniqueness. The hash includes a domain tag
(`"daemon-sock"`, `"daemon-flock"`) to prevent collisions between path types for the same session.

### Socket permissions

The daemon socket is created with mode 0600 (owner-only) via an explicit `set_permissions` call after `bind()`. There's
a TOCTOU window between bind and chmod, but on macOS `$TMPDIR` is already per-user (mode 0700), making exploitation
impractical. The flock file is also set to 0600 for consistency. A more thorough fix would involve setting the umask
before bind, but the added complexity isn't justified for the current threat model (single-user workstation).
