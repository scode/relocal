# relocal

> **WARNING: EARLY PROTOTYPE — NOT RECOMMENDED FOR USE YET.**
>
> Bugs or misconfiguration can delete local files. There are safety checks
> but the tool has not been battle-tested. Use at your own risk.

Run Claude Code on a remote Linux box while working locally. Your local repo is
synced bidirectionally via rsync, and Claude hooks on the remote trigger syncs
automatically.

See `SPEC.md` for the full design document.

## Quick start

Prerequisites: a remote Ubuntu box with passwordless SSH (`ssh user@host` works)
and passwordless `sudo`.

```sh
# Install relocal
cargo install --path .

# In your project directory, create a config file
cd ~/my-project
relocal init
# Follow the prompts — at minimum, enter your remote (e.g. user@host)

# Install dependencies on the remote (Rust, Node, Claude Code, etc.)
relocal remote install

# Start a session — syncs your repo and launches Claude on the remote
relocal claude
```

Once the session is running, Claude hooks handle syncing automatically:
- Your local files are pushed to the remote before each prompt
- Claude's changes are pulled back after each response

After the session ends, you can manually sync:

```sh
relocal sync pull    # fetch remote changes
relocal sync push    # push local changes
```

Other commands:

```sh
relocal list                  # list sessions on the remote
relocal status                # show session info
relocal destroy [session]     # remove a session's remote directory
relocal remote nuke           # wipe all relocal state on the remote
```

## Running tests

Unit tests (no remote needed):

```sh
cargo test
```

Integration tests require SSH access to a remote host (can be localhost):

```sh
RELOCAL_TEST_REMOTE=$USER@localhost cargo test -- --ignored --test-threads=1
```

The `--test-threads=1` is required because integration tests share remote state.
