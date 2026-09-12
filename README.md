# profile-materials

Source for the ItzHerzchen profile service and profile assets.

The service reads Discord presence through an official bot, keeps the last known presence across restarts, streams changes over SSE, and exposes health endpoints for deployment. Discord activity artwork is resolved into usable image URLs with deterministic fallbacks. Linked Spotify activity is read directly from Discord, so no separate Spotify account, OAuth flow, token, or polling service is required.

It also collects GitHub profile statistics and serves SVG cards for GitHub stats, top languages, contribution streaks, Discord presence, and Spotify activity. The GitHub cards follow the visual language and statistics semantics of the profile's existing GitHub Readme Stats setup while using this service's own refresh, revision, and last-known-good state.

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

GITHUB_TOKEN=<read-only token>           # optional; enables GitHub stats
GITHUB_USERNAME=Herzchens                # optional; default shown
GITHUB_FEATURED_REPOS=repo-a,repo-b      # optional; ordered, up to 3 shown
GITHUB_STATE_PATH=state/github.json      # optional; default shown
GITHUB_POLL_SECS=15                      # optional; minimum 15
GITHUB_STALE_AFTER_SECS=21600            # optional; default shown

RUST_LOG=info                            # optional
```

`PROFILE_STALE_AFTER_SECS` must be lower than `PROFILE_UNAVAILABLE_AFTER_SECS`. `PROFILE_BIND_ADDR` defaults to loopback so the service can sit behind a reverse proxy without exposing the listener directly.

GitHub collection is optional. Without `GITHUB_TOKEN`, the Discord and SVG service still runs and the GitHub endpoints return the last persisted GitHub snapshot when one exists, or a neutral unavailable state otherwise. The token is never written to the GitHub LKG file. A token limited to public data produces public-only statistics; if private repositories should contribute to aggregate commit and language statistics, the token must have read access to those repositories.

The GitHub collector refreshes every 15 seconds by default. GitHub rate-limit responses and a low remaining GraphQL budget automatically make the collector back off. `GITHUB_STALE_AFTER_SECS` must be greater than the poll interval.

## Run

```bash
cargo run --locked
```

Available endpoints:

```text
GET /v1/public/presence
GET /v1/public/github
GET /v1/live
GET /v1/svg/hero-test.svg
GET /v1/svg/presence.svg
GET /v1/svg/spotify.svg?layout=mini|compact|wide
GET /v1/svg/github.svg
GET /v1/svg/github-stats.svg
GET /v1/svg/github-languages.svg
GET /v1/svg/github-streak.svg
GET /debug/live
GET /health/live
GET /health/ready
```

`/v1/public/presence` returns the current allow-listed presence state. Activities include a resolved artwork URL when Discord provides a usable asset plus a stable fallback key for the renderer. When a linked Spotify listening activity is present, the response also includes a compact `spotify` object with title, artist, album, cover URL, and track timing information.

`/v1/live` streams the same public presence representation over server-sent events. `/debug/live` is a minimal browser view of that stream.

The GitHub collector follows the same core statistics semantics as the profile's existing `github-readme-stats` cards. It reads the contribution calendar for streaks and contribution totals, uses GitHub commit search for the all-time commit total, and collects pull requests, reviews, issues, stars, repositories contributed to, followers, and the same rank calculation used by that card. Contribution-calendar totals and streaks still describe the calendar window returned by GitHub; the current streak may continue through yesterday when the current calendar day is still empty.

Top languages follow the existing card configuration as well: up to 100 owned, non-fork repositories are considered, up to 10 languages are read from each repository, and the displayed ranking keeps up to 20 languages. Each language is weighted with `size_weight=0.5` and `count_weight=0.5`, so both reported byte size and the number of repositories using the language affect its share. Private repositories participate when the configured token can read them.

Private repository details are not published by the service. Their names, URLs, descriptions, and individual project metadata are excluded from the public response and SVG. Public language output contains only the aggregate language name, color, and percentage. Featured project cards are selected only from public, non-fork, non-archived repositories.

`GITHUB_FEATURED_REPOS` chooses public project cards in the order listed. When it is not set, the service selects up to three eligible public repositories by stars, then recent push time. A failed GitHub refresh keeps the last-known-good snapshot; old snapshots are marked stale instead of making the card disappear.

`/v1/svg/github-stats.svg` is the TokyoNight-style overall stats card, `/v1/svg/github-languages.svg` is the compact top-languages card, and `/v1/svg/github-streak.svg` is the transparent TokyoNight-duo-style streak card. `/v1/svg/github.svg` keeps the broader combined GitHub summary view for compatibility. The presence card keeps all distinct current activities after same-name duplicate selection, while the Spotify endpoint has `mini`, `compact`, and `wide` layouts. `hero-test.svg` is a plain diagnostic render with visible revisions for cache experiments, not the final profile hero.

SVG responses use revision-based render caching and an `ETag`. They ask clients to revalidate instead of treating an unchanged card as permanently fresh. A matching `If-None-Match` request receives `304 Not Modified`.

The dynamic SVG cards can be embedded through GitHub Camo. Remote Discord and Spotify raster artwork is fetched from allow-listed origins and embedded into the SVG, so the rendered card does not depend on nested external image requests. If artwork cannot be embedded, the renderer uses its deterministic fallback instead of emitting the remote image URL.

GitHub controls Camo caching, so cards embedded in a README should be treated as best-effort near-live views rather than a realtime channel. In the current deployment, an observed presence revision change became visible through the same Camo URL about two seconds after the origin changed; refresh timing is not guaranteed.

Before the first valid target presence event, the public endpoint and SVG views use a neutral waiting state. A Gateway transport failure does not fabricate an offline user state. The service keeps the last-known-good snapshot during the grace window, marks it stale after the configured stale threshold, and switches to a neutral unavailable view after the configured unavailable threshold. A real target `PRESENCE_UPDATE` restores fresh state.

The presence and GitHub LKG files contain only normalized state and collection timestamps. Credentials and session data are not persisted there.
