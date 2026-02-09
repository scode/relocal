# relocal

Rust CLI tool. See `SPEC.md` for the full specification.

## Keeping SPEC.md Current

`SPEC.md` is the authoritative design document. As you implement:

- **Update the spec** when making design decisions that diverge from or extend it — new traits, changed command behavior, different hook strategies, altered sync semantics, etc. A future developer re-implementing from the spec should arrive at a compatible design.
- **Don't clutter the spec** with minor implementation details like internal helper function names, error message wording, or module-private types. The spec describes _what_ and _why_, not every _how_.
- When in doubt about whether a change is spec-worthy: if someone implementing from scratch would need to know it to produce a compatible tool, update the spec.

## Before Finishing Work

Before considering any task complete, verify the implementation still satisfies `SPEC.md`. Read the relevant spec sections and check that behavior, CLI interface, sync mechanics, hook handling, and test requirements all match. If the implementation deviates from the spec, ask the user whether to update the spec or change the code — do not silently leave them out of sync.
