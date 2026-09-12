# profile-materials

Source for the ItzHerzchen profile service and profile assets.

The service reads Discord presence through an official bot, keeps the last known presence across restarts, streams changes over SSE, and exposes health endpoints for deployment. Discord activity artwork is resolved into usable image URLs with deterministic fallbacks. Linked Spotify activity is read directly from Discord, so no separate Spotify account, OAuth flow, token, or polling service is required.

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

`PROFILE_STALE_AFTER_SECS` must be lower than `PROFILE_UNAVAILABLE_AFTER_SECS`. `PROFILE_BIND_ADDR` defaults to loopback so the service can sit behind a reverse proxy without exposing the listener directly.

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

`/v1/public/presence` returns the current allow-listed presence state. Activities include a resolved artwork URL when Discord provides a usable asset plus a stable fallback key for the renderer. When a linked Spotify listening activity is present, the response also includes a compact `spotify` object with title, artist, album, cover URL, and track timing information.

`/v1/live` streams the same public presence representation over server-sent events. `/debug/live` is a minimal browser view of that stream.

Before the first valid target presence event, the public endpoint reports `availability: "unknown"`. A Gateway transport failure does not fabricate an offline user state. The service keeps the last-known-good snapshot during the grace window, marks it stale after the configured stale threshold, and returns a neutral `availability: "unavailable"` view after the configured unavailable threshold. A real target `PRESENCE_UPDATE` restores fresh state.

The LKG file contains only the normalized presence snapshot and validation timestamp. Credentials and Discord session data are not persisted there.
