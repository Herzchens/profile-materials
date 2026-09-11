use std::{io, net::SocketAddr, sync::Arc};

use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;

use crate::state::{
    ActivityAssetsSnapshot, ActivityKind, ActivitySnapshot, ActivityTimestampsSnapshot,
    ClientStatusSnapshot, PresenceState, PresenceStatus, PresenceStore,
};

pub async fn serve(bind_addr: SocketAddr, store: Arc<PresenceStore>) -> io::Result<()> {
    let app = Router::new()
        .route("/v1/public/presence", get(public_presence))
        .with_state(store);
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;

    tracing::info!(%bind_addr, "HTTP listener ready");
    axum::serve(listener, app).await
}

async fn public_presence(State(store): State<Arc<PresenceStore>>) -> Json<PublicPresence> {
    Json(build_public_presence(store.load().as_ref()))
}

#[derive(Debug, Serialize)]
struct PublicPresence {
    availability: &'static str,
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

fn build_public_presence(state: &PresenceState) -> PublicPresence {
    match state {
        PresenceState::Unknown => PublicPresence {
            availability: "unknown",
            status: None,
            client_status: None,
            activities: Vec::new(),
            observed_at_unix_ms: None,
            revision: 0,
        },
        PresenceState::Known(snapshot) => PublicPresence {
            availability: "known",
            status: Some(status_name(snapshot.data.status)),
            client_status: Some(public_client_status(&snapshot.data.client_status)),
            activities: snapshot
                .data
                .activities
                .iter()
                .map(public_activity)
                .collect(),
            observed_at_unix_ms: Some(snapshot.observed_at_unix_ms),
            revision: snapshot.revision,
        },
    }
}

fn public_client_status(status: &ClientStatusSnapshot) -> PublicClientStatus {
    PublicClientStatus {
        desktop: status.desktop.map(status_name),
        mobile: status.mobile.map(status_name),
        web: status.web.map(status_name),
    }
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
        ActivityKind, ActivitySnapshot, ClientStatusSnapshot, PresenceData, PresenceSnapshot,
        PresenceState, PresenceStatus,
    };

    use super::build_public_presence;

    #[test]
    fn unknown_state_never_fabricates_offline() {
        let value = serde_json::to_value(build_public_presence(&PresenceState::Unknown)).unwrap();

        assert_eq!(value["availability"], "unknown");
        assert!(value["status"].is_null());
        assert_eq!(value["revision"], 0);
    }

    #[test]
    fn public_dto_exposes_only_selected_presence_fields() {
        let state = PresenceState::Known(PresenceSnapshot {
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
        });

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
