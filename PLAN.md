# relocal Implementation Plan

## Context

`relocal` is a greenfield Rust CLI tool that runs Claude Code on a remote Linux box while keeping a local repo in sync via bidirectional rsync. The codebase currently has only `SPEC.md` and `CLAUDE.md` — no Rust code exists yet. This plan breaks implementation into bottom-up steps where each step is completable and fully tested before moving on.

## Module Structure

```
src/
  main.rs          — entry point, CLI dispatch
  cli.rs           — clap derive definitions
  config.rs        — relocal.toml parsing + defaults
  session.rs       — session name validation + default derivation
  discovery.rs     — repo root discovery (walk up for relocal.toml)
  error.rs         — shared error types
  rsync.rs         — rsync argument construction (push/pull, .claude/ handling)
  hooks.rs         — hook JSON merging + hook script content generation
  runner.rs        — CommandRunner trait + real (Process) implementation
  ssh.rs           — SSH command building helpers
  sidecar.rs       — sync sidecar (FIFO reader, rsync executor, ack writer)
  commands/
    mod.rs
    init.rs        — `relocal init`
    install.rs     — `relocal remote install`
    sync.rs        — `relocal sync push` / `relocal sync pull`
    start.rs       — `relocal start` (main orchestration)
    status.rs      — `relocal status`
    list.rs        — `relocal list`
    destroy.rs     — `relocal destroy`
    nuke.rs        — `relocal remote nuke`
tests/
  integration/
    mod.rs         — shared test helpers (temp dirs, remote cleanup, Drop guards)
    sync_test.rs   — push/pull integration tests
    hooks_test.rs  — hook injection after push
    fifo_test.rs   — FIFO lifecycle tests
    sidecar_test.rs — sidecar end-to-end tests
    session_test.rs — session lifecycle (start, destroy)
    install_test.rs — remote install idempotency
    list_test.rs   — list/status/nuke tests
```

## Implementation Steps

### Step 1: Project Scaffold + Pure Foundations
_Goal: Cargo project compiles, core pure-logic modules exist with full unit test coverage._

- [x] **1a. Create `Cargo.toml`** with dependencies: `clap` (features: derive), `serde` (features: derive), `toml`, `serde_json`, `tracing`, `tracing-subscriber`, `thiserror`, `dialoguer`. Dev-dependency: `tempfile`.
- [x] **1b. Create `src/error.rs`** — shared error enum using `thiserror`. Variants for: ConfigNotFound, ConfigParse, InvalidSessionName, IoError, CommandFailed, RemoteError, etc.
- [x] **1c. Create `src/config.rs`** — `Config` struct with serde derive. Parse from `&str`. Fields: `remote` (required), `exclude` (default `[]`), `apt_packages` (default `[]`), `claude_sync_dirs` (default `["skills", "commands", "plugins"]`). Deny unknown fields: no (forward compat). Unit tests per spec: minimal config, full config, missing remote, invalid TOML, defaults, unknown keys ignored.
- [x] **1d. Create `src/session.rs`** — `validate_session_name(&str) -> Result<()>` (alphanumeric + hyphen + underscore, non-empty). `default_session_name(&Path) -> Result<String>` (from directory name). Unit tests per spec: valid names, invalid names (space, slash, dot, traversal, empty), default derivation, invalid directory name.
- [ ] **1e. Create `src/discovery.rs`** — `find_repo_root(&Path) -> Result<PathBuf>`. Walks up looking for `relocal.toml`. Unit tests with `tempfile`: found in current dir, parent, grandparent, not found, nearest wins.
- [ ] **1f. Create `src/main.rs`** — minimal stub that compiles (`fn main() {}`), imports all modules.

### Step 2: CLI Parsing + Logging
_Goal: All subcommands parse correctly, verbosity flags work, `main` dispatches._

- [ ] **2a. Create `src/cli.rs`** — clap derive structs. Top-level `Cli` with global `-v` verbosity (count). Subcommands: `Init`, `Remote { Install | Nuke }`, `Start { session_name: Option<String> }`, `Sync { Push | Pull, session_name: Option<String> }`, `Status { session_name: Option<String> }`, `List`, `Destroy { session_name: Option<String> }`. Unit tests: each subcommand parses, verbosity levels (0=WARN, 1=INFO, 2=DEBUG, 3=TRACE), session name present/absent.
- [ ] **2b. Wire up `src/main.rs`** — parse CLI, init tracing-subscriber with verbosity level, match on subcommand (stubs that print "not yet implemented"). Verify `cargo run -- --help` works.

### Step 3: CommandRunner Trait
_Goal: Abstraction for shelling out to ssh/rsync, enabling mock-based testing of orchestration._

