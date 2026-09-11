# profile-materials

Source for the ItzHerzchen profile service and profile assets.

The service is currently at the Discord Gateway proof stage: an official Discord bot observes the configured target account through `PRESENCE_UPDATE`, publishes an immutable in-memory snapshot, and exposes the allow-listed result at `GET /v1/public/presence`.

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
PROFILE_BIND_ADDR=127.0.0.1:3000   # optional; this is the default
RUST_LOG=info                       # optional
```

`PROFILE_BIND_ADDR` defaults to loopback so the service can sit behind a reverse proxy without accidentally exposing an unreviewed listener.

## Run

```bash
cargo run --locked
```

The Phase 0 endpoint is then available at:

```text
GET /v1/public/presence
```

Before the first valid target presence event, the endpoint reports `availability: "unknown"`. A Discord Gateway disconnect or reconnect does **not** synthesize an `offline` user state; the most recently published presence remains unchanged until Discord delivers another target `PRESENCE_UPDATE`.

## Checks

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```
