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

## Running without a project config (phase 2)

The hard part here isn't config loading — `load_merged_config` already handles a missing project config gracefully. The
hard part is that two safety mechanisms used `relocal.toml` as their anchor, and without it, `rsync --delete` has fewer
guardrails against wiping the wrong directory.

### What relocal.toml was doing for safety

Two things, beyond holding configuration:

1. **Repo root discovery.** `find_repo_root()` only returned a path if `relocal.toml` existed there. This scoped what
   rsync would sync — get this wrong and `--delete` wipes whatever directory you pointed it at.
2. **Pull target validation.** Before running `rsync --delete` on a pull, `validate_local_pull_target()` checked that
   the local destination contained `relocal.toml`. A second line of defense against a bug in higher-level code passing
   the wrong path.

Both of these need replacement anchors when there's no project config.

### .git as the replacement anchor

The `.git` directory (or file, in the case of worktrees) is the natural replacement. If CWD contains `.git`, it's a git
repo root, and that's what relocal syncs. The no-upward-walk rule still applies — same rationale as before about
preventing accidental syncs of parent directories.

For pull validation, accepting `.git` as an alternative to `relocal.toml` is slightly weaker (a `.git` directory is more
common than a `relocal.toml`), but the scenario where rsync targets the wrong git repo is not meaningfully more
dangerous than targeting the right one — the real danger is targeting a non-repo directory like `$HOME`, and `.git`
prevents that.

### Session naming

All default session names are now `<dirname>-<8-hex-chars>`, regardless of whether `relocal.toml` is present. The hash
is SHA-256 of the canonical local path and git origin URL, truncated to 4 bytes. Using the same format everywhere avoids
a class of problems where adding or removing `relocal.toml` silently changes the session name, causing later commands to
miss the existing session and create a duplicate remote working copy.