- [ ] **3a. Create `src/runner.rs`** — `CommandRunner` trait with methods like: `run_ssh(&self, remote: &str, command: &str) -> Result<CommandOutput>`, `run_ssh_interactive(&self, remote: &str, command: &str) -> Result<ExitStatus>`, `run_rsync(&self, args: &[String]) -> Result<CommandOutput>`, `run_local(&self, program: &str, args: &[&str]) -> Result<CommandOutput>`. `CommandOutput` struct: `stdout: String, stderr: String, status: ExitStatus`.
- [ ] **3b. Implement `ProcessRunner`** (production impl) — uses `std::process::Command`. SSH commands use `ssh user@host "command"`. Interactive SSH uses `ssh -t`.
- [ ] **3c. Create `MockRunner`** (in `#[cfg(test)]` or a test-support module) — records invocations, returns pre-configured results. Uses `RefCell<Vec<Invocation>>` for recording and a configurable response queue.

### Step 4: SSH Helpers + rsync Argument Construction
_Goal: Pure functions that build correct command arguments for all sync scenarios._

- [ ] **4a. Create `src/ssh.rs`** — helper functions to construct SSH command strings for: running remote commands, creating/removing remote directories, creating/checking/removing FIFOs, reading/writing to FIFOs.
- [ ] **4b. Create `src/rsync.rs`** — `Direction` enum (`Push`, `Pull`). `build_rsync_args(config: &Config, direction: Direction, session_name: &str, repo_root: &Path, verbose: bool) -> Vec<String>`. Implements the complex `.claude/` filtering logic: exclude `.claude/` wholesale, re-include configured `claude_sync_dirs` subdirectories, include `settings.json` on push only (not pull). Unit tests per spec: base flags present, .gitignore filter, custom excludes, push vs pull .claude/ handling, correct source/dest paths, verbose adds --progress, non-default claude_sync_dirs.

### Step 5: Hook JSON Merging + Hook Script Generation
_Goal: Pure functions for hook management, fully unit tested._

- [ ] **5a. `src/hooks.rs` — hook JSON merging** — `merge_hooks(existing: Option<serde_json::Value>, session_name: &str) -> serde_json::Value`. Handles all cases per spec: no existing file, no hooks key, no arrays, no relocal entry (append), existing relocal entry (update in place), user hooks preserved. Relocal hooks identified by `relocal-hook.sh` in command string.
- [ ] **5b. `src/hooks.rs` — hook script generation** — `hook_script_content() -> String`. Returns the bash script content for `relocal-hook.sh`. Unit test: script contains correct FIFO paths with `$RELOCAL_SESSION`, correct push/pull handling, correct ack reading.
- [ ] **5c. Unit tests for hook merging** — all cases from spec section "Hook JSON Merging" (no file, no hooks key, no arrays, no relocal entry, existing entry, user hooks preserved, other keys preserved, correct session interpolation, idempotent).

### Step 6: `init` Command
_Goal: First working user-facing command._

- [ ] **6a. Create `src/commands/init.rs`** — interactive prompts via `dialoguer` for remote, exclude, apt_packages. Writes `relocal.toml` to current directory. No CommandRunner needed — purely local.
- [ ] **6b. Unit test** — test TOML output generation from collected inputs (separate the I/O from the generation logic).

### Step 7: `remote install` Command
_Goal: Remote environment setup works end-to-end._

- [ ] **7a. Create `src/commands/install.rs`** — Implements all 6 steps from spec: APT packages, rustup, Claude Code, Claude auth, hook script, FIFO directory. Each step checks if already done (idempotent). Uses `CommandRunner` for all SSH operations.
- [ ] **7b. Unit tests with MockRunner** — verify correct SSH commands are issued for each step, verify idempotency (already-installed detection), verify user packages from config are included in APT command.

### Step 8: Sync Push / Pull Commands
_Goal: Manual sync commands work, hook re-injection on push._

- [ ] **8a. Create `src/commands/sync.rs`** — `sync_push` and `sync_pull` functions. Uses `build_rsync_args` from step 4, runs via `CommandRunner`. Push: after rsync, reads remote `.claude/settings.json` via SSH, runs `merge_hooks`, writes back. Pull: just rsync.
- [ ] **8b. Unit tests with MockRunner** — verify rsync invoked with correct args for push/pull, verify hook re-injection happens after push (SSH read + write of settings.json), verify hook re-injection does NOT happen after pull.

### Step 9: Status, List, Destroy, Remote Nuke Commands
_Goal: All informational and cleanup commands work._

- [ ] **9a. Create `src/commands/status.rs`** — checks remote dir exists, Claude installed, FIFOs exist. All via SSH commands through `CommandRunner`.
- [ ] **9b. Create `src/commands/list.rs`** — lists `~/relocal/` dirs excluding `.bin/` and `.fifos/`. Shows session name + size.
- [ ] **9c. Create `src/commands/destroy.rs`** — prompts for confirmation, removes remote working dir + FIFOs. Uses `CommandRunner`.
- [ ] **9d. Create `src/commands/nuke.rs`** — prompts for confirmation, removes entire `~/relocal/`. Uses `CommandRunner`.
- [ ] **9e. Unit tests with MockRunner** for each command — verify correct SSH commands, verify confirmation prompt behavior.

### Step 10: Sidecar Implementation
_Goal: Background sync mediator works correctly._

