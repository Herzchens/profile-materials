use std::{convert::Infallible, io, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{
        Html, IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use futures_util::stream::{self, Stream};
use serde::Serialize;

use crate::{
    presentation::{
        PublicPresence, build_public_presence, gateway_status_name, presence_availability,
        presence_is_stale,
    },
    state::{GatewayStatus, PresenceState, PresenceStore},
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

pub async fn serve(bind_addr: SocketAddr, store: Arc<PresenceStore>) -> io::Result<()> {
    let app = Router::new()
        .route("/v1/public/presence", get(public_presence))
        .route("/v1/live", get(live))
        .route("/debug/live", get(debug_live))
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .with_state(store);
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;

    tracing::info!(%bind_addr, "HTTP listener ready");
    axum::serve(listener, app).await
}

async fn public_presence(State(store): State<Arc<PresenceStore>>) -> Json<PublicPresence> {
    let runtime = store.load();
    Json(build_public_presence(runtime.as_ref()))
}

async fn live(
    State(store): State<Arc<PresenceStore>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = store.subscribe();
    let event_stream = stream::unfold(
        (store, receiver, true, None::<u64>),
        |(store, mut receiver, mut initial, mut last_sent_revision)| async move {
            loop {
                if !initial && receiver.changed().await.is_err() {
                    return None;
                }
                initial = false;

                let runtime = store.load();
                let stream_revision = runtime.stream_revision;
                if last_sent_revision == Some(stream_revision) {
                    continue;
                }

                let payload = match serde_json::to_string(&build_public_presence(runtime.as_ref()))
                {
                    Ok(payload) => payload,
                    Err(error) => {
                        tracing::error!(error = %error, "failed to serialize SSE presence payload");
                        r#"{"availability":"unknown","stale":false,"status":null,"client_status":null,"activities":[],"spotify":null,"observed_at_unix_ms":null,"revision":0}"#.to_owned()
                    }
                };
                last_sent_revision = Some(stream_revision);
                let event = Event::default()
                    .event("presence")
                    .id(stream_revision.to_string())
                    .data(payload);

                return Some((Ok(event), (store, receiver, initial, last_sent_revision)));
            }
        },
    );

    Sse::new(event_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

async fn debug_live() -> Html<&'static str> {
    Html(DEBUG_LIVE_HTML)
}

async fn health_live() -> Json<LiveHealth> {
    Json(LiveHealth { status: "live" })
}

async fn health_ready(State(store): State<Arc<PresenceStore>>) -> impl IntoResponse {
    let runtime = store.load();
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
