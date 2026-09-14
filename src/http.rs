use std::{convert::Infallible, io, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH},
    },
    response::{
        Html, IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use futures_util::stream::{self, Stream};
use serde::{Deserialize, Serialize};

use crate::{
    clock,
    discord::{detectable::DetectableAppCatalog, identity::IdentityStore},
    github::{
        GitHubStore,
        cards::{GitHubCardDocument, GitHubCardKind, render_card},
        stats::{GitHubPublicResponse, GitHubSvgDocument, render_svg as render_github_summary_svg},
    },
    hero::{HeroDocument, HeroRenderer},
    presence_card::{PresenceCardDocument, PresenceCardRenderer},
    presence_compose::compose_presence,
    presentation::{
        PublicPresence, build_public_presence, gateway_status_name, presence_availability,
        presence_is_stale,
    },
    spotify::SpotifyStore,
    state::{GatewayStatus, PresenceState, PresenceStore},
    svg::{SpotifyLayout, SvgDocument, SvgRenderer},
};

const DEBUG_LIVE_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>ItzHerzchen live presence debug</title>
  <style>
    body { font-family: ui-monospace, SFMono-Regular, Consolas, monospace; margin: 2rem; }
    pre { white-space: pre-wrap; word-break: break-word; }
  </style>
</head>
<body>
  <h1>Live presence debug</h1>
  <p id="connection">connecting...</p>
  <pre id="payload">waiting for SSE...</pre>
  <script>
    const connection = document.getElementById('connection');
    const payload = document.getElementById('payload');
    const source = new EventSource('/v1/live');
    source.addEventListener('presence', (event) => {
      connection.textContent = `connected · event ${event.lastEventId || '?'}`;
      payload.textContent = JSON.stringify(JSON.parse(event.data), null, 2);
    });
    source.onerror = () => {
      connection.textContent = 'reconnecting...';
    };
  </script>
</body>
</html>
"#;

const LIVE_PRESENCE_HTML: &str = r###"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>ItzHerzchen live presence</title>
  <style>
    html, body {
      margin: 0;
      min-height: 100%;
      background: #0f1320;
    }
    body {
      display: grid;
      place-items: start center;
    }
    #card {
      width: min(100vw, 1774px);
    }
    #card svg {
      display: block;
      width: 100%;
      height: auto;
    }
  </style>
