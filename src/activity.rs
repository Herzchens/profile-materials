use crate::state::{ActivityKind, ActivitySnapshot};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtworkSource {
    DiscordApplication,
    DiscordApplicationIcon,
    DiscordMediaProxy,
    Spotify,
    LocalFallback,
}

impl ArtworkSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DiscordApplication => "discord_application",
            Self::DiscordApplicationIcon => "discord_application_icon",
            Self::DiscordMediaProxy => "discord_media_proxy",
            Self::Spotify => "spotify",
            Self::LocalFallback => "local_fallback",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedArtwork {
    pub fallback_key: &'static str,
    pub source: ArtworkSource,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedActivityArtwork {
    pub large: ResolvedArtwork,
    pub small: Option<ResolvedArtwork>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpotifyActivity {
    pub album: Option<String>,
    pub artist: Option<String>,
    pub cover_url: Option<String>,
    pub duration_ms: Option<u64>,
    pub end_unix_ms: Option<u64>,
    pub start_unix_ms: Option<u64>,
    pub title: Option<String>,
}

pub fn resolve_activity_artwork(activity: &ActivitySnapshot) -> ResolvedActivityArtwork {
    let fallback_key = local_icon_key(&activity.name);
    let large = resolve_asset(
        activity,
        activity
            .assets
            .as_ref()
            .and_then(|assets| assets.large_image.as_deref()),
        fallback_key,
    );
    let small = activity
        .assets
        .as_ref()
        .and_then(|assets| assets.small_image.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|raw| resolve_asset(activity, Some(raw), fallback_key));

    ResolvedActivityArtwork { large, small }
}

pub fn resolve_artwork(activity: &ActivitySnapshot) -> ResolvedArtwork {
    resolve_activity_artwork(activity).large
}

fn resolve_asset(
    activity: &ActivitySnapshot,
    raw: Option<&str>,
    fallback_key: &'static str,
) -> ResolvedArtwork {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return ResolvedArtwork {
            fallback_key,
            source: ArtworkSource::LocalFallback,
            url: None,
        };
    };

    if let Some(image_id) = raw.strip_prefix("mp:")
        && valid_media_proxy_id(image_id)
    {
        return ResolvedArtwork {
            fallback_key,
            source: ArtworkSource::DiscordMediaProxy,
            url: Some(format!("https://media.discordapp.net/{image_id}")),
        };
    }

    if is_spotify_activity(activity)
        && let Some(image_id) = raw.strip_prefix("spotify:")
        && valid_simple_id(image_id)
    {
        return ResolvedArtwork {
            fallback_key,
            source: ArtworkSource::Spotify,
            url: Some(format!("https://i.scdn.co/image/{image_id}")),
        };
    }

    if let Some(icon_hash) = raw.strip_prefix("appicon:")
        && valid_simple_id(icon_hash)
        && let Some(application_id) = activity.application_id.as_deref()
        && valid_snowflake(application_id)
    {
        return ResolvedArtwork {
            fallback_key,
            source: ArtworkSource::DiscordApplicationIcon,
            url: Some(format!(
                "https://cdn.discordapp.com/app-icons/{application_id}/{icon_hash}.png?size=512"
            )),
        };
    }

    if valid_simple_id(raw)
        && let Some(application_id) = activity.application_id.as_deref()
        && valid_snowflake(application_id)
    {
        return ResolvedArtwork {
            fallback_key,
            source: ArtworkSource::DiscordApplication,
            url: Some(format!(
                "https://cdn.discordapp.com/app-assets/{application_id}/{raw}.png"
            )),
        };
    }

    ResolvedArtwork {
        fallback_key,
        source: ArtworkSource::LocalFallback,
        url: None,
    }
}

pub fn activity_preference_key(activity: &ActivitySnapshot) -> (u64, u64, u8, u8, u8, u8) {
    let timestamps = activity.timestamps.as_ref();
    let start = timestamps
        .and_then(|timestamps| timestamps.start)
        .unwrap_or(0);
    let end = timestamps
        .and_then(|timestamps| timestamps.end)
        .unwrap_or(0);
    let descriptive_fields = u8::from(non_empty(activity.details.as_deref()))
        + u8::from(non_empty(activity.state.as_deref()));
    let asset_fields = activity.assets.as_ref().map_or(0, |assets| {
        u8::from(non_empty(assets.large_image.as_deref()))
            + u8::from(non_empty(assets.large_text.as_deref()))
            + u8::from(non_empty(assets.small_image.as_deref()))
            + u8::from(non_empty(assets.small_text.as_deref()))
    });
    let timestamp_fields = timestamps.map_or(0, |timestamps| {
        u8::from(timestamps.start.is_some()) + u8::from(timestamps.end.is_some())
    });
    let application_id = u8::from(non_empty(activity.application_id.as_deref()));

    (
        start,
        end,
        descriptive_fields,
        asset_fields,
        timestamp_fields,
        application_id,
    )
}

pub fn spotify_activity(activities: &[ActivitySnapshot]) -> Option<SpotifyActivity> {
    let activity = activities
        .iter()
        .filter(|activity| is_spotify_activity(activity))
        .max_by_key(|activity| activity_preference_key(activity))?;
    let artwork = resolve_artwork(activity);
    let timestamps = activity.timestamps.as_ref();
    let start_unix_ms = timestamps.and_then(|timestamps| timestamps.start);
    let end_unix_ms = timestamps.and_then(|timestamps| timestamps.end);
    let duration_ms = match (start_unix_ms, end_unix_ms) {
        (Some(start), Some(end)) if end >= start => Some(end - start),
        _ => None,
    };

    Some(SpotifyActivity {
        album: activity
            .assets
            .as_ref()
            .and_then(|assets| trimmed_owned(assets.large_text.as_deref())),
        artist: trimmed_owned(activity.state.as_deref()),
        cover_url: artwork.url,
        duration_ms,
        end_unix_ms,
        start_unix_ms,
        title: trimmed_owned(activity.details.as_deref()),
    })
}

pub fn is_spotify_activity(activity: &ActivitySnapshot) -> bool {
    activity.kind == ActivityKind::Listening && activity.name.trim().eq_ignore_ascii_case("spotify")
}

pub fn local_icon_key(name: &str) -> &'static str {
    match name.trim().to_ascii_lowercase().as_str() {
        "spotify" => "spotify",
        "valorant" => "valorant",
        "rustrover" => "rustrover",
        "intellij idea" | "intellij idea ultimate" => "intellij",
        "pycharm" | "pycharm professional" => "pycharm",
        "visual studio code" | "code" => "vscode",
        "genshin impact" | "genshin" => "genshin",
        "soundcloud" => "soundcloud",
        _ => "activity",
    }
}

