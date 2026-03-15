# relocal

Rust CLI tool. See `SPEC.md` for the full specification.

## Keeping SPEC.md Current

`SPEC.md` is the authoritative design document. As you implement:

- **Update the spec** when making design decisions that diverge from or extend it — new traits, changed command behavior, altered sync semantics, etc. A future developer re-implementing from the spec should arrive at a compatible design.
- **Don't clutter the spec** with minor implementation details like internal helper function names, error message wording, or module-private types. The spec describes _what_ and _why_, not every _how_.
- When in doubt about whether a change is spec-worthy: if someone implementing from scratch would need to know it to produce a compatible tool, update the spec.

## Documentation

Add `///` docstrings to modules (via `//!` at the top of the file) and major public types (structs, enums, traits). Focus on the big picture: what the module/type is for, why it exists, and how it fits into the overall architecture. Do not document obvious things — e.g., a field `remote: String` on a config struct does not need `/// The remote host`. Reserve per-field/per-method docs for cases where the purpose or behavior is non-obvious.

When modifying code, check that existing comments and docstrings in the affected area are still accurate. Update or remove any that have become stale or misleading due to your changes.

## Error Handling for Shell and Remote Commands

Every call site that runs a shell command (local or remote) must handle failure. Do not rely on `?` alone — `run_ssh` returns `Ok(CommandOutput)` even when the remote command exits non-zero. The `?` only catches transport-level errors (e.g., SSH binary not found).

- **Always check exit status** after `run_ssh` / `run_rsync` unless the call site intentionally treats all outcomes as non-fatal. Use `CommandOutput::check("description")?` for concise checking.
- **Distinguish SSH transport errors from remote command errors** when the distinction affects correctness. For boolean probes (`test -e`, `command -v`), use `ssh::run_status_check` — it wraps the command so that SSH failures propagate as errors rather than being misinterpreted as "not found". A raw `status.success()` check on `run_ssh` output conflates "SSH couldn't connect" with "remote command returned false".
- **When the distinction doesn't matter** (e.g., both SSH failure and command failure should abort with an error), `.check()` is sufficient.
- **Test both paths**: include tests for the failure case of new commands, not just the success path.

## Testing

Always include unit tests alongside new code. Every new function, method, or non-trivial behavior should have test coverage in a `#[cfg(test)]` module in the same file. Don't defer tests to a later step — write them as part of the implementation.

## After Writing Code

ALWAYS run the following after you're done writing code, and fix any issues before considering the task complete:

1. `cargo fmt` — apply formatting
2. `cargo clippy` — fix all warnings (dead_code warnings for not-yet-used items are acceptable)
3. `cargo test` — all tests must pass

Integration tests share remote state and must run sequentially:

```sh
RELOCAL_TEST_REMOTE=$USER@localhost cargo test -- --ignored --test-threads=1
```

## Before Finishing Work

Before considering any task complete, verify the implementation still satisfies `SPEC.md`. Read the relevant spec sections and check that behavior, CLI interface, sync mechanics, and test requirements all match. If the implementation deviates from the spec, ask the user whether to update the spec or change the code — do not silently leave them out of sync.
