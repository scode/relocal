# relocal — Run Claude Code Remotely, Work Locally

## Overview

`relocal` is a CLI tool that lets developers run Claude Code unsandboxed on a
remote Linux box while maintaining a seamless local workflow. The local repo is
synchronized bidirectionally with a remote working copy via rsync. Claude hooks
on the remote trigger syncs so that local state stays current.

The user interacts with their repo locally using authenticated tools (git,
Graphite, GitHub CLI, etc.) and offloads Claude Code execution to a remote
machine where it can run without restrictions.

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

# Which subdirectories of .claude/ to sync. Defaults shown below.
claude_sync_dirs = ["skills", "commands", "plugins"]
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
or the same repo with different session names. The per-session FIFO naming
ensures no interference between sessions.

## CLI Commands

All commands except `init` require a `relocal.toml` to be found by walking up
from the current directory.

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

1. **APT baseline packages**: Installs `build-essential`, `nodejs`, `npm`, plus
   any packages listed in `relocal.toml`'s `apt_packages`.
   ```
   sudo apt-get update && sudo apt-get install -y build-essential nodejs npm <user-packages>
   ```

2. **Rust via rustup**: Installs stable Rust if `rustup` is not already present.
   ```
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
   ```

3. **Claude Code**: Installs via npm if `claude` is not already on PATH.
   ```
   npm install -g @anthropic-ai/claude-code
   ```

4. **Claude authentication**: Runs `claude login` interactively if Claude is not
   already authenticated. The user follows the normal login flow (supports both
   API key and subscription-based auth). Since the SSH session has a terminal
   attached, the interactive login works as normal.