</head>
<body>
  <main id="card" aria-live="polite"></main>
  <script>
    const card = document.getElementById('card');
    let refreshGeneration = 0;
    let refreshInFlight = false;
    let lastSpotifyExpiryRefresh = 0;

    function pad(value) {
      return String(value).padStart(2, '0');
    }

    function formatDuration(milliseconds) {
      const secondsTotal = Math.max(0, Math.floor(milliseconds / 1000));
      const hours = Math.floor(secondsTotal / 3600);
      const minutes = Math.floor((secondsTotal % 3600) / 60);
      const seconds = secondsTotal % 60;
      return hours > 0
        ? `${pad(hours)}:${pad(minutes)}:${pad(seconds)}`
        : `${pad(minutes)}:${pad(seconds)}`;
    }

    function refreshAfterSpotifyExpiry(now) {
      if (now - lastSpotifyExpiryRefresh < 3000 || refreshInFlight) return;
      lastSpotifyExpiryRefresh = now;
      refreshPresenceParts().catch(console.error);
    }

    function tickTimers() {
      const svg = card.querySelector('svg');
      if (!svg) return;

      const now = Date.now();
      svg.querySelectorAll('[data-time-kind]').forEach((node) => {
        const kind = node.dataset.timeKind;

        if (kind === 'elapsed') {
          const anchor = Number(node.dataset.anchorMs);
          if (Number.isFinite(anchor)) {
            node.textContent = formatDuration(now - anchor);
          }
          return;
        }

        if (kind === 'remaining') {
          const anchor = Number(node.dataset.anchorMs);
          if (Number.isFinite(anchor)) {
            node.textContent = formatDuration(anchor - now);
          }
          return;
        }

        if (kind === 'spotify' || kind === 'spotify-current' || kind === 'media-current') {
          const start = Number(node.dataset.startMs);
          const end = Number(node.dataset.endMs);
          if (!Number.isFinite(start) || !Number.isFinite(end) || end <= start) return;

          const duration = end - start;
          const elapsed = Math.min(duration, Math.max(0, now - start));
          node.textContent = (kind === 'spotify-current' || kind === 'media-current')
            ? formatDuration(elapsed)
            : `${formatDuration(elapsed)} / ${formatDuration(duration)}`;

          const progressId = node.dataset.progressId;
          const progress = progressId ? document.getElementById(progressId) : null;
          if (progress) {
            const maxWidth = Number(progress.dataset.maxWidth);
            if (Number.isFinite(maxWidth)) {
              progress.setAttribute('width', String(Math.round(maxWidth * (elapsed / duration))));
            }
          }

          if (now >= end) {
            refreshAfterSpotifyExpiry(now);
          }
        }
      });
    }

    async function fetchCardSvg() {
      const response = await fetch(`/v1/svg/presence.svg?live=${Date.now()}`, {
        cache: 'no-store',
      });
      if (!response.ok) {
        throw new Error(`presence SVG returned HTTP ${response.status}`);
      }

      const text = await response.text();
      const documentSvg = new DOMParser().parseFromString(text, 'image/svg+xml');
      if (documentSvg.querySelector('parsererror')) {
        throw new Error('presence SVG could not be parsed');
      }
      return documentSvg.documentElement;
    }

    async function loadInitialCard() {
      const generation = ++refreshGeneration;
      const fresh = await fetchCardSvg();
      if (generation !== refreshGeneration) return;
      card.replaceChildren(document.importNode(fresh, true));
      tickTimers();
    }

    function replaceGroup(currentSvg, freshSvg, id) {
      const current = currentSvg.querySelector(`#${id}`);
      const fresh = freshSvg.querySelector(`#${id}`);
      if (current && fresh) {
        current.replaceWith(document.importNode(fresh, true));
      }
    }

    async function refreshPresenceParts() {
      if (refreshInFlight) return;
      refreshInFlight = true;
      const generation = ++refreshGeneration;

      try {
        const freshSvg = await fetchCardSvg();
        if (generation !== refreshGeneration) return;

        const currentSvg = card.querySelector('svg');
        if (!currentSvg) {
          card.replaceChildren(document.importNode(freshSvg, true));
          tickTimers();
          return;
        }

        for (const attribute of ['width', 'height', 'viewBox']) {
          const value = freshSvg.getAttribute(attribute);
          if (value !== null) currentSvg.setAttribute(attribute, value);
        }

        replaceGroup(currentSvg, freshSvg, 'layout-shell');
        replaceGroup(currentSvg, freshSvg, 'presence-header-dynamic');
        replaceGroup(currentSvg, freshSvg, 'activity-grid');
        tickTimers();
      } finally {
        refreshInFlight = false;
      }
    }

    loadInitialCard().catch(console.error);
    setInterval(tickTimers, 1000);
    setInterval(() => {
      refreshPresenceParts().catch(console.error);
    }, 5000);

    const source = new EventSource('/v1/live');
    source.addEventListener('presence', () => {
      refreshPresenceParts().catch(console.error);
    });
  </script>
</body>
</html>
"###;

#[derive(Clone)]
struct HttpState {
    detectable: Arc<DetectableAppCatalog>,
    github: Arc<GitHubStore>,
    github_stale_after: Duration,
    hero: Arc<HeroRenderer>,
    identity: Arc<IdentityStore>,
    presence_card: Arc<PresenceCardRenderer>,
    renderer: Arc<SvgRenderer>,
    spotify: Arc<SpotifyStore>,
    store: Arc<PresenceStore>,
}

