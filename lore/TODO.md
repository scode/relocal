# TODO

## Concurrent `relocal claude` and `relocal codex` sessions

The intent is to run both agents against the same session (same remote directory) concurrently. Currently the lock file
mechanism prevents this — the second session is rejected with a stale session error because the first session's lock
already exists.

To support this:

- The lock file mechanism needs to support multiple holders (e.g., reference counting, or per-tool locks that coexist)
  so the second session can join without being rejected.
- The second session should skip the initial push, since the remote directory is already populated by the first session.
- Avoid starting a second background sync loop if one is already running for that session.
