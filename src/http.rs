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

use crate::state::{
    ActivityAssetsSnapshot, ActivityKind, ActivitySnapshot, ActivityTimestampsSnapshot,
    ClientStatusSnapshot, GatewayStatus, PresenceFreshness, PresenceState, PresenceStatus,
    PresenceStore, RuntimeState,
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
                        r#"{"availability":"unknown","stale":false,"status":null,"client_status":null,"activities":[],"observed_at_unix_ms":null,"revision":0}"#.to_owned()
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

#[derive(Debug, Serialize)]
struct PublicPresence {
    availability: &'static str,
    stale: bool,
    status: Option<&'static str>,
    client_status: Option<PublicClientStatus>,
    activities: Vec<PublicActivity>,
    observed_at_unix_ms: Option<u64>,
    revision: u64,
}

#[derive(Debug, Serialize)]
struct PublicClientStatus {
    desktop: Option<&'static str>,
    mobile: Option<&'static str>,
    web: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct PublicActivity {
    kind: &'static str,
    name: String,
    application_id: Option<String>,
    details: Option<String>,
    state: Option<String>,
    timestamps: Option<PublicActivityTimestamps>,
    assets: Option<PublicActivityAssets>,
}

#[derive(Debug, Serialize)]
struct PublicActivityTimestamps {
    start: Option<u64>,
    end: Option<u64>,
}

#[derive(Debug, Serialize)]
struct PublicActivityAssets {
    large_image: Option<String>,
    large_text: Option<String>,
    small_image: Option<String>,
    small_text: Option<String>,
}

fn build_public_presence(runtime: &RuntimeState) -> PublicPresence {
    match &runtime.presence {
        PresenceState::Unknown => PublicPresence {
            availability: "unknown",
            stale: false,
            status: None,
            client_status: None,
            activities: Vec::new(),
            observed_at_unix_ms: None,
            revision: 0,
        },
        PresenceState::Known(snapshot) if runtime.freshness == PresenceFreshness::Unavailable => {
            PublicPresence {
                availability: "unavailable",
                stale: true,
                status: None,
                client_status: None,
                activities: Vec::new(),
                observed_at_unix_ms: Some(snapshot.observed_at_unix_ms),
                revision: snapshot.revision,
            }
        }
        PresenceState::Known(snapshot) => PublicPresence {
            availability: "known",
            stale: runtime.freshness == PresenceFreshness::Stale,
            status: Some(status_name(snapshot.data.status)),
            client_status: Some(public_client_status(&snapshot.data.client_status)),
            activities: public_activities(&snapshot.data.activities),
            observed_at_unix_ms: Some(snapshot.observed_at_unix_ms),
            revision: snapshot.revision,
        },
    }
}

fn presence_availability(runtime: &RuntimeState) -> &'static str {
    match runtime.presence {
        PresenceState::Unknown => "unknown",
        PresenceState::Known(_) if runtime.freshness == PresenceFreshness::Unavailable => {
            "unavailable"
        }
        PresenceState::Known(_) => "known",
    }
}

fn presence_is_stale(runtime: &RuntimeState) -> bool {
    matches!(
        runtime.freshness,
        PresenceFreshness::Stale | PresenceFreshness::Unavailable
    )
}

fn gateway_status_name(status: GatewayStatus) -> &'static str {
    match status {
        GatewayStatus::Degraded => "degraded",
        GatewayStatus::Live => "live",
        GatewayStatus::Starting => "starting",
    }
}

fn public_client_status(status: &ClientStatusSnapshot) -> PublicClientStatus {
    PublicClientStatus {
        desktop: status.desktop.map(status_name),
        mobile: status.mobile.map(status_name),
        web: status.web.map(status_name),
    }
}

fn public_activities(activities: &[ActivitySnapshot]) -> Vec<PublicActivity> {
    let mut selected: Vec<&ActivitySnapshot> = Vec::with_capacity(activities.len());

    for activity in activities {
        if let Some(index) = selected
            .iter()
            .position(|candidate| same_display_activity(candidate, activity))
        {
            if activity_richness(activity) > activity_richness(selected[index]) {
                selected[index] = activity;
            }
        } else {
            selected.push(activity);
        }
    }

    selected.into_iter().map(public_activity).collect()
}

fn same_display_activity(left: &ActivitySnapshot, right: &ActivitySnapshot) -> bool {
    left.name.trim().to_lowercase() == right.name.trim().to_lowercase()
}