pub async fn serve(
    bind_addr: SocketAddr,
    store: Arc<PresenceStore>,
    identity: Arc<IdentityStore>,
    spotify: Arc<SpotifyStore>,
    github: Arc<GitHubStore>,
    github_stale_after: Duration,
) -> io::Result<()> {
    let state = HttpState {
        detectable: Arc::new(DetectableAppCatalog::new().map_err(io::Error::other)?),
        github,
        github_stale_after,
        hero: Arc::new(HeroRenderer::new().map_err(io::Error::other)?),
        identity,
        presence_card: Arc::new(PresenceCardRenderer::new().map_err(io::Error::other)?),
        renderer: Arc::new(SvgRenderer::new().map_err(io::Error::other)?),
        spotify,
        store,
    };
    let app = Router::new()
        .route("/v1/public/presence", get(public_presence))
        .route("/v1/public/github", get(public_github))
        .route("/v1/live", get(live))
        .route("/v1/svg/hero.svg", get(hero_svg))
        .route("/v1/svg/hero-test.svg", get(hero_test_svg))
        .route("/v1/svg/presence.svg", get(presence_svg))
        .route("/v1/svg/spotify.svg", get(spotify_svg))
        .route("/v1/svg/github.svg", get(github_summary_svg))
        .route("/v1/svg/github-stats.svg", get(github_stats_svg))
        .route("/v1/svg/github-languages.svg", get(github_languages_svg))
        .route("/v1/svg/github-streak.svg", get(github_streak_svg))
        .route("/presence", get(live_presence))
        .route("/live/presence", get(live_presence))
        .route("/debug/live", get(debug_live))
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;

    tracing::info!(%bind_addr, "HTTP listener ready");
    axum::serve(listener, app).await
}

async fn public_presence(State(state): State<HttpState>) -> Json<PublicPresence> {
    let runtime = state.store.load();
    let spotify = state.spotify.load();
    let composed = compose_presence(runtime.as_ref(), spotify.as_deref(), current_unix_ms());
    let enriched = state.detectable.enrich(&composed).await;
    Json(build_public_presence(&enriched))
}

async fn public_github(State(state): State<HttpState>) -> Json<GitHubPublicResponse> {
    Json(GitHubPublicResponse::new(
        state.github.load(),
        current_unix_ms(),
        state.github_stale_after,
    ))
}

async fn live(
    State(state): State<HttpState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let presence_receiver = state.store.subscribe();
    let spotify_receiver = state.spotify.subscribe();
    let event_stream = stream::unfold(
        (
            state.store,
            state.spotify,
            presence_receiver,
            spotify_receiver,
            true,
            None::<(u64, u64)>,
        ),
        |(
            store,
            spotify_store,
            mut presence_receiver,
            mut spotify_receiver,
            mut initial,
            mut last_sent_revision,
        )| async move {
            loop {
                if !initial {
                    tokio::select! {
                        result = presence_receiver.changed() => {
                            if result.is_err() {
                                return None;
                            }
                        }
                        result = spotify_receiver.changed() => {
                            if result.is_err() {
                                return None;
                            }
                        }
                    }
                }
                initial = false;

                let runtime = store.load();
                let spotify = spotify_store.load();
                let spotify_revision = spotify.as_deref().map_or(0, |snapshot| snapshot.revision);
                let revision = (runtime.stream_revision, spotify_revision);
                if last_sent_revision == Some(revision) {
                    continue;
                }

                let composed =
                    compose_presence(runtime.as_ref(), spotify.as_deref(), current_unix_ms());
                let payload = match serde_json::to_string(&build_public_presence(&composed)) {
                    Ok(payload) => payload,
                    Err(error) => {
                        tracing::error!(error = %error, "failed to serialize SSE presence payload");
                        r#"{"availability":"unknown","stale":false,"status":null,"client_status":null,"activities":[],"spotify":null,"observed_at_unix_ms":null,"revision":0}"#.to_owned()
                    }
                };
                last_sent_revision = Some(revision);
                let event = Event::default()
                    .event("presence")
                    .id(format!("{}-{}", revision.0, revision.1))
                    .data(payload);

                return Some((
                    Ok(event),
                    (
                        store,
                        spotify_store,
                        presence_receiver,
                        spotify_receiver,
                        initial,
                        last_sent_revision,
                    ),
                ));
            }
        },
    );

    Sse::new(event_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

async fn hero_svg(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    let runtime = state.store.load();
    hero_svg_response(state.hero.render(runtime.as_ref()).await, &headers)
}

async fn hero_test_svg(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    let runtime = state.store.load();
    svg_response(state.renderer.render_hero_test(runtime.as_ref()), &headers)
}

async fn presence_svg(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    let runtime = state.store.load();
    let spotify = state.spotify.load();
    let composed = compose_presence(runtime.as_ref(), spotify.as_deref(), current_unix_ms());
    let enriched = state.detectable.enrich(&composed).await;
    let identity = state.identity.load();
    presence_card_svg_response(
        state
            .presence_card
            .render(&enriched, identity.as_deref())
            .await,
        &headers,
    )
}

#[derive(Debug, Deserialize)]
struct SpotifySvgQuery {
    layout: Option<String>,
}

async fn spotify_svg(
    State(state): State<HttpState>,
    Query(query): Query<SpotifySvgQuery>,
    headers: HeaderMap,
) -> Response {
    let layout = match query.layout.as_deref() {
        None => SpotifyLayout::Compact,
        Some(value) => match SpotifyLayout::parse(value) {
            Some(layout) => layout,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    "layout must be one of: mini, compact, wide",
                )
                    .into_response();
            }
        },
    };
    let runtime = state.store.load();
    let spotify = state.spotify.load();
    let composed = compose_presence(runtime.as_ref(), spotify.as_deref(), current_unix_ms());
    svg_response(
        state.renderer.render_spotify(&composed, layout).await,
        &headers,
    )
}

