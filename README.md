# profile-materials

Source for the ItzHerzchen profile service and profile assets.

The service uses an official Discord bot and Gateway `PRESENCE_UPDATE` events to maintain an immutable realtime presence snapshot. Phase 1 adds semantic revisions, SSE fan-out, last-known-good persistence, stale-state handling, and health endpoints while keeping the public listener loopback-only by default.

## Requirements

- Rust toolchain from `rust-toolchain.toml`
- an official Discord application/bot
- the bot and target account in the configured target guild
- **Presence Intent** (`GUILD_PRESENCES`) enabled for the bot

The service does not require Message Content intent and must not be run with a Discord user token or selfbot token.

## Configuration

Set these environment variables before starting the service:

```text
DISCORD_BOT_TOKEN=<official bot token>
TARGET_DISCORD_USER_ID=<target user snowflake>
TARGET_GUILD_ID=<shared guild snowflake>

PROFILE_BIND_ADDR=127.0.0.1:3000        # optional; default shown
PROFILE_STATE_PATH=state/presence.json  # optional; default shown
PROFILE_STALE_AFTER_SECS=120            # optional; default shown
PROFILE_UNAVAILABLE_AFTER_SECS=600      # optional; default shown
RUST_LOG=info                            # optional
```

`PROFILE_STALE_AFTER_SECS` must be lower than `PROFILE_UNAVAILABLE_AFTER_SECS`. `PROFILE_BIND_ADDR` defaults to loopback so the service can sit behind a reverse proxy without accidentally exposing an unreviewed listener.

## Run

```bash
cargo run --locked
```

Available endpoints:

```text
GET /v1/public/presence
GET /v1/live
GET /debug/live
GET /health/live
GET /health/ready
```

`/v1/live` is a server-sent-events stream. `/debug/live` is a deliberately minimal browser page used to prove that presence changes arrive without a page refresh.

Before the first valid target presence event, the public endpoint reports `availability: "unknown"`. A Gateway transport failure never fabricates an `offline` user status. The service keeps the last-known-good snapshot during the grace window, marks it stale after the configured stale threshold, and returns a neutral `availability: "unavailable"` view after the configured unavailable threshold. A real target `PRESENCE_UPDATE` immediately restores fresh state.

The LKG file contains only the normalized presence snapshot and validation timestamp; credentials and Discord session data are never persisted there.

## Checks

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
```
