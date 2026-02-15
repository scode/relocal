# relocal

> **WARNING: EARLY PROTOTYPE — NOT RECOMMENDED FOR USE YET.**
>
> Bugs or misconfiguration can delete local files. There are safety checks
> but the tool has not been battle-tested. Use at your own risk.

relocal runs Claude Code on a remote Ubuntu host while your local repo stays
the source of truth through automatic bidirectional sync. When you submit a
prompt, local changes are pushed to the remote before Claude sees them; when
Claude finishes responding, its changes are pulled back. This lets you review
and edit code locally in your editor while Claude Code runs in your terminal,
with no manual synchronization.

## Security Model and Host Exposure

`relocal` runs Claude remotely with `--dangerously-skip-permissions` and syncs
your repo bidirectionally with `rsync --delete`, including the entire `.git/`
directory. These rsync operations are triggered by Claude hooks: `UserPromptSubmit`
pushes local changes before Claude processes your prompt, and `Stop` pulls
remote changes back after Claude finishes a response.

### Caveats

This design is intended to provide enough isolation to run Claude with
`--dangerously-skip-permissions` in many setups, but there are real caveats and
exposure paths you should treat as in-scope risk.

If your remote is `localhost` (or the same machine/account as your
workstation), there is no real isolation. In that setup, remote execution is
effectively local unsandboxed execution.

Because `.git/` is synced both directions, a compromised or misused remote can
affect local host behavior after a pull. Important examples:
- `.git/hooks/*` can be modified remotely and later execute locally when you
  run Git commands.
- `.git/config` can be modified remotely (for example `core.hooksPath`,
  `sshCommand`, credential helpers, remotes, signing programs), which can
  trigger local command execution or credential leakage.
- Full Git metadata is exposed remotely: history, reflogs, stashes, and
  unreachable objects may contain sensitive data you thought was removed.
- `rsync --delete` over `.git/` can propagate ref/config/state tampering back
  to local even when repository object integrity checks pass.

`git fsck` checks in relocal reduce accidental destructive pulls, but they do
not make remote-sourced `.git` content trustworthy.

**Operational expectations:**
- Treat the remote host as disposable sandbox infrastructure, not a trusted
  long-lived environment.
- Prefer a dedicated throwaway VM/user and a dedicated local clone for relocal
  sessions.
- Do not authenticate to external services from the remote sandbox (GitHub CLI,
  cloud CLIs, package registries, databases, internal systems, etc.).
- Claude authentication on the remote is the only expected exception needed for
  relocal to function.
- Keep sensitive credentials and privileged operations on your local machine.
- If credentials are entered on the remote, rotate/revoke them promptly.

## User Guide

### Prerequisites

Local machine:
- `ssh`
- `rsync`
- Rust toolchain (if installing with `cargo install --path .`)

Remote machine:
- Ubuntu host reachable over SSH

### Remote Setup (run as root on Ubuntu)

```sh
# Run on the remote Ubuntu box as root.
# Replace values first:
NEW_USER="alice"
SSH_PUBKEY="ssh-ed25519 AAAA... yourname@laptop"

# 1) Create user
adduser --disabled-password --gecos "" "$NEW_USER"
usermod -aG sudo "$NEW_USER"

# 2) Install SSH key for that user
install -d -m 700 -o "$NEW_USER" -g "$NEW_USER" "/home/$NEW_USER/.ssh"
printf '%s\n' "$SSH_PUBKEY" > "/home/$NEW_USER/.ssh/authorized_keys"
chown "$NEW_USER:$NEW_USER" "/home/$NEW_USER/.ssh/authorized_keys"
chmod 600 "/home/$NEW_USER/.ssh/authorized_keys"

# 3) Grant passwordless sudo
printf '%s ALL=(ALL) NOPASSWD:ALL\n' "$NEW_USER" > "/etc/sudoers.d/90-$NEW_USER-relocal"
chmod 440 "/etc/sudoers.d/90-$NEW_USER-relocal"
visudo -cf "/etc/sudoers.d/90-$NEW_USER-relocal"

# 4) Verify passwordless sudo works
su - "$NEW_USER" -c 'sudo -n true && echo "passwordless sudo OK"'
```

### Quick Start

```sh
# Install relocal locally
cargo install --path .

# In your project directory, create a config file
cd ~/my-project
relocal init
# Follow the prompts and set remote to user@host

# Install dependencies on the remote (Rust, Node, Claude Code, etc.)
relocal remote install

# Start a session — syncs your repo and launches Claude on the remote
relocal claude
```

Once the session is running, hooks keep local and remote in sync automatically.

### Common Commands

```sh
relocal sync pull [session-name]  # fetch remote changes to local
relocal sync push [session-name]  # push local changes to remote
relocal status [session-name]     # show session info
relocal list                      # list sessions on the remote
relocal destroy [session-name]    # remove one session's remote directory
relocal remote nuke               # wipe all relocal state on the remote
```

## Developer Guide

### Local Development

```sh
cargo build
cargo run -- --help
```

### Testing

Unit tests (no remote needed):

```sh
cargo test
```

Integration tests require SSH access to a remote host (localhost works):

```sh
RELOCAL_TEST_REMOTE=$USER@localhost cargo test -- --ignored --test-threads=1
```

The `--test-threads=1` flag is required because integration tests share remote
state.

### Design Notes

See [`SPEC.md`](SPEC.md) for the full design and architecture details.
