use crate::{
    activity::is_spotify_activity,
    spotify::{SpotifyPlayback, SpotifyPlaybackState, SpotifySnapshot},
    state::{
        ActivityAssetsSnapshot, ActivityKind, ActivitySnapshot, ActivityTimestampsSnapshot,
        ClientStatusSnapshot, PresenceData, PresenceFreshness, PresenceSnapshot, PresenceState,
        PresenceStatus, RuntimeState,
    },
};

const SPOTIFY_AUTHORITY_TTL_MS: u64 = 60_000;
const SPOTIFY_IMAGE_PREFIX: &str = "https://i.scdn.co/image/";

pub fn compose_presence(
    base: &RuntimeState,
    spotify: Option<&SpotifySnapshot>,
    now_unix_ms: u64,
) -> RuntimeState {
    let spotify = spotify.filter(|snapshot| {
        now_unix_ms.saturating_sub(snapshot.validated_at_unix_ms) <= SPOTIFY_AUTHORITY_TTL_MS
    });
    let native_activity = spotify.and_then(|snapshot| match &snapshot.playback {
        SpotifyPlaybackState::Idle => None,
        SpotifyPlaybackState::Track(playback) if playback.is_playing => Some(
            native_spotify_activity(playback, snapshot.validated_at_unix_ms),
        ),
        SpotifyPlaybackState::Track(_) => None,
    });

    match &base.presence {
        PresenceState::Known(snapshot) if base.freshness != PresenceFreshness::Unavailable => {
            let mut next = base.clone();
            let mut data = snapshot.data.clone();

            // Discord's Spotify Rich Presence is intentionally never a presentation
            // source. Native Spotify is authoritative, including the absence of a
            // track, so mobile playback does not depend on Discord RPC propagation.
            data.activities
                .retain(|activity| !is_spotify_activity(activity));
            if let Some(activity) = native_activity {
                data.activities.push(activity);
            }

            let spotify_revision = spotify.map_or(0, |snapshot| snapshot.revision);
            let spotify_observed = spotify.map_or(0, |snapshot| snapshot.validated_at_unix_ms);
            let revision = combined_revision(snapshot.revision, spotify_revision);
            next.presence = PresenceState::Known(PresenceSnapshot {
                data,
                observed_at_unix_ms: snapshot.observed_at_unix_ms.max(spotify_observed),
                revision,
            });
            next.stream_revision = combined_revision(base.stream_revision, spotify_revision);
            next
        }
        PresenceState::Known(snapshot) => {
            let Some(activity) = native_activity else {
                let mut next = base.clone();
                let mut data = snapshot.data.clone();
                data.activities
                    .retain(|activity| !is_spotify_activity(activity));
                next.presence = PresenceState::Known(PresenceSnapshot {
                    data,
                    observed_at_unix_ms: snapshot.observed_at_unix_ms,
                    revision: snapshot.revision,
                });
                return next;
            };
            let spotify = spotify.expect("native Spotify track has a fresh snapshot");
            let revision = combined_revision(base.stream_revision, spotify.revision);
            RuntimeState {
                freshness: PresenceFreshness::Fresh,
                gateway_status: base.gateway_status,
                last_target_event_unix_ms: base.last_target_event_unix_ms,
                presence: PresenceState::Known(PresenceSnapshot {
                    data: PresenceData {
                        activities: vec![activity],
                        client_status: ClientStatusSnapshot {
                            desktop: None,
                            mobile: None,
                            web: None,
                        },
                        status: PresenceStatus::Offline,
                    },
                    observed_at_unix_ms: spotify.validated_at_unix_ms,
                    revision,
                }),
                stream_revision: revision,
                unvalidated_since_unix_ms: base.unvalidated_since_unix_ms,
            }
        }
        PresenceState::Unknown => {
            let Some(activity) = native_activity else {
                return base.clone();
            };
            let spotify = spotify.expect("native Spotify track has a fresh snapshot");
            let revision = combined_revision(base.stream_revision, spotify.revision);
            RuntimeState {
                freshness: PresenceFreshness::Fresh,
                gateway_status: base.gateway_status,
                last_target_event_unix_ms: base.last_target_event_unix_ms,
                presence: PresenceState::Known(PresenceSnapshot {
                    data: PresenceData {
                        activities: vec![activity],
                        client_status: ClientStatusSnapshot {
                            desktop: None,
                            mobile: None,
                            web: None,
                        },
                        status: PresenceStatus::Offline,
                    },
                    observed_at_unix_ms: spotify.validated_at_unix_ms,
                    revision,
                }),
                stream_revision: revision,
                unvalidated_since_unix_ms: base.unvalidated_since_unix_ms,
            }
        }
    }
}

