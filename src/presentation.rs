use serde::Serialize;

use crate::{
    activity::{SpotifyActivity, activity_preference_key, resolve_artwork, spotify_activity},
    state::{
        ActivityAssetsSnapshot, ActivityKind, ActivityPartySnapshot, ActivitySnapshot,
        ActivityTimestampsSnapshot, ClientStatusSnapshot, GatewayStatus, PresenceFreshness,
        PresenceState, PresenceStatus, RuntimeState,
    },
};

#[derive(Debug, Serialize)]
pub(crate) struct PublicPresence {
    availability: &'static str,
    stale: bool,
    status: Option<&'static str>,
    client_status: Option<PublicClientStatus>,
    activities: Vec<PublicActivity>,
    spotify: Option<PublicSpotify>,
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
    party: Option<PublicActivityParty>,
    timestamps: Option<PublicActivityTimestamps>,
    assets: Option<PublicActivityAssets>,
    artwork: PublicArtwork,
}

#[derive(Debug, Serialize)]
struct PublicActivityParty {
    current_size: u64,
    max_size: u64,
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

#[derive(Debug, Serialize)]
struct PublicArtwork {
    source: &'static str,
    url: Option<String>,
    fallback_key: &'static str,
}

#[derive(Debug, Serialize)]
struct PublicSpotify {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    cover_url: Option<String>,
    start_unix_ms: Option<u64>,
    end_unix_ms: Option<u64>,
    duration_ms: Option<u64>,
}

pub(crate) fn build_public_presence(runtime: &RuntimeState) -> PublicPresence {
    match &runtime.presence {
        PresenceState::Unknown => PublicPresence {
            availability: "unknown",
            stale: false,
            status: None,
            client_status: None,
            activities: Vec::new(),
            spotify: None,
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
                spotify: None,
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
            spotify: spotify_activity(&snapshot.data.activities).map(public_spotify),
            observed_at_unix_ms: Some(snapshot.observed_at_unix_ms),
            revision: snapshot.revision,
        },
    }
}

pub(crate) fn presence_availability(runtime: &RuntimeState) -> &'static str {
    match runtime.presence {
        PresenceState::Unknown => "unknown",
        PresenceState::Known(_) if runtime.freshness == PresenceFreshness::Unavailable => {
            "unavailable"
        }
        PresenceState::Known(_) => "known",
    }
}

pub(crate) fn presence_is_stale(runtime: &RuntimeState) -> bool {
    matches!(
        runtime.freshness,
        PresenceFreshness::Stale | PresenceFreshness::Unavailable
    )
}

pub(crate) fn gateway_status_name(status: GatewayStatus) -> &'static str {
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
            if activity_preference_key(activity) > activity_preference_key(selected[index]) {
                selected[index] = activity;
            }
        } else {
            selected.push(activity);
        }
    }

    selected.into_iter().map(public_activity).collect()
}

fn same_display_activity(left: &ActivitySnapshot, right: &ActivitySnapshot) -> bool {
    left.name.trim().eq_ignore_ascii_case(right.name.trim())
}

fn public_activity(activity: &ActivitySnapshot) -> PublicActivity {
    let artwork = resolve_artwork(activity);

    PublicActivity {
        kind: activity_kind_name(activity.kind),
        name: activity.name.clone(),
        application_id: activity.application_id.clone(),
        details: activity.details.clone(),
        state: activity.state.clone(),
        party: activity.party.as_ref().map(public_party),
        timestamps: activity.timestamps.as_ref().map(public_timestamps),
        assets: activity.assets.as_ref().map(public_assets),
        artwork: PublicArtwork {
            source: artwork.source.as_str(),
            url: artwork.url,
            fallback_key: artwork.fallback_key,
        },
    }
}

