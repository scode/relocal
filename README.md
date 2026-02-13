# relocal

> **WARNING: EARLY PROTOTYPE — NOT RECOMMENDED FOR USE YET.**
>
> Bugs or misconfiguration can delete local files. There are safety checks
> but the tool has not been battle-tested. Use at your own risk.

relocal runs Claude Code on a remote Ubuntu host while your local repo stays
the source of truth through automatic bidirectional sync.

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
