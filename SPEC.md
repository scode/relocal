# relocal — Run AI Coding Agents Remotely, Work Locally

## Overview

`relocal` is a CLI tool that lets developers run Claude Code or Codex unsandboxed on a remote Linux box while keeping a
local copy of the repo current. The local repo is pushed to the remote at session start, and a background loop
continuously pulls the agent's changes back to local while the session runs.

The user reviews output locally using authenticated tools (git, Graphite, GitHub CLI, etc.) while the agent runs on a
remote machine without restrictions. Local edits made during an active session are **not** synced to the remote — use
`relocal sync push` between sessions to send local changes.

Distribution and installation of the `relocal` binary itself (e.g., via `cargo install` or binary releases) is out of
scope of this spec.

## Assumptions

- Remote host is Ubuntu (see [Future Improvements](#future-improvements) for broadening OS support).
- User has password-less SSH key access: `ssh user@host` works with no extra arguments.
- User has password-less `sudo` on the remote host.
- Local machine has `rsync` and `ssh` installed.

## Configuration

Configuration comes from two layers, merged with per-field override semantics:

- **User config**: `~/.relocal/config.toml` — user-wide defaults (e.g., a default remote host).
- **Project config**: `relocal.toml` in the repo root — per-repo overrides. Created by `relocal init`.

Both files use the same schema:

```toml
remote = "user@host"

# Additional rsync exclusions (beyond .gitignore).
# .gitignore is always respected. .git/ is always synced.
exclude = [".env", "secrets/"]

# APT packages to install on the remote during `relocal remote install`.
# In addition to the always-installed baseline (see Remote Installation).
apt_packages = ["libssl-dev", "pkg-config"]
```

All fields except `remote` are optional at each layer. The merged result must have `remote`.

### Merge Semantics

For each field, the project config wins if it specifies a value; otherwise the user config's value is used. List fields
(`exclude`, `apt_packages`) are replaced entirely, not concatenated — if a project config specifies `exclude`, it
completely overrides the user-level `exclude`.

### User Config

Located at `~/.relocal/config.toml`. Created manually by the user. If the file does not exist, it is silently skipped.
If it exists but cannot be parsed, relocal exits with an error.

### Project Config Discovery

The repo root is discovered by checking the current working directory for a `relocal.toml` file. Unlike tools that walk
up the directory tree, relocal intentionally only checks the CWD. This prevents accidentally discovering a
`relocal.toml` high in the tree (e.g. in `$HOME`) which would cause the tool to sync an unexpectedly large directory
with `rsync --delete`. If not found, commands that require it fail with an error suggesting `relocal init` or running
from the project root.

## Session Naming

Each session gets its own remote working copy at `~/relocal/<session-name>/`.

The default session name is the local repo directory name. An explicit name can be passed to commands that accept
`[session-name]`.

Session names must contain only alphanumeric characters, hyphens, and underscores. Names containing other characters
(spaces, slashes, dots, etc.) are rejected with an error.

## Concurrent Sessions

Multiple sessions can run simultaneously — different repos to the same remote, or the same repo with different session
names. Each session has its own remote directory and independent sync infrastructure.

Within a single session, multiple tool invocations from the same machine can run concurrently — for example,
`relocal claude` and `relocal codex` against the same session name. They share a single SSH ControlMaster, background
sync loop, and remote lock file via a local session daemon (see [Session Daemon](#session-daemon)). Cross-machine
concurrency against the same session is prevented by the remote lock file.

## CLI Commands

All commands except `init` require a `relocal.toml` in the current directory.

Global flags:

- `-v` / `-vv`: Increase log verbosity (DEBUG / TRACE). Default level is INFO.

### `relocal init`

Interactive command that guides the user to create a `relocal.toml` in the current directory.

Prompts for:

- `remote` (required): `user@host`
- `exclude`: additional rsync exclusion patterns
- `apt_packages`: additional APT packages to install on the remote

Writes the file and confirms.

### `relocal remote install`

Installs the full environment on the remote host. Intended to be run once per remote (or re-run to update). Performs the
following steps in order:

1. **APT baseline packages**: Installs `build-essential`, `git`, `nodejs`, `npm`, plus any packages listed in
   `relocal.toml`'s `apt_packages`.
   ```
   sudo apt-get update && sudo apt-get install -y build-essential git nodejs npm <user-packages>
   ```

2. **Homebrew (Linuxbrew)**: Installs Homebrew if `brew` is not already on PATH. Used as the package manager for tools
   like `gh`.

3. **GitHub CLI**: Installs `gh` via `brew install gh` if not already on PATH.

4. **Rust via rustup**: Installs stable Rust if `rustup` is not already present.
   ```
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
   ```

5. **Claude Code**: Installs via npm if `claude` is not already on PATH.
   ```
   npm install -g @anthropic-ai/claude-code
   ```

6. **Codex CLI**: Installs via npm if `codex` is not already on PATH.
   ```
   npm install -g @openai/codex
   ```

7. **Claude authentication**: Runs `claude login` interactively if Claude is not already authenticated. The user follows
   the normal login flow (supports both API key and subscription-based auth). Since the SSH session has a terminal
   attached, the interactive login works as normal.

8. **Codex authentication**: Runs `codex login --device-auth` interactively if `~/.codex/auth.json` does not exist. The
   device code flow prints a URL and one-time code; the user opens the URL in any browser and enters the code to
   complete authentication. Works over headless SSH sessions.

All steps are idempotent — re-running `relocal remote install` is safe.

### `relocal claude [session-name]`

Main command. Connects to (or spawns) a session daemon, then launches an interactive Claude session on the remote.

Steps:

1. Read `relocal.toml` from the repo root. Fail if not found.
2. Validate the session name (see [Session Naming](#session-naming)).
3. Connect to the session daemon for this session name, or spawn one if none is running (see
   [Session Daemon](#session-daemon)). The daemon owns the SSH ControlMaster, background sync loop, and remote lock
   file.
4. Check that Claude Code is installed on the remote (using the daemon's shared ControlMaster). Fail with a message
   suggesting `relocal remote install` if not found.
5. Open an interactive SSH session (`ssh -t`) to the remote host, `cd` into the working directory, and exec
   `claude --dangerously-skip-permissions`.
6. When the SSH session ends (Claude exits or user quits):
   - Disconnect from the session daemon. If this was the last connected client, the daemon performs a final sync pull
     and tears down (see [Session Daemon — Shutdown](#daemon-shutdown)).
   - Print a summary (session name, remote path, reminder about `sync pull`).

**Signal handling**: `SIGINT` (Ctrl+C) is naturally forwarded to the remote Claude process by the SSH terminal session.
When the SSH session exits (whether from Claude exiting, user quitting, or signal), the client disconnects from the
daemon.

If the SSH session drops unexpectedly (network failure, laptop sleep):

- The main process detects the SSH child process exiting with an error.
- `relocal` prints a message informing the user that the session was interrupted, that there may be unsynchronized work
  on the remote, and that they should use `relocal sync pull` or `relocal sync push` as appropriate to avoid data loss.
  The user must decide the correct action since the tool cannot know the state.
- The daemon connection is closed. If other clients are still connected, the daemon continues running.

### `relocal codex [session-name]`

Identical to `relocal claude` except it launches Codex instead of Claude Code. The remote command is `codex --yolo`
(instead of `claude --dangerously-skip-permissions`). All other behavior — daemon connection, sync, dirty shutdown
handling — is the same. Multiple `relocal claude` and `relocal codex` invocations can run concurrently against the same
session, sharing the daemon's infrastructure.

### `relocal ssh [session-name]`

Opens an interactive SSH shell in the remote session directory (`~/relocal/<session-name>/`). No sync, lock, or
background loop — just a direct `ssh -t` that `cd`s into the directory and execs a login shell.

Can be used alongside a running `relocal claude` or `relocal codex` session to inspect what the agent is doing, make
quick manual edits, or run commands in the same working directory. Also useful between sessions for debugging or general
remote work.

### `relocal sync push [session-name]`

Manual sync: local → remote. Uses the same rsync invocation as the background sync loop.

### `relocal sync pull [session-name]`

Manual sync: remote → local. Uses the same rsync invocation as the background sync loop.

**Safety gate**: Before running rsync, verifies the remote session directory is a valid git repository by running
`git fsck --strict --full --no-dangling` over SSH. If the check fails (remote was destroyed, emptied, corrupted, or is
not a git repo), the pull is refused. This prevents `rsync --delete` from wiping the local working tree. This check also
applies to background-sync-triggered pulls.

### `relocal status [session-name]`

Shows information about the current session:

- Session name (default or configured)
- Remote host
- Remote working directory path
- Whether the remote working directory exists
- Whether Claude is installed on the remote
- Whether Codex is installed on the remote

### `relocal list`

Lists all sessions on the configured remote by listing directories under `~/relocal/`.

Shows each session name and the size of its working copy.

### `relocal destroy [session-name]`

Removes the remote working copy `~/relocal/<session-name>/` and local daemon artifacts (socket, flock, log files in
`$TMPDIR`).

Refuses to proceed if a daemon is running for the session (detected by probing the daemon socket). The user must exit
all active claude/codex/ssh sessions first.

Prompts for confirmation before deleting.

### `relocal log [session-name]`

Tails the daemon log file for the given session. Execs `tail -f` on the log file at `$TMPDIR/rlc-<prefix>-<hash>.log`,
so standard `tail` behavior applies (Ctrl-C to stop).

The daemon writes its tracing output to this file rather than stderr, keeping the client terminal clean during
interactive claude/codex sessions.

### `relocal remote nuke`

Deletes the entire `~/relocal/` directory on the remote, including all sessions. Does **not** uninstall APT packages,
Rust, Claude Code, or Codex.

This is a development/upgrade escape hatch — intended for when developing or upgrading relocal itself and you want a
clean slate to re-run `relocal remote install` and start fresh. Not part of normal workflow.

Prompts for confirmation before deleting.

## Sync Mechanism

### rsync Invocation

All syncs use rsync over SSH. The core invocation:

```
rsync -az --delete --filter=':- .gitignore' \
  --exclude=.claude/ \
  --exclude=<patterns-from-relocal.toml> \
  <source>/ <destination>/
```

Key behaviors:

- `-a`: archive mode (preserves permissions, timestamps, symlinks, etc.)
- `-z`: compression during transfer
- `--delete`: destination mirrors source (files deleted on one side are deleted on the other after sync)
- `--filter=':- .gitignore'`: respects `.gitignore` files at every level of the tree to avoid syncing build artifacts,
  platform-specific binaries, etc.
- `.git/` **is synced** — the remote has full git history.
- `.claude/` is **excluded** — the remote manages its own `.claude/` directory independently. This prevents the
  background sync from overwriting remote Claude state (MCP configs, settings, etc.) with local versions that may
  differ.
- Additional exclusions from `relocal.toml`'s `exclude` array are appended as `--exclude=<pattern>` flags.

### `.claude/` Directory Handling

The `.claude/` directory is **excluded entirely** from rsync in both directions. The remote Claude session manages its
own `.claude/` directory independently.

See [Future Improvements](#future-improvements) for plans to selectively sync `.claude/` subdirectories (skills,
commands, plugins).

### Direction

- **Push** (local → remote): source is the local repo root, destination is `user@host:~/relocal/<session-name>/`.
- **Pull** (remote → local): source is `user@host:~/relocal/<session-name>/`, destination is the local repo root.

### Conflict Handling

During an active session, sync is one-directional: remote → local only. Local edits made while Claude is running will be
overwritten by the next background pull. To send local changes to the remote, end the session and run
`relocal sync push` before starting a new one.

See [Future Improvements](#future-improvements) for plans to support bidirectional sync during sessions.

## SSH Connection Sharing

All SSH and rsync commands during a session share a single persistent SSH connection via OpenSSH's ControlMaster
feature. This avoids the overhead of establishing a new TCP+SSH handshake for every command. The ControlMaster is owned
by the session daemon and shared across all connected clients.

### ControlMaster Setup

The session daemon establishes a ControlMaster at startup:

```
ssh -o ControlMaster=yes -o ControlPath=<socket> -o ControlPersist=300 -N -f <remote>
```

- `-N`: no remote command (just holds the connection open)
- `-f`: backgrounds after connecting
- The socket path is `$TMPDIR/rlc-<prefix>-<hash>` where prefix is up to 20 characters of the session name and hash is
  an 8-hex-digit digest of session name + remote. The fixed-length filename avoids exceeding the 104-byte Unix socket
  path limit on macOS. The path is deterministic per (session, remote) pair (no PID component) so that all clients and
  the daemon for a given session on a given remote use the same socket.

Standalone commands (`relocal sync push/pull`) do not establish a persistent ControlMaster — SSH opens ad-hoc
connections for each rsync/ssh invocation.

### ControlMaster Teardown

When the session daemon shuts down (last client disconnected), the ControlMaster is torn down:

```
ssh -O exit -o ControlPath=<socket> <remote>
```

The teardown is implemented via a `Drop` trait to ensure cleanup even on panic.

### Connection Injection

All `ProcessRunner` commands inject `-o ControlPath=<socket>
-o ControlMaster=auto` into their SSH invocations:

- `run_ssh`: extra args before the remote host argument
- `run_ssh_interactive`: extra args before `-t`
- `run_rsync`: via `-e "ssh -o ControlPath=<socket> -o ControlMaster=auto"` added to the rsync argument list

This is transparent to higher-level code — the `CommandRunner` trait interface is unchanged. Clients receive the
ControlMaster socket path from the daemon during connection handshake and create their own `ProcessRunner` configured
with that path.

## Session Daemon

The session daemon is a local process that owns the shared infrastructure for a session: the SSH ControlMaster,
background sync loop, and remote lock file. It is spawned automatically by the first `relocal claude` or `relocal codex`
invocation for a given session and exits when the last client disconnects.

Daemon identity is keyed on `(session_name, remote)` — not session name alone. Two repos that happen to have the same
session name (e.g., both named `my-project`) but different `remote` values in their `relocal.toml` get independent
daemons with separate ControlMasters, sync loops, and lock files. Without the remote in the key, a client targeting one
remote could attach to a daemon managing a different remote's working tree.

The local `repo_root` is intentionally _not_ part of the daemon identity. Two local checkouts of the same repo with the
same session name and remote share the same remote working directory (`~/relocal/<session>/`), so they should share the
same daemon. The initial push comes from whichever checkout spawned the daemon. Users who need independent remote
directories for different local checkouts should use different session names.

The daemon is tool-agnostic — it does not know whether clients are running Claude, Codex, or any future tool. Tool
installation checks are the client's responsibility.

### Daemon Files

The daemon uses several files in `$TMPDIR`, all following the same prefix+hash naming scheme keyed on
`(session_name, remote)`:

- `.sock` — Unix domain socket for client connections (mode 0600).
- `.flock` — advisory lock file for serializing daemon startup/shutdown (mode 0600).
- `.log` — tracing output (mode 0600). The daemon writes here instead of stderr so interactive sessions aren't cluttered
  with background sync noise. Use `relocal log` to tail it.

All daemon files must be created with mode 0600 (owner-only). Log files contain the remote hostname and session details;
on a shared system with a world-readable `$TMPDIR` (e.g. Linux `/tmp`), permissive defaults would expose this to other
users.

Clients connect to this socket to register their presence. The protocol is minimal:

1. Client connects to the daemon socket.
2. Daemon sends the ControlMaster socket path as a newline-terminated UTF-8 string.
3. The connection stays open for the duration of the client's session. When the client exits (or crashes), the kernel
   closes the fd and the daemon detects the disconnect.

There are no other messages. Client tracking is entirely connection-lifecycle-based.

### Daemon Startup

When `relocal claude` or `relocal codex` starts:

1. Try to connect to the daemon socket. If successful, read the ControlMaster path (with a 30-second read timeout) and
   proceed.
2. If the connection fails (socket does not exist or connection refused): acquire an exclusive `flock(2)` on
   `$TMPDIR/rlc-<prefix>-<hash>.flock` to serialize daemon startup across concurrent invocations.
3. Under the flock, try connecting again — another process may have started the daemon while we waited.
4. If still no daemon: remove any stale socket file at `$TMPDIR/rlc-<prefix>-<hash>.sock`, then spawn
   `relocal _daemon <session-name> <repo-root>` as a subprocess with piped stdout.
5. Wait for the daemon to write `READY\n` to stdout, indicating setup is complete and the socket is accepting
   connections. If the daemon exits before writing READY, report the setup error.
6. Connect to the daemon socket, release the flock.

### Daemon Setup

The daemon performs these steps before accepting clients:

1. Start the SSH ControlMaster (deterministic socket path, no PID).
2. Create the remote working directory.
3. Acquire the remote lock file (atomic via `set -o noclobber`). The remote lock prevents a second machine from starting
   a daemon against the same session — local concurrency is handled by the Unix socket and flock.
4. Perform the initial sync push (local → remote).
5. Bind the Unix domain socket and begin accepting connections.
6. Write `READY\n` to stdout and close it.

### Daemon Main Loop

The daemon runs a single-threaded event loop using `poll(2)`:

- The poll set contains the listener socket fd and all connected client socket fds.
- Poll timeout is 3 seconds (the sync interval).
- On listener activity: accept the new connection, send the ControlMaster path, add the client fd to the poll set. If
  `set_nonblocking` fails on the accepted stream, the stream is rejected (a blocking fd in the poll set would freeze the
  event loop). If `write_all` of the control path fails, the stream is still added — the disconnect will be detected on
  the next poll iteration, which is necessary to keep the "last client left" exit condition correct.
- On client activity (EOF or error): remove the client fd from the poll set. If no clients remain, begin shutdown.
- On timeout: run `sync_pull` (remote → local) if at least one client is connected. Transient failures are logged as
  warnings; the loop continues.

If no client has ever connected within 30 seconds of startup, the daemon shuts down. This prevents the daemon from
running indefinitely if the spawning process dies between writing READY and connecting to the socket.

### Daemon Shutdown

<a id="daemon-shutdown"></a>

When the last client disconnects:

1. Close the listener (stop accepting new connections). New `connect()` calls will get ECONNREFUSED.
2. Acquire the startup flock (`$TMPDIR/rlc-<prefix>-<hash>.flock`). This prevents a race where a new client sees
   ECONNREFUSED, spawns a fresh daemon, and hits the still-present remote lock (StaleSession). The flock cannot be
   acquired at daemon startup because of a deadlock: the spawning client holds the flock while waiting for READY, and
   the daemon can't send the control path (which unblocks the client) until it enters the poll loop, which it can't do
   while blocked on the flock.
3. Perform a final `sync_pull`.
4. Remove the remote lock file.
5. Drop the ControlMaster (tears down the SSH connection).
6. Remove the Unix domain socket file.
7. Exit (releases the flock).

The spawning client starts a background thread to reap the daemon child process, preventing zombie accumulation. The
daemon outlives the spawning client, so synchronous `wait()` is not possible.

If the daemon crashes, the Unix socket file and remote lock file may be left behind. Clients detect a stale socket
(connection refused on an existing file) and clean it up during the startup sequence. The remote lock file requires
manual cleanup via `relocal destroy`.

### `relocal _daemon`

Hidden internal subcommand. Not intended for direct use. Accepts the session name and repo root path as arguments. Reads
`relocal.toml` from the repo root for the remote host and exclusion patterns.

## Background Sync Loop

The background sync loop runs inside the session daemon, pulling remote changes to local on a fixed interval. It is part
of the daemon's poll-based event loop rather than a separate thread.

### Architecture

```
 LOCAL                                    REMOTE
┌──────────────────────────────┐        ┌──────────────────────┐
│ session daemon               │        │                      │
│  ├─ ControlMaster ──────────────(SSH)─│                      │
│  ├─ sync loop (pull) ──────────(rsync)│  ~/relocal/<session> │
│  └─ client tracking          │        │                      │
│       ├─ client 1 (claude)   │        │                      │
│       └─ client 2 (codex)    │        │                      │
│                              │        │                      │
│ relocal claude ──(unix sock)─┤  SSH   │                      │
│   └─ ssh -t ────────────────────────→ │  claude session      │
│                              │        │                      │
│ relocal codex ──(unix sock)──┤  SSH   │                      │
│   └─ ssh -t ────────────────────────→ │  codex session       │
└──────────────────────────────┘        └──────────────────────┘
```

Each poll timeout (3 seconds), the daemon runs `sync_pull` (remote → local) if at least one client is connected. If the
pull fails, it logs a warning and continues — transient rsync failures do not kill the session.

### Trade-offs

The polling approach is less efficient than hook-triggered syncs — it runs rsync even when nothing has changed. However:

- rsync with no changes is cheap (just stat comparisons, no data transfer).
- It eliminates all hook machinery, FIFO management, and remote helper scripts.
- It works with any remote agent (Claude, Codex, etc.) with zero integration.

## Error Handling

- **Background sync failure**: If a pull fails during the daemon's sync loop (e.g., transient network issue), the error
  is logged as a warning and the loop continues. Connected sessions are not interrupted.
- **SSH connection drop**: The client process detects the SSH child process exit and prints a warning with recovery
  instructions (use `relocal sync push`/`pull`). The daemon continues running if other clients are still connected.
- **Daemon crash**: If the daemon crashes while clients are running, they lose the shared ControlMaster. Their SSH
  sessions may break — this is the same failure mode as a network drop. The next invocation detects the stale daemon
  socket (connection refused) and starts a fresh daemon.
- **Daemon spawn failure**: If the daemon fails during setup (ControlMaster, lock, push), it exits without writing the
  readiness signal. The spawning client reports the error.
- **Spawner dies before connecting**: If the process that spawned the daemon dies between READY and socket connect, the
  daemon exits after 30 seconds with no clients (see [Daemon Main Loop](#daemon-main-loop)).
- **Pull safety — remote validation**: Before any remote→local sync (manual or background-loop-triggered),
  `git fsck --strict --full --no-dangling` is run on the remote session directory. If it fails, the pull is refused to
  prevent `rsync --delete` from wiping the local tree.
- **Pull safety — local destination validation**: The command runner validates the local pull target before invoking
  rsync. It canonicalizes the path and verifies that `relocal.toml` exists there. This is a last line of defense against
  a bug in higher-level code passing the wrong `repo_root` to `rsync --delete`. Push does not validate (the destructive
  side of `--delete` on push is the remote, not local).
- **Missing `relocal.toml`**: All commands except `init` fail with a clear error message. Only the current working
  directory is checked (no upward walk).
- **Remote directory does not exist**: `claude`/`codex` creates it. `ssh` fails (the remote `cd` fails and the user sees
  the shell error). `sync` fails (rsync reports the error). `status` reports that the directory does not exist (does not
  fail). `destroy` fails with a message that the session was not found.
- **Tool not installed on remote**: `claude`/`codex` fails with a message suggesting `relocal remote install`.

## Implementation

- Language: Rust
- CLI parsing: `clap` (latest version, derive API)
- SSH/rsync: shell out to `ssh` and `rsync` commands via `std::process::Command` (no SSH library needed)
- Configuration: `toml` crate for parsing `relocal.toml`
- Logging: `tracing` crate. Default level WARN. `-v` gives INFO, `-vv` gives DEBUG, `-vvv` gives TRACE.
- Session daemon event loop: `nix` crate for `poll(2)`. Unix domain sockets via `std::os::unix::net`. File locking via
  `libc::flock`.
- No async runtime needed — the daemon uses a single-threaded poll loop.

## Output and UX

- The default log level is INFO. Client-side progress (connecting, launching, syncing) is logged to stderr via tracing.
  The daemon logs to a file instead (see Daemon Files above).
- In verbose mode (`-v`+), rsync's `--progress` flag is added so the user can see file transfer progress.
- Errors: printed to stderr with context (which operation failed, the remote host, the session name).
- Colors: not required. Plain text output. Can be added as a future improvement.

## Testability

Code and design choices should favor testability. Specifically:

- **Pure logic in isolated modules**: Config parsing, session name validation, and rsync argument construction must be
  pure functions with no I/O dependencies. These are the primary unit test surface.

- **Trait abstraction for command execution**: A `CommandRunner` trait (or equivalent) abstracts shelling out to `ssh`,
  `rsync`, and other external commands. The production implementation uses `std::process::Command`. Test implementations
  record invocations and return configured results. This allows orchestration logic (sync loop, session commands,
  `install`) to be tested without real SSH.

- **Function signatures that enable testing**:
  - Config parsing: `&str` → `Result<Config, Error>`
  - Session name validation: `&str` → `Result<(), Error>`
  - Repo root discovery: `&Path` (CWD) → `Result<PathBuf, Error>` (testable with temp directories; only checks given
    dir, no upward walk)
  - rsync argument construction: `(&Config, Direction, &str)` → `RsyncParams` (carries the argument list plus
    `direction` and `local_path` metadata used by the command runner for safety validation)

## Testing

### Unit Tests

Unit tests cover all pure logic. They do not require SSH, network access, or a remote host.

#### Config Parsing

- Minimal valid config (only `remote` field) parses successfully.
- Full config (all fields populated) parses successfully.
- Missing required `remote` field → error.
- Invalid TOML syntax → error.
- Default values when optional fields are omitted: `exclude` = `[]`, `apt_packages` = `[]`.
- Unknown keys are ignored without error (forward compatibility).

#### Session Name Validation

- Valid names: `my-session`, `session_1`, `foo`, `A-B_C-123`.
- Invalid names: `my session` (space), `a/b` (slash), `a.b` (dot), `../escape` (traversal), empty string.
- Default name derived from directory name (e.g., `/home/user/my-project` → `my-project`).
- Default derivation when directory name contains invalid characters → error with clear message.

#### Repo Root Discovery

- `relocal.toml` in current directory → returns current directory.
- `relocal.toml` only in parent (not CWD) → error (does not walk up).
- No `relocal.toml` in CWD → error.

#### rsync Argument Construction

- Base flags present: `-a`, `-z`, `--delete`.
- `.gitignore` filter rule is included.
- Custom exclude patterns from config are each added as `--exclude=<pattern>`.
- `.claude/` is excluded entirely.
- Source and destination paths are correct for push vs. pull.
- Verbose mode (`-v`+) adds `--progress` to rsync.

#### CLI Argument Parsing

- Each subcommand parses correctly with required and optional arguments.
- Verbosity: no flag → WARN, `-v` → INFO, `-vv` → DEBUG, `-vvv` → TRACE.
- Session name present and absent on commands that accept `[session-name]`.
- `init` does not require `relocal.toml`; all other commands do.

### Integration Tests

Integration tests exercise real SSH, rsync, and filesystem operations.

**Prerequisites**: Integration tests require SSH access to a configured remote host. The remote may be the local machine
(e.g., `localhost`) — the test suite must not assume local and remote are different machines. The user must have
passwordless SSH and passwordless `sudo` on the test remote. Users are responsible for configuring their own
`authorized_keys`; the test suite does not set this up.

The remote host is specified via an environment variable: `RELOCAL_TEST_REMOTE=user@host`. Integration tests are skipped
when this variable is not set.

Each integration test creates a fresh local temporary directory and a unique remote session name, and cleans up both on
completion (including on panic, via `Drop` guard).

#### Sync Push

- Files created locally appear on remote after push.
- Files deleted locally are deleted on remote after push.
- Files matching `.gitignore` are NOT synced.
- Files matching `relocal.toml` `exclude` patterns are NOT synced.
- `.claude/` directory is NOT synced.

#### Sync Pull

- Files created on remote appear locally after pull.
- Files deleted on remote are deleted locally after pull.
- `.claude/` directory is NOT synced.
- `.gitignore`-matching files on remote are not pulled.
- Pull from a non-git remote → refused with error (git fsck safety gate).

#### Background Sync Loop

- Start daemon, create file on remote, wait for one poll cycle, verify file appears locally.
- Sync loop continues after a transient sync failure.
- When the last client disconnects, the daemon shuts down promptly (within one poll cycle).

#### Session Lifecycle

- First `claude`/`codex` invocation spawns daemon, creates remote directory, performs initial push.
- Second concurrent invocation connects to existing daemon, shares the same ControlMaster.
- Disconnecting one client while another is connected does not shut down the daemon.
- On clean exit of last client: final pull is performed, daemon socket is removed.
- `destroy` removes working directory.
- `destroy` on non-existent session → error.

#### `relocal remote install`

- Idempotent: re-run does not fail or corrupt state.
- Each install step is tested for both the already-installed (skip) and absent (install) cases, plus install failure.

#### `relocal list`

- No sessions → empty output.
- Multiple sessions → all listed.

#### `relocal status`

- Reports correct remote host and path.
- Reports whether remote directory exists.
- Reports whether Claude and Codex are installed.

#### `relocal remote nuke`

- Removes entire `~/relocal/` directory.
- After nuke, `list` returns empty, `status` shows directory absent.

#### Localhost-as-Remote

- Push and pull work correctly when the remote is the same machine. The remote working directory
  (`~/relocal/<session>/`) must be distinct from the local temp directory to avoid self-referential rsync.

### Test Infrastructure

- Unit tests live alongside source code in `#[cfg(test)]` modules.
- Integration tests live in `tests/` (Rust integration test directory).
- Integration tests are gated on the `RELOCAL_TEST_REMOTE` environment variable. When unset, integration tests are
  `#[ignore]`d with a message explaining the required setup.
- A shared test utilities module provides helpers for: creating local temp directories, creating and cleaning up remote
  session directories via SSH, and reading/writing remote files.

## Future Improvements

The following are explicitly deferred for simplicity but noted as intended improvements:

- **Conflict handling**: Replace last-write-wins with smarter merge or conflict detection (e.g., checksums before/after,
  user prompts on conflict).
- **OS support beyond Ubuntu**: Currently assumes Ubuntu and APT. Future versions should support other Linux
  distributions and package managers.
- **Sync exclusion of `.git/`**: Evaluate whether syncing only git-tracked content (via `git archive` or similar) is
  preferable to syncing the full `.git/` directory, which can be large.
- **Session persistence**: Detect and reattach to a running remote Claude session after a network drop, rather than
  requiring a fresh start.
- **Automatic reconnection**: Retry SSH on transient network failures.
- **`.claude/` directory syncing**: The entire `.claude/` directory is currently excluded from rsync. A future version
  should selectively sync `.claude/` subdirectories (skills, commands, plugins) while keeping settings and MCP configs
  managed independently per side.
- **Colored output**: Add color support for better UX.
- **Efficient sync**: Replace polling with file-watching (e.g., inotify/fsevents) on the remote to only sync when files
  actually change.
