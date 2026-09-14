# profile-materials

Source for the ItzHerzchen profile service and profile assets.

## Temporary GitHub compatibility preview

[![Live presence card](https://profile.tailed8451.ts.net/v1/svg/presence.svg?compat=activity-cards)](https://profile.tailed8451.ts.net/presence)

[![Contribution streak compatibility preview](https://profile.tailed8451.ts.net/v1/svg/github-streak.svg?compat=campfire-v1)](https://profile.tailed8451.ts.net/v1/svg/github-streak.svg?compat=campfire-v1)

The streak preview intentionally embeds the live origin URL through normal GitHub Markdown so GitHub Camo compatibility can be checked after the branch build is deployed for testing. The origin SVG embeds its mascot raster asset directly and uses self-contained SVG animation for the lit campfire state.

The service reads Discord presence through an official bot, keeps the last known presence across restarts, streams changes over SSE, and exposes health endpoints for deployment. Discord activity artwork is resolved into usable image URLs with deterministic fallbacks. Spotify playback is collected independently through the Spotify Web API when configured; Discord Spotify RPC is excluded from the presentation layer so Spotify visibility does not depend on Discord presence propagation.

It also collects GitHub profile statistics and serves SVG cards for GitHub stats, top languages, contribution streaks, Discord presence, Spotify activity, and the production profile hero. The GitHub cards follow the visual language and statistics semantics of the profile's existing GitHub Readme Stats setup while using this service's own refresh, revision, and last-known-good state.

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

SPOTIFY_CLIENT_ID=<client id>            # optional as a complete Spotify group
SPOTIFY_CLIENT_SECRET=<client secret>    # required when Spotify is enabled
SPOTIFY_REFRESH_TOKEN=<refresh token>    # required when Spotify is enabled
SPOTIFY_REFRESH_TOKEN_PATH=state/spotify-refresh-token # optional; default shown
SPOTIFY_POLL_SECS=5                      # optional; minimum/default 5

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
GET /v1/svg/hero.svg
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

`/v1/public/presence` returns the current allow-listed presence state. Activities include a resolved artwork URL when Discord provides a usable asset plus a stable fallback key for the renderer. When native Spotify playback is active, the response also includes a compact `spotify` object with title, artist, album, cover URL, and track timing information. Listening activities that provide both start and end timestamps render a live progress bar; long media metadata is fitted to the card instead of being ellipsized.

`/v1/live` streams the same public presence representation over server-sent events. `/debug/live` is a minimal browser view of that stream.

The production hero at `/v1/svg/hero.svg` embeds the final authored banner PNG unchanged. It does not redraw the character, profile typography, decorative artwork, or existing banner composition, and it does not render Discord, RPC, game, application, or Spotify state. Realtime presence stays on the dedicated presence and Spotify cards so the profile README can place and link those separately without covering the hero artwork.

The GitHub collector follows the same core statistics semantics as the profile's existing `github-readme-stats` cards. It reads the contribution calendar for streaks and contribution totals, uses GitHub commit search for the all-time commit total, and collects pull requests, reviews, issues, stars, repositories contributed to, followers, and the same rank calculation used by that card. Contribution-calendar totals and streaks still describe the calendar window returned by GitHub; the current streak may continue through yesterday when the current calendar day is still empty.

Top languages follow the existing card configuration as well: up to 100 owned, non-fork repositories are considered, up to 10 languages are read from each repository, and the displayed ranking keeps up to 20 languages. Each language is weighted with `size_weight=0.5` and `count_weight=0.5`, so both reported byte size and the number of repositories using the language affect its share. Private repositories participate when the configured token can read them.

Private repository details are not published by the service. Their names, URLs, descriptions, and individual project metadata are excluded from the public response and SVG. Public language output contains only the aggregate language name, color, and percentage. Featured project cards are selected only from public, non-fork, non-archived repositories.

`GITHUB_FEATURED_REPOS` chooses public project cards in the order listed. When it is not set, the service selects up to three eligible public repositories by stars, then recent push time. A failed GitHub refresh keeps the last-known-good snapshot; old snapshots are marked stale instead of making the card disappear.

`/v1/svg/github-stats.svg` is the TokyoNight-style overall stats card, `/v1/svg/github-languages.svg` is the compact top-languages card, and `/v1/svg/github-streak.svg` is the campfire-style contribution streak card. The streak card embeds the authored mascot asset, derives contribution ranges from the GitHub contribution calendar, keeps the current streak through an empty current day, and changes the campfire between lit, extinguished, and neutral states based on today's contribution state and snapshot freshness. `/v1/svg/github.svg` keeps the broader combined GitHub summary view for compatibility. The presence card keeps all distinct current activities after same-name duplicate selection, while the Spotify endpoint has `mini`, `compact`, and `wide` layouts. `hero-test.svg` remains a plain diagnostic render with visible revisions for cache experiments.

SVG responses use `ETag` revalidation. Dynamic presence and GitHub cards use their source revisions to avoid unnecessary rerenders, while the production hero has a stable static ETag because its body does not depend on presence state. A matching `If-None-Match` request receives `304 Not Modified`.

The dynamic SVG cards can be embedded through GitHub Camo. Remote Discord and Spotify raster artwork is fetched from allow-listed origins and embedded into the SVG, so the rendered card does not depend on nested external image requests. If artwork cannot be embedded, the renderer uses its deterministic fallback instead of emitting the remote image URL. The final hero banner is embedded as a local PNG data URI as well.

GitHub controls Camo caching, so dynamic cards embedded in a README should be treated as best-effort near-live views rather than a realtime channel. In the current deployment, an observed presence revision change became visible through the same Camo URL about two seconds after the origin changed; refresh timing is not guaranteed.

GitHub does not provide reliable clickable subregions inside a Camo-rendered SVG. A Discord presence card that should open the Discord profile is therefore linked by wrapping the whole image in the profile README, rather than by placing an internal SVG link over part of the graphic.

Before the first valid target presence event, the public endpoint and SVG views use a neutral waiting state. A Gateway transport failure does not fabricate an offline user state. The service keeps the last-known-good snapshot during the grace window, marks it stale after the configured stale threshold, and switches to a neutral unavailable view after the configured unavailable threshold. A real target `PRESENCE_UPDATE` restores fresh state.

The presence and GitHub LKG files contain only normalized state and collection timestamps. Credentials and session data are not persisted there.
