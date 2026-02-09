# relocal

Rust CLI tool. See `SPEC.md` for the full specification.

## Keeping SPEC.md Current

`SPEC.md` is the authoritative design document. As you implement:

- **Update the spec** when making design decisions that diverge from or extend it — new traits, changed command behavior, different hook strategies, altered sync semantics, etc. A future developer re-implementing from the spec should arrive at a compatible design.
- **Don't clutter the spec** with minor implementation details like internal helper function names, error message wording, or module-private types. The spec describes _what_ and _why_, not every _how_.
- When in doubt about whether a change is spec-worthy: if someone implementing from scratch would need to know it to produce a compatible tool, update the spec.

## After Writing Code

ALWAYS run the following after you're done writing code, and fix any issues before considering the task complete:

1. `cargo fmt` — apply formatting
2. `cargo clippy` — fix all warnings (dead_code warnings for not-yet-used items are acceptable)
3. `cargo test` — all tests must pass

## Before Finishing Work

Before considering any task complete, verify the implementation still satisfies `SPEC.md`. Read the relevant spec sections and check that behavior, CLI interface, sync mechanics, hook handling, and test requirements all match. If the implementation deviates from the spec, ask the user whether to update the spec or change the code — do not silently leave them out of sync.