async fn github_summary_svg(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    let snapshot = state.github.load();
    let document = render_github_summary_svg(
        snapshot.as_deref(),
        current_unix_ms(),
        state.github_stale_after,
    );
    github_summary_svg_response(&document, &headers)
}

async fn github_stats_svg(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    github_card_response(&state, GitHubCardKind::Stats, &headers)
}

async fn github_languages_svg(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    github_card_response(&state, GitHubCardKind::Languages, &headers)
}

async fn github_streak_svg(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    github_card_response(&state, GitHubCardKind::Streak, &headers)
}

fn github_card_response(state: &HttpState, kind: GitHubCardKind, headers: &HeaderMap) -> Response {
    let snapshot = state.github.load();
    let document = render_card(
        kind,
        snapshot.as_deref(),
        current_unix_ms(),
        state.github_stale_after,
    );
    github_card_svg_response(&document, headers)
}

fn hero_svg_response(document: Arc<HeroDocument>, request_headers: &HeaderMap) -> Response {
    let mut response_headers = base_svg_headers();
    if let Ok(etag) = HeaderValue::from_str(document.etag()) {
        response_headers.insert(ETAG, etag);
    }
    if let Ok(revision) = HeaderValue::from_str(&document.stream_revision().to_string()) {
        response_headers.insert("x-profile-stream-revision", revision);
    }

    if etag_matches(request_headers, document.etag()) {
        return (StatusCode::NOT_MODIFIED, response_headers, "").into_response();
    }

    (response_headers, document.body().to_owned()).into_response()
}

fn presence_card_svg_response(
    document: Arc<PresenceCardDocument>,
    request_headers: &HeaderMap,
) -> Response {
    let mut response_headers = base_svg_headers();
    if let Ok(etag) = HeaderValue::from_str(document.etag()) {
        response_headers.insert(ETAG, etag);
    }
    if let Ok(revision) = HeaderValue::from_str(&document.stream_revision().to_string()) {
        response_headers.insert("x-profile-stream-revision", revision);
    }

    if etag_matches(request_headers, document.etag()) {
        return (StatusCode::NOT_MODIFIED, response_headers, "").into_response();
    }

    (response_headers, document.body().to_owned()).into_response()
}

fn svg_response(document: Arc<SvgDocument>, request_headers: &HeaderMap) -> Response {
    let mut response_headers = base_svg_headers();
    if let Ok(etag) = HeaderValue::from_str(document.etag()) {
        response_headers.insert(ETAG, etag);
    }
    if let Ok(revision) = HeaderValue::from_str(&document.stream_revision().to_string()) {
        response_headers.insert("x-profile-stream-revision", revision);
    }

    if etag_matches(request_headers, document.etag()) {
        return (StatusCode::NOT_MODIFIED, response_headers, "").into_response();
    }

    (response_headers, document.body().to_owned()).into_response()
}