5. **Hook helper script**: Installs `~/relocal/.bin/relocal-hook.sh` (see
   [Hook Mechanism](#hook-mechanism) for details). Always overwritten on
   re-runs to ensure the latest version is deployed.

6. **FIFO directory**: Creates `~/relocal/.fifos/` for the sync signaling
   mechanism.

All steps are idempotent — re-running `relocal remote install` is safe.

### `relocal start [session-name]`

Main command. Syncs the repo to the remote and launches an interactive Claude
session.

**Stale session detection**: Before proceeding, checks whether FIFOs already
exist for this session name. If they do, assumes a previous session is still
running or crashed. Prints an error telling the user to either wait for the
existing session to finish, or run `relocal destroy <session-name>` if the
previous session crashed. Does not proceed.

Steps:

1. Read `relocal.toml` from the repo root. Fail if not found.
2. Validate the session name (see [Session Naming](#session-naming)).
3. Check for stale FIFOs (see above). Fail if found.
4. Check that Claude Code is installed on the remote. Fail with a message
   suggesting `relocal remote install` if not found.
5. Create the remote working directory `~/relocal/<session-name>/` if it does
   not exist.
6. Create session-specific FIFOs on the remote (see [Sync Sidecar](#sync-sidecar)):
   - `~/relocal/.fifos/<session-name>-request`
   - `~/relocal/.fifos/<session-name>-ack`
7. Perform an initial sync: local → remote (push).
8. Install Claude project-level hooks in the remote working copy's
   `.claude/settings.json` (see [Hook Mechanism](#hook-mechanism)).
9. Start the sync sidecar (local background process, see
   [Sync Sidecar](#sync-sidecar)).
10. Open an interactive SSH session (`ssh -t`) to the remote host, `cd` into the
    working directory, and exec `claude --dangerously-skip-permissions`.
11. When the SSH session ends (Claude exits or user quits):
    - Shut down the sync sidecar.
    - Clean up the remote FIFOs.
    - Print a summary (session name, remote path, reminder about `sync pull`).

**Signal handling**: `SIGINT` (Ctrl+C) is naturally forwarded to the remote
Claude process by the SSH terminal session. When the SSH session exits (whether
from Claude exiting, user quitting, or signal), `relocal` proceeds with
cleanup: the sidecar is shut down and FIFOs are removed. The sidecar is
terminated by the main process (e.g., via thread join or process signal) — it
does not need to independently detect the session end.

If the SSH session drops unexpectedly (network failure, laptop sleep):
- The main process detects the SSH child process exiting with an error.
- The sidecar is shut down.
- FIFOs are cleaned up on a best-effort basis (may fail if network is down).
- `relocal` prints a message informing the user that the session was
  interrupted, that there may be unsynchronized work on the remote, and that
  they should use `relocal sync pull` or `relocal sync push` as appropriate to
  avoid data loss. The user must decide the correct action since the tool cannot
  know the state.

### `relocal sync push [session-name]`

Manual sync: local → remote. Uses the same rsync invocation as the sidecar.
After rsync completes, re-injects hooks into the remote `.claude/settings.json`
(since the push may have overwritten it).

### `relocal sync pull [session-name]`

Manual sync: remote → local. Uses the same rsync invocation as the sidecar.

**Safety gate**: Before running rsync, verifies the remote session directory is a
valid git repository by running `git fsck --strict --full --no-dangling` over
SSH. If the check fails (remote was destroyed, emptied, corrupted, or is not a
git repo), the pull is refused. This prevents `rsync --delete` from wiping the
local working tree. This check also applies to sidecar-triggered pulls.

### `relocal status [session-name]`

Shows information about the current session:
- Session name (default or configured)
- Remote host
- Remote working directory path
- Whether the remote working directory exists
- Whether Claude is installed on the remote
- Whether FIFOs exist (session appears active)

### `relocal list`

Lists all sessions on the configured remote by listing directories under
`~/relocal/` (excluding `.bin/` and `.fifos/`).

Shows each session name and the size of its working copy.

### `relocal destroy [session-name]`

Removes the remote working copy `~/relocal/<session-name>/` and any associated
FIFOs.

Prompts for confirmation before deleting.

### `relocal remote nuke`

Deletes the entire `~/relocal/` directory on the remote, including all sessions,
FIFOs, and the hook helper script. Does **not** uninstall APT packages, Rust,
or Claude Code.

This is a development/upgrade escape hatch — intended for when developing or
upgrading relocal itself and you want a clean slate to re-run
`relocal remote install` and start fresh. Not part of normal workflow.

Prompts for confirmation before deleting.

## Sync Mechanism

### rsync Invocation

All syncs use rsync over SSH. The core invocation:

```
rsync -az --delete --filter=':- .gitignore' \
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
- Additional exclusions from `relocal.toml`'s `exclude` array are appended as
  `--exclude=<pattern>` flags.

### `.claude/` Directory Handling

The `.claude/` directory requires special treatment because it contains both
user configuration that should sync and session-specific state that should not.

**Synced subdirectories** (configurable via `claude_sync_dirs`, defaults:
`skills`, `commands`, `plugins`):
- These are synced bidirectionally like any other files.

**`settings.json` handling**:
- **Push** (local → remote): `settings.json` is synced normally (local
  overwrites remote). After the push completes, relocal re-injects the hook
  configuration into the remote's `settings.json` (see
  [Hook Mechanism](#hook-mechanism)).
- **Pull** (remote → local): `settings.json` is **excluded** from the pull.
  The remote copy contains relocal hooks that are not meaningful locally.

**Everything else in `.claude/`** (conversations, cache, etc.): excluded from
sync in both directions.

This is implemented by excluding `.claude/` wholesale from rsync, then
explicitly including the configured subdirectories and `settings.json` (push
only).

**Known limitation**: Changes to `.claude/settings.json` on the remote (e.g.,
made by Claude itself) are not synced back to local. See
[Future Improvements](#future-improvements).

### Direction

- **Push** (local → remote): source is the local repo root, destination is
  `user@host:~/relocal/<session-name>/`.
- **Pull** (remote → local): source is
  `user@host:~/relocal/<session-name>/`, destination is the local repo root.

### Conflict Handling

rsync uses last-write-wins. If the user edits a file locally while Claude edits
the same file remotely, whichever sync runs last will overwrite the other's
changes. See [Future Improvements](#future-improvements).

## Hook Mechanism

Claude project-level hooks are installed in the remote working copy at
`.claude/settings.json` within the `hooks` key.

Two hooks are configured:

### `UserPromptSubmit` (sync local → remote)

Fires when the user submits a prompt, before Claude begins processing. The hook
runs the helper script which triggers a push sync (local → remote) so that
Claude works on the latest local state.

### `Stop` (sync remote → local)

Fires when Claude finishes a response. The hook runs the helper script which
triggers a pull sync (remote → local) so the user sees Claude's changes
locally.

### Hook Helper Script

Installed at `~/relocal/.bin/relocal-hook.sh` on the remote.

```bash
#!/bin/bash
set -euo pipefail
```

Accepts a single argument: the sync direction (`push` or `pull`).

Behavior:
1. Write the direction (`push` or `pull`) to the session's request FIFO.
   This blocks until the sidecar reads it.
2. Read from the session's ack FIFO. This blocks until the sidecar writes a
   response.
3. If the ack is `ok`, exit 0 (Claude proceeds).
4. If the ack is `error:<message>`, write the error to stderr and exit 1
   (Claude is blocked from proceeding).

The script receives the session name via an environment variable
(`RELOCAL_SESSION`) set in the hook configuration.

### Hook Configuration

The `.claude/settings.json` in the remote working copy will contain:

```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "RELOCAL_SESSION=<session-name> ~/relocal/.bin/relocal-hook.sh push"
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "RELOCAL_SESSION=<session-name> ~/relocal/.bin/relocal-hook.sh pull"
          }
        ]
      }
    ]
  }
}
```

### Hook Merging Strategy

The hook installation must handle existing `.claude/settings.json` content:

- If the file does not exist, create it with the hook configuration.
- If the file exists but has no `hooks` key, add the `hooks` key.
- If `hooks` exists but has no `UserPromptSubmit` or `Stop` arrays, add them.
- If the arrays exist, check for existing relocal matcher groups. Relocal
  matcher groups are identified by the presence of `relocal-hook.sh` in any
  command string within the group's `hooks` array.
  - If a relocal matcher group exists, update it in place (to handle session
    name or script path changes).
  - If no relocal matcher group exists, append a new one to the array.

This ensures user-defined hooks in the same arrays are preserved, and repeated
runs of `relocal start` or `relocal sync push` do not duplicate entries.

## Sync Sidecar

The sidecar is a local background process managed by `relocal start`. It
mediates all syncs triggered by remote hooks.

### Architecture

```
 LOCAL                          REMOTE
┌─────────────┐    SSH #1     ┌──────────────────────┐
│ relocal     │───(ssh -t)───→│ claude session       │
│ start       │               │                      │
│             │    SSH #2     │  hook fires →        │
│ sidecar  ←──│───(cat fifo)──│  writes to req fifo  │
│  │          │               │  blocks on ack fifo  │
│  ├─ rsync ──│───(ssh)──────→│                      │
│  │          │    SSH #3     │                      │
│  └─ ack  ───│───(echo>fifo)→│  reads ack, exits    │
└─────────────┘               └──────────────────────┘
```

1. The sidecar opens an SSH connection to the remote, reading from the request
   FIFO: `ssh user@host "cat ~/relocal/.fifos/<session>-request"`.
   Each time the hook writes a line, the sidecar receives it.

   **Implementation note**: A FIFO will deliver EOF to the reader once all
   writers close it. Since each hook invocation opens the FIFO, writes, and
   closes it, the `cat` command will exit after each request. The
   implementation must account for this — e.g., by wrapping the read in a
   loop (`while true; do cat <fifo>; done` on the remote, or by reopening
   the SSH read connection after each request on the local side).

2. On receiving a request (`push` or `pull`):
   a. Run the appropriate rsync command **locally**.
   b. If this was a push, re-inject hooks into the remote
      `.claude/settings.json` (since the push may have overwritten it).
   c. On success: write `ok` to the ack FIFO via
      `ssh user@host "echo ok > ~/relocal/.fifos/<session>-ack"`.
   d. On failure: write `error:<message>` to the ack FIFO via
      `ssh user@host "echo 'error:<message>' > ~/relocal/.fifos/<session>-ack"`.

3. The hook script on the remote blocks on the ack FIFO read, receives the
   result, and either lets Claude proceed or blocks with an error.

4. When the main SSH session (claude) ends, the main `relocal` process
   terminates the sidecar thread/process and cleans up FIFOs.

### FIFO Lifecycle

- Created by `relocal start` before launching Claude.
- Removed by `relocal start` on clean shutdown.
- On dirty shutdown (network drop), FIFOs may be left behind. `relocal start`
  checks for existing FIFOs and refuses to start if they exist (see
  [Stale session detection](#relocal-start-session-name)).

## Error Handling

- **rsync failure during hook**: The hook script receives an error ack and exits
  non-zero. Claude is blocked from proceeding. The error is printed to stderr
  which will be visible in the Claude session.
- **SSH connection drop**: The main process detects the SSH child process exit,
  terminates the sidecar, attempts FIFO cleanup, and prints a warning with
  recovery instructions (use `relocal sync push`/`pull`).
- **Pull safety**: Before any remote→local sync (manual or sidecar-triggered),
  `git fsck --strict --full --no-dangling` is run on the remote session
  directory. If it fails, the pull is refused to prevent `rsync --delete` from
  wiping the local tree.
- **Missing `relocal.toml`**: All commands except `init` fail with a clear error
  message. Only the current working directory is checked (no upward walk).
- **Remote directory does not exist**: `start` creates it. `sync` fails (rsync
  reports the error). `status` reports that the directory does not exist (does
  not fail). `destroy` fails with a message that the session was not found.
- **Claude not installed on remote**: `start` fails with a message suggesting
  `relocal remote install`.
- **Stale FIFOs on start**: `start` refuses to proceed and instructs the user
  to check for an existing session or run `relocal destroy`.

## Implementation

- Language: Rust
- CLI parsing: `clap` (latest version, derive API)
- SSH/rsync: shell out to `ssh` and `rsync` commands via `std::process::Command`
  (no SSH library needed)
- Configuration: `toml` crate for parsing `relocal.toml`
- Logging: `tracing` crate. Default level WARN. `-v` gives INFO, `-vv` gives
  DEBUG, `-vvv` gives TRACE.
- No async runtime needed — the sidecar can use threads (one for reading the
  request FIFO, main thread for running rsync and writing acks)

## Output and UX

- Progress: rsync is run with default output. In verbose mode (`-v`+), rsync's
  `--progress` flag is added so the user can see file transfer progress.
- Errors: printed to stderr with context (which operation failed, the remote
  host, the session name).
- Colors: not required. Plain text output. Can be added as a future improvement.

## Testability

Code and design choices should favor testability. Specifically:

- **Pure logic in isolated modules**: Config parsing, session name validation,
  rsync argument construction, hook JSON merging, and hook script content
  generation must be pure functions with no I/O dependencies. These are the
  primary unit test surface.

- **Trait abstraction for command execution**: A `CommandRunner` trait (or
  equivalent) abstracts shelling out to `ssh`, `rsync`, and other external
  commands. The production implementation uses `std::process::Command`. Test
  implementations record invocations and return configured results. This allows
  orchestration logic (sidecar, `start`, `install`) to be tested without real
  SSH.

- **Function signatures that enable testing**:
  - Config parsing: `&str` → `Result<Config, Error>`
  - Session name validation: `&str` → `Result<(), Error>`
  - Repo root discovery: `&Path` (CWD) → `Result<PathBuf, Error>`
    (testable with temp directories; only checks given dir, no upward walk)
  - rsync argument construction: `(&Config, Direction, &str)` →
    `Vec<String>` (includes all `.claude/` filter logic)
  - Hook JSON merging: `(Option<serde_json::Value>, &str)` →
    `serde_json::Value`

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
  `apt_packages` = `[]`,
  `claude_sync_dirs` = `["skills", "commands", "plugins"]`.
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
- Push direction: `.claude/` excluded wholesale, configured `claude_sync_dirs`
  re-included, `settings.json` re-included.
- Pull direction: `.claude/` excluded wholesale, configured `claude_sync_dirs`
  re-included, `settings.json` NOT re-included.
- Source and destination paths are correct for push vs. pull.
- Verbose mode (`-v`+) adds `--progress` to rsync.
- Non-default `claude_sync_dirs` produces correct include rules.

#### Hook JSON Merging

- No existing file → complete `settings.json` with hooks.
- Existing file with no `hooks` key → `hooks` key added, all other keys
  preserved.
- Existing `hooks` with no `UserPromptSubmit`/`Stop` → arrays added.
- Existing arrays with no relocal matcher group → relocal matcher group appended.
- Existing arrays with a relocal matcher group → matcher group updated in place.
- User-defined hooks in the same arrays are preserved and not reordered.
- Other top-level keys in `settings.json` are preserved.
- Session name is correctly interpolated into hook commands.
- Re-running merge with the same session produces identical output
  (idempotent).

#### CLI Argument Parsing

- Each subcommand parses correctly with required and optional arguments.
- Verbosity: no flag → WARN, `-v` → INFO, `-vv` → DEBUG, `-vvv` → TRACE.
- Session name present and absent on commands that accept `[session-name]`.
- `init` does not require `relocal.toml`; all other commands do.

### Integration Tests

Integration tests exercise real SSH, rsync, FIFOs, and filesystem operations.

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
- `.claude/skills/` (default sync dir) is synced.
- `.claude/conversations/` (not a sync dir) is NOT synced.
- `.claude/settings.json` is synced and hooks are present after push.

#### Sync Pull

- Files created on remote appear locally after pull.
- Files deleted on remote are deleted locally after pull.
- `.claude/settings.json` is NOT pulled (excluded).
- `.claude/skills/` is pulled.
- `.gitignore`-matching files on remote are not pulled.
- Pull from a non-git remote → refused with error (git fsck safety gate).

#### Hook Injection After Push

- Push that overwrites remote `settings.json` → hooks are re-injected.
- Re-injected hooks reference the correct session name.

#### FIFO Lifecycle

- `start` creates both FIFOs on remote.
- Clean shutdown removes both FIFOs.
- Stale FIFO detection: pre-create FIFOs, verify `start` refuses with error.

#### Sidecar

- Write `push` to request FIFO → sidecar runs rsync (local → remote), writes
  `ok` to ack FIFO.
- Write `pull` to request FIFO → sidecar runs rsync (remote → local), writes
  `ok` to ack FIFO.
- Simulate rsync failure (e.g., invalid remote path) → sidecar writes
  `error:<message>` to ack FIFO.
- Multiple sequential requests are handled correctly.
- Sidecar terminates cleanly when signaled by main process.

#### Hook Script (End-to-End)

- Hook script writes to request FIFO and blocks on ack FIFO.
- After `ok` written to ack FIFO, hook script exits 0.
- After `error:msg` written to ack FIFO, hook script exits non-zero and
  message appears on stderr.

#### Session Lifecycle

- `start` creates remote directory, FIFOs, performs initial push, installs
  hooks.
- On clean exit: FIFOs are removed, summary is printed.
- `destroy` removes working directory and FIFOs.
- `destroy` on non-existent session → error.

#### `relocal remote install`

- Installs hook helper script at `~/relocal/.bin/relocal-hook.sh`.
- Creates `~/relocal/.fifos/` directory.
- Idempotent: re-run does not fail or corrupt state.
- APT/rustup/Claude install steps are tested only for the already-installed
  case (verifying idempotency without uninstalling).

#### `relocal list`

- No sessions → empty output.
- Multiple sessions → all listed.
- `.bin/` and `.fifos/` are excluded from listing.

#### `relocal status`

- Reports correct remote host and path.
- Reports whether remote directory exists.
- Reports whether FIFOs exist.

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
- **SSH connection multiplexing**: Use `ControlMaster`/`ControlPath` to reuse a
  single SSH connection for the sidecar, ack writes, and rsync, reducing
  connection overhead.
- **OS support beyond Ubuntu**: Currently assumes Ubuntu and APT. Future versions
  should support other Linux distributions and package managers.
- **Sync exclusion of `.git/`**: Evaluate whether syncing only git-tracked
  content (via `git archive` or similar) is preferable to syncing the full
  `.git/` directory, which can be large.
- **Session persistence**: Detect and reattach to a running remote Claude session
  after a network drop, rather than requiring a fresh start.
- **Automatic reconnection**: Retry SSH on transient network failures.
- **Bidirectional `.claude/settings.json` sync**: Currently, remote changes to
  `settings.json` are not synced back to local. A future version could diff and
  merge settings changes intelligently.
- **Colored output**: Add color support for better UX.