fn native_spotify_activity(
    playback: &SpotifyPlayback,
    observed_at_unix_ms: u64,
) -> ActivitySnapshot {
    let timestamps = if playback.is_playing && playback.duration_ms > 0 {
        let start = observed_at_unix_ms.saturating_sub(playback.progress_ms);
        Some(ActivityTimestampsSnapshot {
            start: Some(start),
            end: Some(start.saturating_add(playback.duration_ms)),
        })
    } else {
        None
    };

    ActivitySnapshot {
        application_id: None,
        assets: Some(ActivityAssetsSnapshot {
            large_image: playback.cover_url.as_deref().and_then(spotify_cover_asset),
            large_text: playback.album.clone(),
            small_image: None,
            small_text: None,
        }),
        details: Some(playback.title.clone()),
        kind: ActivityKind::Listening,
        name: "Spotify".to_owned(),
        party: None,
        state: (!playback.artists.is_empty()).then(|| playback.artists.clone()),
        timestamps,
    }
}

fn spotify_cover_asset(url: &str) -> Option<String> {
    let image_id = url.strip_prefix(SPOTIFY_IMAGE_PREFIX)?;
    if image_id.is_empty()
        || !image_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some(format!("spotify:{image_id}"))
}

fn combined_revision(base: u64, spotify: u64) -> u64 {
    base.wrapping_mul(1_000_003).wrapping_add(spotify)
}

#[cfg(test)]
mod tests {
    use crate::{
        spotify::{SpotifyPlayback, SpotifyPlaybackState, SpotifySnapshot},
        state::{
            ActivityKind, ActivitySnapshot, ClientStatusSnapshot, GatewayStatus, PresenceData,
            PresenceFreshness, PresenceSnapshot, PresenceState, PresenceStatus, RuntimeState,
        },
    };

    use super::compose_presence;