fn activity_richness(activity: &ActivitySnapshot) -> (u8, u8, u8, u8) {
    let descriptive_fields = u8::from(non_empty(activity.details.as_deref()))
        + u8::from(non_empty(activity.state.as_deref()));
    let asset_fields = activity.assets.as_ref().map_or(0, |assets| {
        u8::from(non_empty(assets.large_image.as_deref()))
            + u8::from(non_empty(assets.large_text.as_deref()))
            + u8::from(non_empty(assets.small_image.as_deref()))
            + u8::from(non_empty(assets.small_text.as_deref()))
    });
    let timestamp_fields = activity.timestamps.as_ref().map_or(0, |timestamps| {
        u8::from(timestamps.start.is_some()) + u8::from(timestamps.end.is_some())
    });
    let application_id = u8::from(non_empty(activity.application_id.as_deref()));

    (
        descriptive_fields,
        asset_fields,
        timestamp_fields,
        application_id,
    )
}

fn non_empty(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}

fn public_activity(activity: &ActivitySnapshot) -> PublicActivity {
    PublicActivity {
        kind: activity_kind_name(activity.kind),
        name: activity.name.clone(),
        application_id: activity.application_id.clone(),
        details: activity.details.clone(),
        state: activity.state.clone(),
        timestamps: activity.timestamps.as_ref().map(public_timestamps),
        assets: activity.assets.as_ref().map(public_assets),
    }
}

fn public_timestamps(timestamps: &ActivityTimestampsSnapshot) -> PublicActivityTimestamps {
    PublicActivityTimestamps {
        start: timestamps.start,
        end: timestamps.end,
    }
}

fn public_assets(assets: &ActivityAssetsSnapshot) -> PublicActivityAssets {
    PublicActivityAssets {
        large_image: assets.large_image.clone(),
        large_text: assets.large_text.clone(),
        small_image: assets.small_image.clone(),
        small_text: assets.small_text.clone(),
    }
}

fn status_name(status: PresenceStatus) -> &'static str {
    match status {
        PresenceStatus::DoNotDisturb => "dnd",
        PresenceStatus::Idle => "idle",
        PresenceStatus::Invisible => "invisible",
        PresenceStatus::Offline => "offline",
        PresenceStatus::Online => "online",
    }
}