fn public_party(party: &ActivityPartySnapshot) -> PublicActivityParty {
    PublicActivityParty {
        current_size: party.current_size,
        max_size: party.max_size,
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

fn public_spotify(spotify: SpotifyActivity) -> PublicSpotify {
    PublicSpotify {
        title: spotify.title,
        artist: spotify.artist,
        album: spotify.album,
        cover_url: spotify.cover_url,
        start_unix_ms: spotify.start_unix_ms,
        end_unix_ms: spotify.end_unix_ms,
        duration_ms: spotify.duration_ms,
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
        ActivityAssetsSnapshot, ActivityKind, ActivityPartySnapshot, ActivitySnapshot,
        ActivityTimestampsSnapshot, ClientStatusSnapshot, GatewayStatus, PresenceData,
        PresenceFreshness, PresenceSnapshot, PresenceState, PresenceStatus, RuntimeState,
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
        assert!(value["spotify"].is_null());
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
        assert!(unavailable["spotify"].is_null());
        assert_eq!(unavailable["activities"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn public_dto_exposes_only_selected_presence_fields() {
        let state = runtime(
            PresenceState::Known(PresenceSnapshot {
                data: PresenceData {
                    activities: vec![ActivitySnapshot {
                        application_id: Some("333".to_owned()),
                        assets: Some(ActivityAssetsSnapshot {
                            large_image: Some("444".to_owned()),
                            large_text: None,
                            small_image: None,
                            small_text: None,
                        }),
                        details: Some("Editing".to_owned()),
                        kind: ActivityKind::Playing,
                        name: "Visual Studio Code".to_owned(),
                        party: Some(ActivityPartySnapshot {
                            current_size: 2,
                            max_size: 5,
                        }),
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
        assert_eq!(value["activities"][0]["party"]["current_size"], 2);
        assert_eq!(value["activities"][0]["party"]["max_size"], 5);
        assert_eq!(
            value["activities"][0]["artwork"]["url"],
            "https://cdn.discordapp.com/app-assets/333/444.png"
        );
        assert_eq!(value["activities"][0]["artwork"]["fallback_key"], "vscode");

        let object = value.as_object().unwrap();
        assert!(!object.contains_key("guild_id"));
        assert!(!object.contains_key("user_id"));
        assert!(!object.contains_key("session_id"));
        assert!(!contains_key_recursive(&value, "secrets"));
    }

    #[test]
    fn keeps_distinct_activities_selects_newest_duplicate_and_projects_spotify() {
        let state = runtime(
            PresenceState::Known(PresenceSnapshot {
                data: PresenceData {
                    activities: vec![
                        ActivitySnapshot {
                            application_id: Some("123".to_owned()),
                            assets: None,
                            details: Some("Old match".to_owned()),
                            kind: ActivityKind::Playing,
                            name: "VALORANT".to_owned(),
                            party: None,
                            state: None,
                            timestamps: Some(ActivityTimestampsSnapshot {
                                start: Some(10),
                                end: None,
                            }),
                        },
                        ActivitySnapshot {
                            application_id: Some("1107202385799041054".to_owned()),
                            assets: None,
                            details: Some("Editing gateway.rs".to_owned()),
                            kind: ActivityKind::Playing,
                            name: "RustRover".to_owned(),
                            party: None,
                            state: Some("profile-materials".to_owned()),
                            timestamps: None,
                        },
                        ActivitySnapshot {
                            application_id: Some("456".to_owned()),
                            assets: Some(ActivityAssetsSnapshot {
                                large_image: Some("789".to_owned()),
                                large_text: None,
                                small_image: None,
                                small_text: None,
                            }),
                            details: Some("Competitive (Split) 0 - 0".to_owned()),
                            kind: ActivityKind::Playing,
                            name: "valorant".to_owned(),
                            party: None,
                            state: None,
                            timestamps: Some(ActivityTimestampsSnapshot {
                                start: Some(20),
                                end: None,
                            }),
                        },
                        ActivitySnapshot {
                            application_id: None,
                            assets: Some(ActivityAssetsSnapshot {
                                large_image: Some("spotify:old".to_owned()),
                                large_text: Some("Old album".to_owned()),
                                small_image: None,
                                small_text: None,
                            }),
                            details: Some("Old track".to_owned()),
                            kind: ActivityKind::Listening,
                            name: "Spotify".to_owned(),
                            party: None,
                            state: Some("Old artist".to_owned()),
                            timestamps: Some(ActivityTimestampsSnapshot {
                                start: Some(1000),
                                end: Some(181000),
                            }),
                        },
                        ActivitySnapshot {
                            application_id: None,
                            assets: Some(ActivityAssetsSnapshot {
                                large_image: Some("spotify:new".to_owned()),
                                large_text: Some("New album".to_owned()),
                                small_image: None,
                                small_text: None,
                            }),
                            details: Some("New track".to_owned()),
                            kind: ActivityKind::Listening,
                            name: "spotify".to_owned(),
                            party: None,
                            state: Some("New artist".to_owned()),
                            timestamps: Some(ActivityTimestampsSnapshot {
                                start: Some(200000),
                                end: Some(380000),
                            }),
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
        assert_eq!(activities[0]["application_id"], "456");
        assert_eq!(activities[0]["details"], "Competitive (Split) 0 - 0");
        assert_eq!(activities[1]["name"], "RustRover");
        assert_eq!(activities[2]["name"], "spotify");
        assert_eq!(activities[2]["details"], "New track");
        assert_eq!(value["spotify"]["title"], "New track");
        assert_eq!(value["spotify"]["artist"], "New artist");
        assert_eq!(value["spotify"]["album"], "New album");
        assert_eq!(value["spotify"]["duration_ms"], 180000);
        assert_eq!(value["spotify"]["cover_url"], "https://i.scdn.co/image/new");
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