- [ ] **10a. Create `src/sidecar.rs`** — `Sidecar` struct. Spawns a thread that: opens SSH connection reading request FIFO (`ssh user@host "while true; do cat <fifo>; done"`), on each line received runs appropriate rsync + hook re-injection (for push), writes ack to ack FIFO via SSH. Provides `shutdown()` method that terminates the SSH process and joins the thread.
- [ ] **10b. Unit tests with MockRunner** — verify sidecar issues correct SSH command for FIFO reading, verify push request triggers rsync + hook re-injection + ok ack, verify pull request triggers rsync + ok ack, verify rsync failure triggers error ack, verify multiple sequential requests work, verify clean shutdown.

### Step 11: `start` Command (Main Orchestration)
_Goal: The primary user workflow works end-to-end._

- [ ] **11a. Create `src/commands/start.rs`** — implements full flow: load config, validate session name, check stale FIFOs, create remote dir, create FIFOs, initial push, install hooks, start sidecar, SSH interactive session (`ssh -t ... "cd ~/relocal/<session> && claude --dangerously-skip-permissions"`), on SSH exit: shutdown sidecar, remove FIFOs, print summary.
- [ ] **11b. Signal handling** — SIGINT forwarded naturally by SSH terminal. On SSH exit (any cause): cleanup proceeds.
- [ ] **11c. Dirty shutdown handling** — detect SSH error exit, attempt FIFO cleanup (best-effort), print recovery instructions.
- [ ] **11d. Unit tests with MockRunner** — verify full sequence of operations, verify stale FIFO detection (refuses to start), verify FIFO cleanup on clean exit, verify error path (SSH fails), verify summary printed.

### Step 12: Wire Up Main + End-to-End Smoke Test
_Goal: All commands dispatched from main, binary works._

- [ ] **12a. Wire `src/main.rs`** — match each CLI subcommand to its implementation. Handle repo root discovery (skip for `init`). Resolve session name (default from dir name if not provided). Pass `ProcessRunner` + config + session to command functions.
- [ ] **12b. Manual smoke test** — `cargo run -- init`, verify `relocal.toml` created. `cargo run -- --help`, verify all subcommands listed.

### Step 13: Integration Tests
_Goal: Real SSH/rsync/FIFO tests against localhost. All gated on `RELOCAL_TEST_REMOTE` env var._

- [ ] **13a. Test infrastructure** (`tests/integration/mod.rs`) — helpers for: creating local temp dirs with `tempfile`, generating unique session names, SSH cleanup (remove remote dirs + FIFOs) via `Drop` guard, reading/writing remote files via SSH, skipping when `RELOCAL_TEST_REMOTE` unset.
- [ ] **13b. Sync push tests** — files appear on remote, deletes propagate, .gitignore respected, config excludes respected, .claude/skills/ synced, .claude/conversations/ not synced, settings.json synced + hooks present.
- [ ] **13c. Sync pull tests** — files appear locally, deletes propagate, settings.json NOT pulled, .claude/skills/ pulled, .gitignore-matching files not pulled.
- [ ] **13d. Hook injection tests** — push overwrites settings.json → hooks re-injected, correct session name in hooks.
- [ ] **13e. FIFO lifecycle tests** — start creates FIFOs, clean shutdown removes them, stale FIFO detection works.
- [ ] **13f. Sidecar tests** — write push to request FIFO → rsync runs + ok ack, write pull → rsync + ok ack, rsync failure → error ack, multiple sequential requests, clean sidecar shutdown.
- [ ] **13g. Hook script end-to-end tests** — hook writes to request FIFO, blocks on ack, ok → exit 0, error:msg → exit non-zero + stderr.
- [ ] **13h. Session lifecycle tests** — start creates dir + FIFOs + pushes + hooks, clean exit cleans FIFOs, destroy removes dir + FIFOs, destroy non-existent → error.
- [ ] **13i. Remote install tests** — hook script installed at correct path, .fifos/ dir created, re-run is idempotent.
- [ ] **13j. List/status/nuke tests** — no sessions → empty, multiple sessions listed, .bin/.fifos excluded, status reports correct info, nuke removes everything.
- [ ] **13k. Localhost-as-remote test** — push/pull work when remote is same machine, remote dir distinct from local temp dir.

### Step 14: Final Polish + Spec Reconciliation
_Goal: Implementation matches SPEC.md, code is clean._

- [ ] **14a. Read SPEC.md end-to-end**, verify every behavior is implemented.
- [ ] **14b. Update SPEC.md** if any design decisions diverged during implementation.
- [ ] **14c. Run full test suite** — `cargo test` (unit) + `RELOCAL_TEST_REMOTE=$USER@localhost cargo test` (integration).
- [ ] **14d. Run `cargo clippy`** and fix warnings.

## Notes

- Interactive prompts (init wizard, destroy/nuke confirmations) use the `dialoguer` crate.
- Integration tests assume passwordless SSH to localhost is available on the dev machine.
- Each step is a potential commit point — implement, test, commit.

## Verification

After each step:
- `cargo build` succeeds
- `cargo test` passes (unit tests)

After step 13:
- `RELOCAL_TEST_REMOTE=$USER@localhost cargo test -- --ignored` passes all integration tests

After step 14:
- `cargo clippy` clean
- SPEC.md and implementation are in sync