fn github_card_svg_response(
    document: &GitHubCardDocument,
    request_headers: &HeaderMap,
) -> Response {
    let mut response_headers = base_svg_headers();
    if let Ok(etag) = HeaderValue::from_str(document.etag()) {
        response_headers.insert(ETAG, etag);
    }
    if let Ok(revision) = HeaderValue::from_str(&document.revision().to_string()) {
        response_headers.insert("x-profile-github-revision", revision);
    }

    if etag_matches(request_headers, document.etag()) {
        return (StatusCode::NOT_MODIFIED, response_headers, "").into_response();
    }

    (response_headers, document.body().to_owned()).into_response()
}

fn github_summary_svg_response(
    document: &GitHubSvgDocument,
    request_headers: &HeaderMap,
) -> Response {
    let mut response_headers = base_svg_headers();
    if let Ok(etag) = HeaderValue::from_str(document.etag()) {
        response_headers.insert(ETAG, etag);
    }
    if let Ok(revision) = HeaderValue::from_str(&document.revision().to_string()) {
        response_headers.insert("x-profile-github-revision", revision);
    }

    if etag_matches(request_headers, document.etag()) {
        return (StatusCode::NOT_MODIFIED, response_headers, "").into_response();
    }

    (response_headers, document.body().to_owned()).into_response()
}

fn base_svg_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("no-cache, max-age=0, must-revalidate"),
    );
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("image/svg+xml; charset=utf-8"),
    );
    headers
}

fn etag_matches(headers: &HeaderMap, current_etag: &str) -> bool {
    headers
        .get_all(IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| {
            candidate == "*" || normalize_etag(candidate) == normalize_etag(current_etag)
        })
}

fn normalize_etag(value: &str) -> &str {
    value.strip_prefix("W/").unwrap_or(value)
}

fn current_unix_ms() -> u64 {
    match clock::unix_time_millis() {
        Ok(now) => now,
        Err(error) => {
            tracing::warn!(error = %error, "system time unavailable while serving HTTP response");
            0
        }
    }
}

async fn live_presence() -> Html<&'static str> {
    Html(LIVE_PRESENCE_HTML)
}

async fn debug_live() -> Html<&'static str> {
    Html(DEBUG_LIVE_HTML)
}

async fn health_live() -> Json<LiveHealth> {
    Json(LiveHealth { status: "live" })
}

async fn health_ready(State(state): State<HttpState>) -> impl IntoResponse {
    let runtime = state.store.load();
    let ready = runtime.gateway_status == GatewayStatus::Live
        || matches!(runtime.presence, PresenceState::Known(_));
    let status_code = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = ReadyHealth {
        status: if ready { "ready" } else { "starting" },
        gateway: gateway_status_name(runtime.gateway_status),
        presence_availability: presence_availability(runtime.as_ref()),
        stale: presence_is_stale(runtime.as_ref()),
        last_target_event_unix_ms: runtime.last_target_event_unix_ms,
    };

    (status_code, Json(body))
}

#[derive(Debug, Serialize)]
struct LiveHealth {
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct ReadyHealth {
    status: &'static str,
    gateway: &'static str,
    presence_availability: &'static str,
    stale: bool,
    last_target_event_unix_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header::IF_NONE_MATCH};

    use super::{LIVE_PRESENCE_HTML, etag_matches};

    #[test]
    fn if_none_match_accepts_strong_weak_and_list_matches() {
        let mut headers = HeaderMap::new();
        headers.insert(
            IF_NONE_MATCH,
            HeaderValue::from_static("\"other\", W/\"profile-svg-v1-presence-8\""),
        );

        assert!(etag_matches(&headers, "\"profile-svg-v1-presence-8\""));
        assert!(!etag_matches(&headers, "\"profile-svg-v1-presence-9\""));
    }

    #[test]
    fn live_presence_page_uses_sse_timers_and_safety_refresh() {
        assert!(LIVE_PRESENCE_HTML.contains("EventSource('/v1/live')"));
        assert!(LIVE_PRESENCE_HTML.contains("setInterval(tickTimers, 1000)"));
        assert!(LIVE_PRESENCE_HTML.contains("refreshPresenceParts().catch(console.error)"));
        assert!(LIVE_PRESENCE_HTML.contains("presence-header-dynamic"));
        assert!(LIVE_PRESENCE_HTML.contains("activity-grid"));
    }
}