    fn runtime(freshness: PresenceFreshness) -> RuntimeState {
        RuntimeState {
            freshness,
            gateway_status: GatewayStatus::Live,
            last_target_event_unix_ms: Some(900),
            presence: PresenceState::Known(PresenceSnapshot {
                data: PresenceData {
                    activities: vec![
                        ActivitySnapshot {
                            application_id: None,
                            assets: None,
                            details: Some("Old Discord track".to_owned()),
                            kind: ActivityKind::Listening,
                            name: "Spotify".to_owned(),
                            party: None,
                            state: Some("Old artist".to_owned()),
                            timestamps: None,
                        },
                        ActivitySnapshot {
                            application_id: Some("1".to_owned()),
                            assets: None,
                            details: Some("Competitive".to_owned()),
                            kind: ActivityKind::Playing,
                            name: "VALORANT".to_owned(),
                            party: None,
                            state: None,
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
                observed_at_unix_ms: 900,
                revision: 7,
            }),
            stream_revision: 7,
            unvalidated_since_unix_ms: None,
        }
    }

    fn spotify(playback: SpotifyPlaybackState) -> SpotifySnapshot {
        SpotifySnapshot {
            playback,
            revision: 3,
            validated_at_unix_ms: 1_000,
        }
    }

    fn track() -> SpotifyPlaybackState {
        SpotifyPlaybackState::Track(SpotifyPlayback {
            album: Some("Album".to_owned()),
            artists: "Artist".to_owned(),
            cover_url: Some("https://i.scdn.co/image/cover".to_owned()),
            duration_ms: 180_000,
            is_playing: true,
            progress_ms: 42_000,
            title: "New native track".to_owned(),
            track_id: Some("track".to_owned()),
        })
    }

    #[test]
    fn native_spotify_replaces_discord_spotify_without_duplicates() {
        let composed = compose_presence(
            &runtime(PresenceFreshness::Fresh),
            Some(&spotify(track())),
            1_000,
        );
        let PresenceState::Known(snapshot) = composed.presence else {
            panic!("expected known presence");
        };

        assert_eq!(snapshot.data.activities.len(), 2);
        assert_eq!(snapshot.data.activities[0].name, "VALORANT");
        assert_eq!(snapshot.data.activities[1].name, "Spotify");
        assert_eq!(
            snapshot.data.activities[1].details.as_deref(),
            Some("New native track")
        );
        assert_eq!(
            snapshot.data.activities[1]
                .assets
                .as_ref()
                .and_then(|assets| assets.large_image.as_deref()),
            Some("spotify:cover")
        );
    }

    #[test]
    fn native_idle_hides_discord_spotify_instead_of_using_rpc_fallback() {
        let composed = compose_presence(
            &runtime(PresenceFreshness::Fresh),
            Some(&spotify(SpotifyPlaybackState::Idle)),
            1_000,
        );
        let PresenceState::Known(snapshot) = composed.presence else {
            panic!("expected known presence");
        };

        assert_eq!(snapshot.data.activities.len(), 1);
        assert_eq!(snapshot.data.activities[0].name, "VALORANT");
    }

    #[test]
    fn native_spotify_stays_visible_when_discord_is_unavailable() {
        let composed = compose_presence(
            &runtime(PresenceFreshness::Unavailable),
            Some(&spotify(track())),
            1_000,
        );
        let PresenceState::Known(snapshot) = composed.presence else {
            panic!("expected known presence");
        };

        assert_eq!(composed.freshness, PresenceFreshness::Fresh);
        assert_eq!(snapshot.data.status, PresenceStatus::Offline);
        assert_eq!(snapshot.data.activities.len(), 1);
        assert_eq!(snapshot.data.activities[0].name, "Spotify");
    }

    #[test]
    fn stale_native_snapshot_still_does_not_expose_discord_spotify() {
        let composed = compose_presence(
            &runtime(PresenceFreshness::Fresh),
            Some(&spotify(track())),
            70_000,
        );
        let PresenceState::Known(snapshot) = composed.presence else {
            panic!("expected known presence");
        };

        assert_eq!(snapshot.data.activities.len(), 1);
        assert_eq!(snapshot.data.activities[0].name, "VALORANT");
    }

    #[test]
    fn missing_native_snapshot_does_not_expose_discord_spotify() {
        let composed = compose_presence(&runtime(PresenceFreshness::Fresh), None, 1_000);
        let PresenceState::Known(snapshot) = composed.presence else {
            panic!("expected known presence");
        };

        assert_eq!(snapshot.data.activities.len(), 1);
        assert_eq!(snapshot.data.activities[0].name, "VALORANT");
    }
    #[test]
    fn paused_native_spotify_is_not_presented() {
        let SpotifyPlaybackState::Track(mut playback) = track() else {
            unreachable!();
        };
        playback.is_playing = false;
        let composed = compose_presence(
            &runtime(PresenceFreshness::Fresh),
            Some(&spotify(SpotifyPlaybackState::Track(playback))),
            1_000,
        );
        let PresenceState::Known(snapshot) = composed.presence else {
            panic!("expected known presence");
        };

        assert_eq!(snapshot.data.activities.len(), 1);
        assert_eq!(snapshot.data.activities[0].name, "VALORANT");
    }
}
