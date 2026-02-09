# relocal

Rust CLI tool. See `SPEC.md` for the full specification.

## Keeping SPEC.md Current

`SPEC.md` is the authoritative design document. As you implement:

- **Update the spec** when making design decisions that diverge from or extend it — new traits, changed command behavior, different hook strategies, altered sync semantics, etc. A future developer re-implementing from the spec should arrive at a compatible design.
- **Don't clutter the spec** with minor implementation details like internal helper function names, error message wording, or module-private types. The spec describes _what_ and _why_, not every _how_.
- When in doubt about whether a change is spec-worthy: if someone implementing from scratch would need to know it to produce a compatible tool, update the spec.

## Documentation

Add `///` docstrings to modules (via `//!` at the top of the file) and major public types (structs, enums, traits). Focus on the big picture: what the module/type is for, why it exists, and how it fits into the overall architecture. Do not document obvious things — e.g., a field `remote: String` on a config struct does not need `/// The remote host`. Reserve per-field/per-method docs for cases where the purpose or behavior is non-obvious.

When modifying code, check that existing comments and docstrings in the affected area are still accurate. Update or remove any that have become stale or misleading due to your changes.

## After Writing Code

ALWAYS run the following after you're done writing code, and fix any issues before considering the task complete:

1. `cargo fmt` — apply formatting
2. `cargo clippy` — fix all warnings (dead_code warnings for not-yet-used items are acceptable)
3. `cargo test` — all tests must pass

## Before Finishing Work

Before considering any task complete, verify the implementation still satisfies `SPEC.md`. Read the relevant spec sections and check that behavior, CLI interface, sync mechanics, hook handling, and test requirements all match. If the implementation deviates from the spec, ask the user whether to update the spec or change the code — do not silently leave them out of sync.