fn trimmed_owned(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn non_empty(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}

fn valid_snowflake(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_simple_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_media_proxy_id(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && value.bytes().all(|byte| {
            !byte.is_ascii_control() && !byte.is_ascii_whitespace() && !matches!(byte, b'?' | b'#')
        })
}

#[cfg(test)]
mod tests {
    use crate::state::{
        ActivityAssetsSnapshot, ActivityKind, ActivitySnapshot, ActivityTimestampsSnapshot,
    };

    use super::{
        ArtworkSource, activity_preference_key, local_icon_key, resolve_activity_artwork,
        resolve_artwork, spotify_activity,
    };

    fn activity(
        name: &str,
        kind: ActivityKind,
        application_id: Option<&str>,
        large_image: Option<&str>,
    ) -> ActivitySnapshot {
        ActivitySnapshot {
            application_id: application_id.map(ToOwned::to_owned),
            assets: large_image.map(|large_image| ActivityAssetsSnapshot {
                large_image: Some(large_image.to_owned()),
                large_text: None,
                small_image: None,
                small_text: None,
            }),
            details: None,
            kind,
            name: name.to_owned(),
            party: None,
            state: None,
            timestamps: None,
        }
    }

    #[test]
    fn resolves_discord_application_assets() {
        let activity = activity(
            "VALORANT",
            ActivityKind::Playing,
            Some("1443350165678198935"),
            Some("1514484181319811163"),
        );
        let artwork = resolve_artwork(&activity);

        assert_eq!(artwork.source, ArtworkSource::DiscordApplication);
        assert_eq!(
            artwork.url.as_deref(),
            Some(
                "https://cdn.discordapp.com/app-assets/1443350165678198935/1514484181319811163.png"
            )
        );
        assert_eq!(artwork.fallback_key, "valorant");
    }

    #[test]
    fn resolves_large_and_small_application_assets() {
        let mut activity = activity(
            "VALORANT",
            ActivityKind::Playing,
            Some("1443350165678198935"),
            Some("1514484181319811163"),
        );
        activity.assets.as_mut().unwrap().small_image = Some("987654321".to_owned());

        let artwork = resolve_activity_artwork(&activity);
        assert!(artwork.large.url.is_some());
        assert_eq!(
            artwork.small.unwrap().url.as_deref(),
            Some("https://cdn.discordapp.com/app-assets/1443350165678198935/987654321.png")
        );
    }

    #[test]
    fn resolves_discord_application_icons() {
        let activity = activity(
            "Wuthering Waves",
            ActivityKind::Playing,
            Some("1247227126416146462"),
            Some("appicon:1e7d7e9ca69ea0951467994c581f70f5"),
        );
        let artwork = resolve_artwork(&activity);

        assert_eq!(artwork.source, ArtworkSource::DiscordApplicationIcon);
        assert_eq!(
            artwork.url.as_deref(),
            Some(
                "https://cdn.discordapp.com/app-icons/1247227126416146462/1e7d7e9ca69ea0951467994c581f70f5.png?size=512"
            )
        );
    }

    #[test]
    fn resolves_discord_media_proxy_assets() {
        let activity = activity(
            "RustRover",
            ActivityKind::Playing,
            Some("1107202385799041054"),
            Some("mp:external/hash/https/raw.githubusercontent.com/icon.png"),
        );
        let artwork = resolve_artwork(&activity);

        assert_eq!(artwork.source, ArtworkSource::DiscordMediaProxy);
        assert_eq!(
            artwork.url.as_deref(),
            Some(
                "https://media.discordapp.net/external/hash/https/raw.githubusercontent.com/icon.png"
            )
        );
        assert_eq!(artwork.fallback_key, "rustrover");
    }

    #[test]
    fn resolves_spotify_cover_without_spotify_api() {
        let activity = activity(
            "Spotify",
            ActivityKind::Listening,
            None,
            Some("spotify:ab67616d0000b2730ae4f4d42e4a09f3a29f64ad"),
        );
        let artwork = resolve_artwork(&activity);

        assert_eq!(artwork.source, ArtworkSource::Spotify);
        assert_eq!(
            artwork.url.as_deref(),
            Some("https://i.scdn.co/image/ab67616d0000b2730ae4f4d42e4a09f3a29f64ad")
        );
        assert_eq!(artwork.fallback_key, "spotify");
    }

    #[test]
    fn unsafe_or_unknown_assets_fall_back_deterministically() {
        let activity = activity(
            "Mystery App",
            ActivityKind::Playing,
            Some("123"),
            Some("../../bad?asset"),
        );
        let artwork = resolve_artwork(&activity);

        assert_eq!(artwork.source, ArtworkSource::LocalFallback);
        assert!(artwork.url.is_none());
        assert_eq!(artwork.fallback_key, "activity");
        assert_eq!(local_icon_key("Visual Studio Code"), "vscode");
    }

    #[test]
    fn prefers_the_newest_duplicate_activity() {
        let mut old = activity(
            "Spotify",
            ActivityKind::Listening,
            None,
            Some("spotify:old"),
        );
        old.details = Some("Old track".to_owned());
        old.timestamps = Some(ActivityTimestampsSnapshot {
            start: Some(1_000),
            end: Some(181_000),
        });

        let mut new = activity(
            "Spotify",
            ActivityKind::Listening,
            None,
            Some("spotify:new"),
        );
        new.details = Some("New track".to_owned());
        new.timestamps = Some(ActivityTimestampsSnapshot {
            start: Some(200_000),
            end: Some(380_000),
        });

        assert!(activity_preference_key(&new) > activity_preference_key(&old));
        let spotify = spotify_activity(&[old, new]).unwrap();
        assert_eq!(spotify.title.as_deref(), Some("New track"));
        assert_eq!(spotify.start_unix_ms, Some(200_000));
    }

    #[test]
    fn normalizes_linked_spotify_activity_and_duration() {
        let mut activity = activity(
            "Spotify",
            ActivityKind::Listening,
            None,
            Some("spotify:abc123"),
        );
        activity.details = Some("  Track title  ".to_owned());
        activity.state = Some(" Artist A; Artist B ".to_owned());
        activity.assets.as_mut().unwrap().large_text = Some(" Album name ".to_owned());
        activity.timestamps = Some(ActivityTimestampsSnapshot {
            start: Some(1_000),
            end: Some(181_000),
        });

        let spotify = spotify_activity(&[activity]).unwrap();
        assert_eq!(spotify.title.as_deref(), Some("Track title"));
        assert_eq!(spotify.artist.as_deref(), Some("Artist A; Artist B"));
        assert_eq!(spotify.album.as_deref(), Some("Album name"));
        assert_eq!(spotify.duration_ms, Some(180_000));
        assert_eq!(spotify.start_unix_ms, Some(1_000));
        assert_eq!(spotify.end_unix_ms, Some(181_000));
        assert_eq!(
            spotify.cover_url.as_deref(),
            Some("https://i.scdn.co/image/abc123")
        );
    }
}
