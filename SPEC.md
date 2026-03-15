# relocal — Run Claude Code Remotely, Work Locally

## Overview

`relocal` is a CLI tool that lets developers run Claude Code unsandboxed on a
remote Linux box while keeping a local copy of the repo current. The local repo
is pushed to the remote at session start, and a background loop continuously
pulls Claude's changes back to local while the session runs.

The user reviews Claude's output locally using authenticated tools (git,
Graphite, GitHub CLI, etc.) while Claude Code runs on a remote machine without
restrictions. Local edits made during an active session are **not** synced to
the remote — use `relocal sync push` between sessions to send local changes.

Distribution and installation of the `relocal` binary itself (e.g., via
`cargo install` or binary releases) is out of scope of this spec.

## Assumptions

- Remote host is Ubuntu (see [Future Improvements](#future-improvements) for
  broadening OS support).
- User has password-less SSH key access: `ssh user@host` works with no extra
  arguments.
- User has password-less `sudo` on the remote host.
- Local machine has `rsync` and `ssh` installed.

## Configuration

A single `relocal.toml` file in the repo root. Created by `relocal init`.

The repo root is discovered by checking the current working directory for a
`relocal.toml` file. Unlike tools that walk up the directory tree, relocal
intentionally only checks the CWD. This prevents accidentally discovering a
`relocal.toml` high in the tree (e.g. in `$HOME`) which would cause the tool to
sync an unexpectedly large directory with `rsync --delete`. If not found,
commands that require it fail with an error suggesting `relocal init` or running
from the project root.

```toml
remote = "user@host"

# Additional rsync exclusions (beyond .gitignore).
# .gitignore is always respected. .git/ is always synced.
exclude = [".env", "secrets/"]

# APT packages to install on the remote during `relocal remote install`.
# In addition to the always-installed baseline (see Remote Installation).
apt_packages = ["libssl-dev", "pkg-config"]
```

All fields except `remote` are optional.

## Session Naming

Each session gets its own remote working copy at `~/relocal/<session-name>/`.

The default session name is the local repo directory name. An explicit name can
be passed to commands that accept `[session-name]`.

Session names must contain only alphanumeric characters, hyphens, and
underscores. Names containing other characters (spaces, slashes, dots, etc.) are
rejected with an error.

## Concurrent Sessions

Multiple sessions can run simultaneously — different repos to the same remote,
or the same repo with different session names. Each session has its own remote
directory and independent background sync loop.

## CLI Commands

All commands except `init` require a `relocal.toml` in the current directory.

Global flags:
- `-v` / `-vv` / `-vvv`: Increase log verbosity (INFO / DEBUG / TRACE).
  Default level is WARN.

### `relocal init`

Interactive command that guides the user to create a `relocal.toml` in the
current directory.

Prompts for:
- `remote` (required): `user@host`
- `exclude`: additional rsync exclusion patterns
- `apt_packages`: additional APT packages to install on the remote

Writes the file and confirms.

### `relocal remote install`

Installs the full environment on the remote host. Intended to be run once per
remote (or re-run to update). Performs the following steps in order:

1. **APT baseline packages**: Installs `build-essential`, `git`, `nodejs`,
   `npm`, plus any packages listed in `relocal.toml`'s `apt_packages`.
   ```
   sudo apt-get update && sudo apt-get install -y build-essential git nodejs npm <user-packages>
   ```

2. **Homebrew (Linuxbrew)**: Installs Homebrew if `brew` is not already on PATH.
   Used as the package manager for tools like `gh`.

3. **GitHub CLI**: Installs `gh` via `brew install gh` if not already on PATH.

4. **Rust via rustup**: Installs stable Rust if `rustup` is not already present.
   ```
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
   ```

5. **Claude Code**: Installs via npm if `claude` is not already on PATH.
   ```
   npm install -g @anthropic-ai/claude-code
   ```

6. **Claude authentication**: Runs `claude login` interactively if Claude is not
   already authenticated. The user follows the normal login flow (supports both
   API key and subscription-based auth). Since the SSH session has a terminal
   attached, the interactive login works as normal.

All steps are idempotent — re-running `relocal remote install` is safe.

### `relocal claude [session-name]`

Main command. Syncs the repo to the remote and launches an interactive Claude
session with continuous background synchronization.

Steps:

1. Read `relocal.toml` from the repo root. Fail if not found.
2. Validate the session name (see [Session Naming](#session-naming)).
3. Establish an SSH ControlMaster connection (see
   [SSH Connection Sharing](#ssh-connection-sharing)). All subsequent SSH and
   rsync commands in this session reuse this connection.
4. Check that Claude Code is installed on the remote. Fail with a message
   suggesting `relocal remote install` if not found.
5. Create the remote working directory `~/relocal/<session-name>/` if it does
   not exist.
6. Perform an initial sync: local → remote (push).
7. Start the background sync loop (see
   [Background Sync Loop](#background-sync-loop)).
8. Open an interactive SSH session (`ssh -t`) to the remote host, `cd` into the
   working directory, and exec `claude --dangerously-skip-permissions`.
9. When the SSH session ends (Claude exits or user quits):
   - Shut down the background sync loop.
   - If Claude exited cleanly (exit code 0), perform a final sync:
     remote → local (pull).
   - Tear down the ControlMaster connection.
   - Print a summary (session name, remote path, reminder about `sync pull`).

**Signal handling**: `SIGINT` (Ctrl+C) is naturally forwarded to the remote
Claude process by the SSH terminal session. When the SSH session exits (whether
from Claude exiting, user quitting, or signal), `relocal` proceeds with
cleanup: the background sync is shut down and the ControlMaster is torn down.

If the SSH session drops unexpectedly (network failure, laptop sleep):
- The main process detects the SSH child process exiting with an error.
- The background sync loop is shut down.
- `relocal` prints a message informing the user that the session was
  interrupted, that there may be unsynchronized work on the remote, and that
  they should use `relocal sync pull` or `relocal sync push` as appropriate to
  avoid data loss. The user must decide the correct action since the tool cannot
  know the state.

### `relocal ssh [session-name]`

Opens an interactive SSH shell in the remote session directory
(`~/relocal/<session-name>/`). No sync, lock, or background loop — just a
direct `ssh -t` that `cd`s into the directory and execs a login shell.

Can be used alongside a running `relocal claude` session to inspect what the
agent is doing, make quick manual edits, or run commands in the same working
directory. Also useful between sessions for debugging or general remote work.

### `relocal sync push [session-name]`

Manual sync: local → remote. Uses the same rsync invocation as the background
sync loop.

### `relocal sync pull [session-name]`

Manual sync: remote → local. Uses the same rsync invocation as the background
sync loop.

**Safety gate**: Before running rsync, verifies the remote session directory is a
valid git repository by running `git fsck --strict --full --no-dangling` over
SSH. If the check fails (remote was destroyed, emptied, corrupted, or is not a
git repo), the pull is refused. This prevents `rsync --delete` from wiping the
local working tree. This check also applies to background-sync-triggered pulls.

### `relocal status [session-name]`

Shows information about the current session:
- Session name (default or configured)
- Remote host
- Remote working directory path
- Whether the remote working directory exists
- Whether Claude is installed on the remote

### `relocal list`

Lists all sessions on the configured remote by listing directories under
`~/relocal/`.

Shows each session name and the size of its working copy.

### `relocal destroy [session-name]`

Removes the remote working copy `~/relocal/<session-name>/`.

Prompts for confirmation before deleting.

### `relocal remote nuke`

Deletes the entire `~/relocal/` directory on the remote, including all sessions.
Does **not** uninstall APT packages, Rust, or Claude Code.

This is a development/upgrade escape hatch — intended for when developing or
upgrading relocal itself and you want a clean slate to re-run
`relocal remote install` and start fresh. Not part of normal workflow.

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
- `--delete`: destination mirrors source (files deleted on one side are deleted
  on the other after sync)
- `--filter=':- .gitignore'`: respects `.gitignore` files at every level of the
  tree to avoid syncing build artifacts, platform-specific binaries, etc.
- `.git/` **is synced** — the remote has full git history.
- `.claude/` is **excluded** — the remote manages its own `.claude/` directory
  independently. This prevents the background sync from overwriting remote
  Claude state (MCP configs, settings, etc.) with local versions that may
  differ.
- Additional exclusions from `relocal.toml`'s `exclude` array are appended as
  `--exclude=<pattern>` flags.

### `.claude/` Directory Handling

The `.claude/` directory is **excluded entirely** from rsync in both directions.
The remote Claude session manages its own `.claude/` directory independently.

See [Future Improvements](#future-improvements) for plans to selectively sync
`.claude/` subdirectories (skills, commands, plugins).

### Direction

- **Push** (local → remote): source is the local repo root, destination is
  `user@host:~/relocal/<session-name>/`.
- **Pull** (remote → local): source is
  `user@host:~/relocal/<session-name>/`, destination is the local repo root.

### Conflict Handling

During an active session, sync is one-directional: remote → local only. Local
edits made while Claude is running will be overwritten by the next background
pull. To send local changes to the remote, end the session and run
`relocal sync push` before starting a new one.

See [Future Improvements](#future-improvements) for plans to support
bidirectional sync during sessions.

## SSH Connection Sharing

All SSH and rsync commands during a `relocal claude` session share a single
persistent SSH connection via OpenSSH's ControlMaster feature. This avoids the
overhead of establishing a new TCP+SSH handshake for every command.

### ControlMaster Setup

At the start of a `relocal claude` session, a ControlMaster is established:

```
ssh -o ControlMaster=yes -o ControlPath=<socket> -o ControlPersist=300 -N -f <remote>
```

- `-N`: no remote command (just holds the connection open)
- `-f`: backgrounds after connecting
- The socket path is `$TMPDIR/rlc-<prefix>-<hash>` where prefix is up to 20
  characters of the session name and hash is an 8-hex-digit digest of
  session+PID. The fixed-length filename avoids exceeding the 104-byte Unix
  socket path limit on macOS.

### ControlMaster Teardown

On session end (clean or dirty), the ControlMaster is torn down:

```
ssh -O exit -o ControlPath=<socket> <remote>
```

The teardown is implemented via a `Drop` trait to ensure cleanup even on panic.

### Connection Injection

All `ProcessRunner` commands inject `-o ControlPath=<socket>
-o ControlMaster=auto` into their SSH invocations:

- `run_ssh`: extra args before the remote host argument
- `run_ssh_interactive`: extra args before `-t`
- `run_rsync`: via `-e "ssh -o ControlPath=<socket> -o ControlMaster=auto"`
  added to the rsync argument list

This is transparent to higher-level code — the `CommandRunner` trait interface
is unchanged.

## Background Sync Loop

The background sync loop replaces the previous hook-triggered sidecar. It runs
as a local background thread managed by `relocal claude`.

### Architecture

```
 LOCAL                          REMOTE
┌─────────────┐    SSH #1     ┌──────────────────────┐
│ relocal     │───(ssh -t)───→│ claude session       │
│             │               │                      │
│ sync loop   │    rsync      │                      │
│  └─ pull ───│──────────────→│  (every ~3 seconds)  │
│             │               │                      │
└─────────────┘               └──────────────────────┘
         (all via ControlMaster)
```

The loop thread uses `mpsc::recv_timeout` on a shutdown channel. Each iteration:

1. Wait up to N seconds (currently 3) for a shutdown signal.
2. If shutdown received (or sender dropped): exit the loop.
3. Otherwise (timeout): run `sync_pull` (remote → local).
4. If the pull fails, log a warning and continue — transient rsync failures
   should not kill the session.

### Shutdown

The main thread shuts down the sync loop by dropping the channel sender, which
causes `recv_timeout` to return immediately. The main thread then joins the
background thread. This gives sub-millisecond shutdown latency (no need to
wait for a sleep cycle).

### Trade-offs

The polling approach is less efficient than hook-triggered syncs — it runs rsync
even when nothing has changed. However:
- rsync with no changes is cheap (just stat comparisons, no data transfer).
- It eliminates all hook machinery, FIFO management, and remote helper scripts.
- It works with any remote agent (Claude, Codex, etc.) with zero integration.

## Error Handling

- **Background sync failure**: If a pull fails during the background loop (e.g.,
  transient network issue), the error is logged as a warning and the loop
  continues. The session is not interrupted.
- **SSH connection drop**: The main process detects the SSH child process exit,
  shuts down the background sync, tears down the ControlMaster (best-effort),
  and prints a warning with recovery instructions (use `relocal sync
  push`/`pull`).
- **Pull safety — remote validation**: Before any remote→local sync (manual or
  background-loop-triggered), `git fsck --strict --full --no-dangling` is run on
  the remote session directory. If it fails, the pull is refused to prevent
  `rsync --delete` from wiping the local tree.
- **Pull safety — local destination validation**: The command runner validates
  the local pull target before invoking rsync. It canonicalizes the path and
  verifies that `relocal.toml` exists there. This is a last line of defense
  against a bug in higher-level code passing the wrong `repo_root` to
  `rsync --delete`. Push does not validate (the destructive side of `--delete`
  on push is the remote, not local).
- **Missing `relocal.toml`**: All commands except `init` fail with a clear error
  message. Only the current working directory is checked (no upward walk).
- **Remote directory does not exist**: `claude` creates it. `ssh` fails (the
  remote `cd` fails and the user sees the shell error). `sync` fails (rsync
  reports the error). `status` reports that the directory does not exist (does
  not fail). `destroy` fails with a message that the session was not found.
- **Claude not installed on remote**: `claude` fails with a message suggesting
  `relocal remote install`.

## Implementation

- Language: Rust
- CLI parsing: `clap` (latest version, derive API)
- SSH/rsync: shell out to `ssh` and `rsync` commands via `std::process::Command`
  (no SSH library needed)
- Configuration: `toml` crate for parsing `relocal.toml`
- Logging: `tracing` crate. Default level WARN. `-v` gives INFO, `-vv` gives
  DEBUG, `-vvv` gives TRACE.
- No async runtime needed — the background sync loop uses a single thread with
  `mpsc::recv_timeout` for clean shutdown.

## Output and UX

- Progress: rsync is run with default output. In verbose mode (`-v`+), rsync's
  `--progress` flag is added so the user can see file transfer progress.
- Errors: printed to stderr with context (which operation failed, the remote
  host, the session name).
- Colors: not required. Plain text output. Can be added as a future improvement.

## Testability

Code and design choices should favor testability. Specifically:

- **Pure logic in isolated modules**: Config parsing, session name validation,
  and rsync argument construction must be pure functions with no I/O
  dependencies. These are the primary unit test surface.

- **Trait abstraction for command execution**: A `CommandRunner` trait (or
  equivalent) abstracts shelling out to `ssh`, `rsync`, and other external
  commands. The production implementation uses `std::process::Command`. Test
  implementations record invocations and return configured results. This allows
  orchestration logic (sync loop, `claude`, `install`) to be tested without real
  SSH.

- **Function signatures that enable testing**:
  - Config parsing: `&str` → `Result<Config, Error>`
  - Session name validation: `&str` → `Result<(), Error>`
  - Repo root discovery: `&Path` (CWD) → `Result<PathBuf, Error>`
    (testable with temp directories; only checks given dir, no upward walk)
  - rsync argument construction: `(&Config, Direction, &str)` →
    `RsyncParams` (carries the argument list plus `direction` and `local_path`
    metadata used by the command runner for safety validation)

## Testing

### Unit Tests

Unit tests cover all pure logic. They do not require SSH, network access, or a
remote host.

#### Config Parsing

- Minimal valid config (only `remote` field) parses successfully.
- Full config (all fields populated) parses successfully.
- Missing required `remote` field → error.
- Invalid TOML syntax → error.
- Default values when optional fields are omitted: `exclude` = `[]`,
  `apt_packages` = `[]`.
- Unknown keys are ignored without error (forward compatibility).

#### Session Name Validation

- Valid names: `my-session`, `session_1`, `foo`, `A-B_C-123`.
- Invalid names: `my session` (space), `a/b` (slash), `a.b` (dot),
  `../escape` (traversal), empty string.
- Default name derived from directory name
  (e.g., `/home/user/my-project` → `my-project`).
- Default derivation when directory name contains invalid characters → error
  with clear message.

#### Repo Root Discovery

- `relocal.toml` in current directory → returns current directory.
- `relocal.toml` only in parent (not CWD) → error (does not walk up).
- No `relocal.toml` in CWD → error.

#### rsync Argument Construction

- Base flags present: `-a`, `-z`, `--delete`.
- `.gitignore` filter rule is included.
- Custom exclude patterns from config are each added as
  `--exclude=<pattern>`.
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

**Prerequisites**: Integration tests require SSH access to a configured remote
host. The remote may be the local machine (e.g., `localhost`) — the test suite
must not assume local and remote are different machines. The user must have
passwordless SSH and passwordless `sudo` on the test remote. Users are
responsible for configuring their own `authorized_keys`; the test suite does not
set this up.

The remote host is specified via an environment variable:
`RELOCAL_TEST_REMOTE=user@host`. Integration tests are skipped when this
variable is not set.

Each integration test creates a fresh local temporary directory and a unique
remote session name, and cleans up both on completion (including on panic, via
`Drop` guard).

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

- Start background loop, create file on remote, wait for one poll cycle, verify
  file appears locally.
- Background loop continues after a transient sync failure.
- Shutdown signal stops the loop promptly (within one poll cycle).

#### Session Lifecycle

- `claude` creates remote directory, performs initial push.
- On clean exit: final pull is performed, summary is printed.
- `destroy` removes working directory.
- `destroy` on non-existent session → error.

#### `relocal remote install`

- Idempotent: re-run does not fail or corrupt state.
- APT/rustup/Claude install steps are tested only for the already-installed
  case (verifying idempotency without uninstalling).

#### `relocal list`

- No sessions → empty output.
- Multiple sessions → all listed.

#### `relocal status`

- Reports correct remote host and path.
- Reports whether remote directory exists.

#### `relocal remote nuke`

- Removes entire `~/relocal/` directory.
- After nuke, `list` returns empty, `status` shows directory absent.

#### Localhost-as-Remote

- Push and pull work correctly when the remote is the same machine. The remote
  working directory (`~/relocal/<session>/`) must be distinct from the local
  temp directory to avoid self-referential rsync.

### Test Infrastructure

- Unit tests live alongside source code in `#[cfg(test)]` modules.
- Integration tests live in `tests/` (Rust integration test directory).
- Integration tests are gated on the `RELOCAL_TEST_REMOTE` environment
  variable. When unset, integration tests are `#[ignore]`d with a message
  explaining the required setup.
- A shared test utilities module provides helpers for: creating local temp
  directories, creating and cleaning up remote session directories via SSH,
  and reading/writing remote files.

## Future Improvements

The following are explicitly deferred for simplicity but noted as intended
improvements:

- **Conflict handling**: Replace last-write-wins with smarter merge or conflict
  detection (e.g., checksums before/after, user prompts on conflict).
- **OS support beyond Ubuntu**: Currently assumes Ubuntu and APT. Future versions
  should support other Linux distributions and package managers.
- **Sync exclusion of `.git/`**: Evaluate whether syncing only git-tracked
  content (via `git archive` or similar) is preferable to syncing the full
  `.git/` directory, which can be large.
- **Session persistence**: Detect and reattach to a running remote Claude session
  after a network drop, rather than requiring a fresh start.
- **Automatic reconnection**: Retry SSH on transient network failures.
- **`.claude/` directory syncing**: The entire `.claude/` directory is currently
  excluded from rsync. A future version should selectively sync `.claude/`
  subdirectories (skills, commands, plugins) while keeping settings and MCP
  configs managed independently per side.
- **Colored output**: Add color support for better UX.
- **Efficient sync**: Replace polling with file-watching (e.g., inotify/fsevents)
  on the remote to only sync when files actually change.
