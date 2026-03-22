# User-level configuration

## Problem

I have one or two disposable SSH boxes that I reuse across everything. Right now, every repo I want to use relocal in
needs its own `relocal.toml` with at least `remote = "user@host"`, and I need to make sure it's in `.gitignore`. That's
enough friction that I won't bother for a quick one-off session in a repo I haven't set up yet.

The goal is to be able to `cd` into any git repo and just run `relocal claude` without touching any project files.

## Design

A user-level config at `~/.relocal/config.toml` with the same schema as the project-level `relocal.toml`. The project
file acts as a per-field override when present.

Merge semantics are simple replacement, not union: if a project config specifies `exclude`, that list entirely replaces
the user-level one. I considered concatenation (user excludes + project excludes) but it makes the mental model harder —
you'd need to remember what your user config contributes to figure out what's actually excluded. With replacement, the
project config is "what I want for this repo" and the user config is "my defaults when I haven't said otherwise."

## Phasing

Two steps, each its own change:

1. Add user config loading with project-level override. A `relocal.toml` in the project is still required for discovery
   — without it, commands fail as before. But fields like `remote` can come from the user config.
2. Allow running without a project-level `relocal.toml` at all. If the user config provides `remote`, that's enough.
   This is the step that actually achieves the "cd into any repo and go" goal.