fn activity_kind_name(kind: ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Competing => "competing",
        ActivityKind::Custom => "custom",
        ActivityKind::Listening => "listening",
        ActivityKind::Playing => "playing",
        ActivityKind::Streaming => "streaming",
        ActivityKind::Unknown(_) => "unknown",
        ActivityKind::Watching => "watching",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::state::{
        ActivityAssetsSnapshot, ActivityKind, ActivitySnapshot, ActivityTimestampsSnapshot,
        ClientStatusSnapshot, GatewayStatus, PresenceData, PresenceFreshness, PresenceSnapshot,
        PresenceState, PresenceStatus, RuntimeState,
    };

    use super::build_public_presence;

    fn runtime(presence: PresenceState, freshness: PresenceFreshness) -> RuntimeState {
        RuntimeState {
            freshness,
            gateway_status: GatewayStatus::Live,
            last_target_event_unix_ms: Some(1234),
            presence,
            stream_revision: 8,
            unvalidated_since_unix_ms: None,
        }
    }

    #[test]
    fn unknown_state_never_fabricates_offline() {
        let value = serde_json::to_value(build_public_presence(&runtime(
            PresenceState::Unknown,
            PresenceFreshness::Unavailable,
        )))
        .unwrap();

        assert_eq!(value["availability"], "unknown");
        assert!(value["status"].is_null());
        assert_eq!(value["revision"], 0);
    }

    #[test]
    fn stale_state_keeps_lkg_but_unavailable_state_goes_neutral() {
        let snapshot = PresenceSnapshot {
            data: PresenceData {
                activities: Vec::new(),
                client_status: ClientStatusSnapshot {
                    desktop: Some(PresenceStatus::Online),
                    mobile: None,
                    web: None,
                },
                status: PresenceStatus::Online,
            },
            observed_at_unix_ms: 1234,
            revision: 7,
        };

        let stale = serde_json::to_value(build_public_presence(&runtime(
            PresenceState::Known(snapshot.clone()),
            PresenceFreshness::Stale,
        )))
        .unwrap();
        assert_eq!(stale["availability"], "known");
        assert_eq!(stale["stale"], true);
        assert_eq!(stale["status"], "online");

        let unavailable = serde_json::to_value(build_public_presence(&runtime(
            PresenceState::Known(snapshot),
            PresenceFreshness::Unavailable,
        )))
        .unwrap();
        assert_eq!(unavailable["availability"], "unavailable");
        assert!(unavailable["status"].is_null());
        assert_eq!(unavailable["activities"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn public_dto_exposes_only_selected_presence_fields() {
        let state = runtime(
            PresenceState::Known(PresenceSnapshot {
                data: PresenceData {
                    activities: vec![ActivitySnapshot {
                        application_id: Some("333".to_owned()),
                        assets: None,
                        details: Some("Editing".to_owned()),
                        kind: ActivityKind::Playing,
                        name: "Visual Studio Code".to_owned(),
                        state: Some("main.rs".to_owned()),
                        timestamps: None,
                    }],
                    client_status: ClientStatusSnapshot {
                        desktop: Some(PresenceStatus::Online),
                        mobile: None,
                        web: None,
                    },
                    status: PresenceStatus::Online,
                },
                observed_at_unix_ms: 1234,
                revision: 7,
            }),
            PresenceFreshness::Fresh,
        );

        let value = serde_json::to_value(build_public_presence(&state)).unwrap();
        assert_eq!(value["availability"], "known");
        assert_eq!(value["status"], "online");
        assert_eq!(value["activities"][0]["application_id"], "333");

        let object = value.as_object().unwrap();
        assert!(!object.contains_key("guild_id"));
        assert!(!object.contains_key("user_id"));
        assert!(!object.contains_key("session_id"));
        assert!(!contains_key_recursive(&value, "secrets"));
    }

    #[test]
    fn keeps_distinct_activities_and_selects_the_richest_same_name_activity() {
        let state = runtime(
            PresenceState::Known(PresenceSnapshot {
                data: PresenceData {
                    activities: vec![
                        ActivitySnapshot {
                            application_id: Some("basic-valorant".to_owned()),
                            assets: None,
                            details: None,
                            kind: ActivityKind::Playing,
                            name: "VALORANT".to_owned(),
                            state: None,
                            timestamps: Some(ActivityTimestampsSnapshot {
                                start: Some(10),
                                end: None,
                            }),
                        },
                        ActivitySnapshot {
                            application_id: Some("rustrover".to_owned()),
                            assets: None,
                            details: Some("Editing gateway.rs".to_owned()),
                            kind: ActivityKind::Playing,
                            name: "RustRover".to_owned(),
                            state: Some("profile-materials".to_owned()),
                            timestamps: None,
                        },
                        ActivitySnapshot {
                            application_id: Some("rich-valorant".to_owned()),
                            assets: Some(ActivityAssetsSnapshot {
                                large_image: Some("valorant-art".to_owned()),
                                large_text: None,
                                small_image: None,
                                small_text: None,
                            }),
                            details: Some("Competitive (Split) 0 - 0".to_owned()),
                            kind: ActivityKind::Playing,
                            name: "valorant".to_owned(),
                            state: None,
                            timestamps: Some(ActivityTimestampsSnapshot {
                                start: Some(20),
                                end: None,
                            }),
                        },
                        ActivitySnapshot {
                            application_id: Some("spotify".to_owned()),
                            assets: None,
                            details: Some("Track title".to_owned()),
                            kind: ActivityKind::Listening,
                            name: "Spotify".to_owned(),
                            state: Some("Artist".to_owned()),
                            timestamps: None,
                        },
                    ],
                    client_status: ClientStatusSnapshot {
                        desktop: Some(PresenceStatus::Online),
                        mobile: None,
                        web: None,
                    },
                    status: PresenceStatus::Online,
                },
                observed_at_unix_ms: 1234,
                revision: 8,
            }),
            PresenceFreshness::Fresh,
        );

        let value = serde_json::to_value(build_public_presence(&state)).unwrap();
        let activities = value["activities"].as_array().unwrap();

        assert_eq!(activities.len(), 3);
        assert_eq!(activities[0]["name"], "valorant");
        assert_eq!(activities[0]["application_id"], "rich-valorant");
        assert_eq!(activities[0]["details"], "Competitive (Split) 0 - 0");
        assert_eq!(activities[1]["name"], "RustRover");
        assert_eq!(activities[2]["name"], "Spotify");
    }

    fn contains_key_recursive(value: &Value, key: &str) -> bool {
        match value {
            Value::Object(object) => object
                .iter()
                .any(|(name, value)| name == key || contains_key_recursive(value, key)),
            Value::Array(values) => values
                .iter()
                .any(|value| contains_key_recursive(value, key)),
            _ => false,
        }
    }
}
