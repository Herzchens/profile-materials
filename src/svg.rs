use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::{
    activity::{ResolvedArtwork, SpotifyActivity, resolve_artwork, spotify_activity},
    artwork_embed::{ArtworkEmbedder, EmbeddedArtworkBatch},
    state::{
        ActivityKind, ActivitySnapshot, PresenceFreshness, PresenceState, PresenceStatus,
        RuntimeState,
    },
};

const RENDERER_REVISION: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SpotifyLayout {
    Mini,
    Compact,
    Wide,
}

impl SpotifyLayout {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "mini" => Some(Self::Mini),
            "compact" => Some(Self::Compact),
            "wide" => Some(Self::Wide),
            _ => None,
        }
    }

    const fn cache_key(self) -> &'static str {
        match self {
            Self::Mini => "spotify-mini",
            Self::Compact => "spotify-compact",
            Self::Wide => "spotify-wide",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum SvgVariant {
    HeroTest,
    Presence,
    Spotify(SpotifyLayout),
}

impl SvgVariant {
    const fn cache_key(self) -> &'static str {
        match self {
            Self::HeroTest => "hero-test",
            Self::Presence => "presence",
            Self::Spotify(layout) => layout.cache_key(),
        }
    }
}

#[derive(Debug)]
pub struct SvgDocument {
    body: String,
    etag: String,
    stream_revision: u64,
}

impl SvgDocument {
    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn etag(&self) -> &str {
        &self.etag
    }

    pub const fn stream_revision(&self) -> u64 {
        self.stream_revision
    }
}

pub struct SvgRenderer {
    artwork: ArtworkEmbedder,
    cache: Mutex<HashMap<SvgVariant, Arc<SvgDocument>>>,
}

impl SvgRenderer {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            artwork: ArtworkEmbedder::new()?,
            cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn render_hero_test(&self, runtime: &RuntimeState) -> Arc<SvgDocument> {
        if let Some(document) = self.cached(runtime, SvgVariant::HeroTest) {
            return document;
        }

        self.finish_render(
            runtime,
            SvgVariant::HeroTest,
            &EmbeddedArtworkBatch::default(),
        )
    }

    pub async fn render_presence(&self, runtime: &RuntimeState) -> Arc<SvgDocument> {
        if let Some(document) = self.cached(runtime, SvgVariant::Presence) {
            return document;
        }

        let urls = presence_artwork_urls(runtime);
        let artwork = self.artwork.embed_all(&urls).await;
        self.finish_render(runtime, SvgVariant::Presence, &artwork)
    }

    pub async fn render_spotify(
        &self,
        runtime: &RuntimeState,
        layout: SpotifyLayout,
    ) -> Arc<SvgDocument> {
        let variant = SvgVariant::Spotify(layout);
        if let Some(document) = self.cached(runtime, variant) {
            return document;
        }

        let urls = spotify_artwork_urls(runtime);
        let artwork = self.artwork.embed_all(&urls).await;
        self.finish_render(runtime, variant, &artwork)
    }

    fn cached(&self, runtime: &RuntimeState, variant: SvgVariant) -> Option<Arc<SvgDocument>> {
        let cache = match self.cache.lock() {
            Ok(cache) => cache,
            Err(poisoned) => {
                tracing::warn!("SVG render cache mutex was poisoned; recovering cached entries");
                poisoned.into_inner()
            }
        };

        cache.get(&variant).and_then(|document| {
            (document.stream_revision == runtime.stream_revision).then(|| Arc::clone(document))
        })
    }

    fn finish_render(
        &self,
        runtime: &RuntimeState,
        variant: SvgVariant,
        artwork: &EmbeddedArtworkBatch,
    ) -> Arc<SvgDocument> {
        let body = match variant {
            SvgVariant::HeroTest => render_hero_test(runtime),
            SvgVariant::Presence => render_presence_card(runtime, artwork),
            SvgVariant::Spotify(layout) => render_spotify_card(runtime, layout, artwork),
        };

        let artwork_state = if artwork.complete() {
            "full"
        } else {
            "partial"
        };

        let document = Arc::new(SvgDocument {
            body,
            etag: format!(
                "\"profile-svg-v{RENDERER_REVISION}-{}-{}-{artwork_state}\"",
                variant.cache_key(),
                runtime.stream_revision
            ),
            stream_revision: runtime.stream_revision,
        });

        if !artwork.complete() {
            return document;
        }

        let mut cache = match self.cache.lock() {
            Ok(cache) => cache,
            Err(poisoned) => {
                tracing::warn!("SVG render cache mutex was poisoned; recovering cached entries");
                poisoned.into_inner()
            }
        };

        if let Some(existing) = cache.get(&variant)
            && existing.stream_revision == runtime.stream_revision
        {
            return Arc::clone(existing);
        }

        cache.insert(variant, Arc::clone(&document));
        document
    }
}

fn render_hero_test(runtime: &RuntimeState) -> String {
    let mut body = String::with_capacity(2_048);
    let (status, revision, activity_line, freshness) = match visible_snapshot(runtime) {
        VisibleSnapshot::Known {
            activities,
            snapshot_revision,
            stale,
            status,
        } => {
            let names = activities
                .iter()
                .map(|activity| activity.name.trim())
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            (
                status_name(status),
                snapshot_revision,
                if names.is_empty() {
                    "No current activity".to_owned()
                } else {
                    names
                },
                if stale { "last known" } else { "live" },
            )
        }
        VisibleSnapshot::Unknown => (
            "unknown",
            0,
            "Waiting for Discord presence".to_owned(),
            "waiting",
        ),
        VisibleSnapshot::Unavailable { snapshot_revision } => (
            "unavailable",
            snapshot_revision,
            "Presence temporarily unavailable".to_owned(),
            "unavailable",
        ),
    };

    push_svg_open(&mut body, 960, 180, "ItzHerzchen realtime renderer test");
    body.push_str(r##"<rect width="960" height="180" rx="24" fill="#0b1020"/>"##);
    body.push_str(
        r##"<rect x="1" y="1" width="958" height="178" rx="23" fill="none" stroke="#293659"/>"##,
    );
    body.push_str(r##"<text x="36" y="58" fill="#f4f7ff" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="26" font-weight="700">ItzHerzchen realtime renderer</text>"##);
    body.push_str(&format!(
        r##"<text x="36" y="96" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="17">status: {} · {} · revision {} · stream {}</text>"##,
        escape_xml(status),
        escape_xml(freshness),
        revision,
        runtime.stream_revision
    ));
    body.push_str(&format!(
        r##"<text x="36" y="137" fill="#d7def2" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="18">{}</text>"##,
        escape_xml(&truncate_chars(&activity_line, 90))
    ));
    body.push_str("</svg>");
    body
}

fn render_presence_card(runtime: &RuntimeState, embedded: &EmbeddedArtworkBatch) -> String {
    let visible = visible_snapshot(runtime);
    let activity_count = match &visible {
        VisibleSnapshot::Known { activities, .. } => activities.len(),
        _ => 0,
    };
    let rows = activity_count.max(1);
    let height = 112_u32.saturating_add(u32::try_from(rows).unwrap_or(u32::MAX).saturating_mul(58));
    let mut body = String::with_capacity(4_096 + activity_count.saturating_mul(768));

    push_svg_open(&mut body, 760, height, "ItzHerzchen Discord presence");
    body.push_str(&format!(
        r##"<rect width="760" height="{height}" rx="22" fill="#0b1020"/>"##
    ));
    body.push_str(&format!(
        r##"<rect x="1" y="1" width="758" height="{}" rx="21" fill="none" stroke="#293659"/>"##,
        height.saturating_sub(2)
    ));
    body.push_str(r##"<text x="28" y="42" fill="#f4f7ff" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="22" font-weight="700">Discord presence</text>"##);

    match visible {
        VisibleSnapshot::Known {
            activities,
            snapshot_revision,
            stale,
            status,
        } => {
            body.push_str(&format!(
                r##"<text x="28" y="70" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="15">{} · revision {}</text>"##,
                escape_xml(status_name(status)),
                snapshot_revision
            ));
            if stale {
                body.push_str(r##"<text x="728" y="42" text-anchor="end" fill="#f2c879" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="14">last known</text>"##);
            }

            if activities.is_empty() {
                body.push_str(r##"<text x="28" y="126" fill="#c4cee8" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="17">No current activity</text>"##);
            } else {
                for (index, activity) in activities.iter().enumerate() {
                    let row_y = 94_u32.saturating_add(
                        u32::try_from(index).unwrap_or(u32::MAX).saturating_mul(58),
                    );
                    render_activity_row(&mut body, activity, row_y, embedded);
                }
            }
        }
        VisibleSnapshot::Unknown => {
            body.push_str(r##"<text x="28" y="70" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="15">waiting</text>"##);
            body.push_str(r##"<text x="28" y="126" fill="#c4cee8" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="17">Waiting for Discord presence</text>"##);
        }
        VisibleSnapshot::Unavailable { snapshot_revision } => {
            body.push_str(&format!(
                r##"<text x="28" y="70" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="15">unavailable · last revision {snapshot_revision}</text>"##
            ));
            body.push_str(r##"<text x="28" y="126" fill="#c4cee8" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="17">Presence temporarily unavailable</text>"##);
        }
    }

    body.push_str("</svg>");
    body
}

fn render_activity_row(
    body: &mut String,
    activity: &ActivitySnapshot,
    row_y: u32,
    embedded: &EmbeddedArtworkBatch,
) {
    let artwork = resolve_artwork(activity);
    render_artwork(body, &artwork, 28, row_y, 42, embedded);

    body.push_str(&format!(
        r##"<text x="84" y="{}" fill="#f4f7ff" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="17" font-weight="600">{}</text>"##,
        row_y.saturating_add(17),
        escape_xml(&truncate_chars(activity.name.trim(), 52))
    ));

    let detail = activity_detail(activity);
    body.push_str(&format!(
        r##"<text x="84" y="{}" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="14">{}</text>"##,
        row_y.saturating_add(38),
        escape_xml(&truncate_chars(&detail, 72))
    ));
}

fn render_spotify_card(
    runtime: &RuntimeState,
    layout: SpotifyLayout,
    embedded: &EmbeddedArtworkBatch,
) -> String {
    let (width, height, cover_size, title_size, x_text) = match layout {
        SpotifyLayout::Mini => (420_u32, 96_u32, 64_u32, 16_u32, 94_u32),
        SpotifyLayout::Compact => (560_u32, 128_u32, 92_u32, 18_u32, 126_u32),
        SpotifyLayout::Wide => (720_u32, 152_u32, 116_u32, 21_u32, 154_u32),
    };
    let mut body = String::with_capacity(3_072);

    push_svg_open(&mut body, width, height, "ItzHerzchen Spotify activity");
    body.push_str(&format!(
        r##"<rect width="{width}" height="{height}" rx="20" fill="#0b1020"/>"##
    ));
    body.push_str(&format!(
        r##"<rect x="1" y="1" width="{}" height="{}" rx="19" fill="none" stroke="#293659"/>"##,
        width.saturating_sub(2),
        height.saturating_sub(2)
    ));

    match visible_spotify(runtime) {
        VisibleSpotify::Track { spotify, stale } => {
            render_spotify_cover(&mut body, &spotify, 16, 16, cover_size, embedded);
            let title = spotify.title.as_deref().unwrap_or("Unknown track");
            let artist = spotify.artist.as_deref().unwrap_or("Unknown artist");
            body.push_str(&format!(
                r##"<text x="{x_text}" y="{}" fill="#f4f7ff" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="{title_size}" font-weight="700">{}</text>"##,
                if layout == SpotifyLayout::Mini { 38 } else { 42 },
                escape_xml(&truncate_chars(title, if layout == SpotifyLayout::Wide { 52 } else { 36 }))
            ));
            body.push_str(&format!(
                r##"<text x="{x_text}" y="{}" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="14">{}</text>"##,
                if layout == SpotifyLayout::Mini { 63 } else { 68 },
                escape_xml(&truncate_chars(artist, if layout == SpotifyLayout::Wide { 58 } else { 40 }))
            ));

            if layout != SpotifyLayout::Mini {
                let album = spotify.album.as_deref().unwrap_or("Unknown album");
                body.push_str(&format!(
                    r##"<text x="{x_text}" y="94" fill="#7f91bd" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="13">{}</text>"##,
                    escape_xml(&truncate_chars(album, if layout == SpotifyLayout::Wide { 58 } else { 40 }))
                ));
                if let Some(duration_ms) = spotify.duration_ms {
                    body.push_str(&format!(
                        r##"<text x="{}" y="{}" text-anchor="end" fill="#7f91bd" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="13">{}</text>"##,
                        width.saturating_sub(18),
                        height.saturating_sub(18),
                        format_duration(duration_ms)
                    ));
                }
            }

            if stale {
                body.push_str(&format!(
                    r##"<text x="{}" y="24" text-anchor="end" fill="#f2c879" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="12">last known</text>"##,
                    width.saturating_sub(18)
                ));
            }
        }
        VisibleSpotify::Inactive => {
            body.push_str(r##"<circle cx="44" cy="48" r="24" fill="#18213a"/>"##);
            body.push_str(r##"<text x="44" y="54" text-anchor="middle" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="18">♪</text>"##);
            body.push_str(r##"<text x="84" y="43" fill="#f4f7ff" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="17" font-weight="700">Spotify</text>"##);
            body.push_str(r##"<text x="84" y="67" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="14">Not listening right now</text>"##);
        }
        VisibleSpotify::Unavailable => {
            body.push_str(r##"<circle cx="44" cy="48" r="24" fill="#18213a"/>"##);
            body.push_str(r##"<text x="44" y="54" text-anchor="middle" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="18">♪</text>"##);
            body.push_str(r##"<text x="84" y="43" fill="#f4f7ff" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="17" font-weight="700">Spotify</text>"##);
            body.push_str(r##"<text x="84" y="67" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="14">Presence temporarily unavailable</text>"##);
        }
    }

    body.push_str("</svg>");
    body
}

fn render_spotify_cover(
    body: &mut String,
    spotify: &SpotifyActivity,
    x: u32,
    y: u32,
    size: u32,
    embedded: &EmbeddedArtworkBatch,
) {
    if let Some(data_uri) = spotify
        .cover_url
        .as_deref()
        .and_then(|url| embedded.href(url))
    {
        body.push_str(&format!(
            r##"<image x="{x}" y="{y}" width="{size}" height="{size}" href="{}" preserveAspectRatio="xMidYMid slice"/>"##,
            escape_xml(data_uri)
        ));
    } else {
        body.push_str(&format!(
            r##"<rect x="{x}" y="{y}" width="{size}" height="{size}" rx="12" fill="#18213a"/>"##
        ));
        body.push_str(&format!(
            r##"<text x="{}" y="{}" text-anchor="middle" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="22">♪</text>"##,
            x.saturating_add(size / 2),
            y.saturating_add(size / 2).saturating_add(7)
        ));
    }
}

fn render_artwork(
    body: &mut String,
    artwork: &ResolvedArtwork,
    x: u32,
    y: u32,
    size: u32,
    embedded: &EmbeddedArtworkBatch,
) {
    if let Some(data_uri) = artwork.url.as_deref().and_then(|url| embedded.href(url)) {
        body.push_str(&format!(
            r##"<image x="{x}" y="{y}" width="{size}" height="{size}" href="{}" preserveAspectRatio="xMidYMid slice"/>"##,
            escape_xml(data_uri)
        ));
        return;
    }

    body.push_str(&format!(
        r##"<rect x="{x}" y="{y}" width="{size}" height="{size}" rx="10" fill="#18213a"/>"##
    ));
    let fallback = artwork
        .fallback_key
        .chars()
        .next()
        .map(|character| character.to_ascii_uppercase().to_string())
        .unwrap_or_else(|| "?".to_owned());
    body.push_str(&format!(
        r##"<text x="{}" y="{}" text-anchor="middle" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="18" font-weight="700">{}</text>"##,
        x.saturating_add(size / 2),
        y.saturating_add(size / 2).saturating_add(6),
        escape_xml(&fallback)
    ));
}

fn push_svg_open(body: &mut String, width: u32, height: u32, title: &str) {
    body.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img">"##
    ));
    body.push_str(&format!("<title>{}</title>", escape_xml(title)));
}

fn activity_detail(activity: &ActivitySnapshot) -> String {
    let details = activity
        .details
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let state = activity
        .state
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match (details, state) {
        (Some(details), Some(state)) if details != state => format!("{details} · {state}"),
        (Some(details), _) => details.to_owned(),
        (_, Some(state)) => state.to_owned(),
        _ => activity_kind_name(activity.kind).to_owned(),
    }
}

fn select_display_activities(activities: &[ActivitySnapshot]) -> Vec<&ActivitySnapshot> {
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

    selected
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

fn presence_artwork_urls(runtime: &RuntimeState) -> Vec<String> {
    let PresenceState::Known(snapshot) = &runtime.presence else {
        return Vec::new();
    };

    if runtime.freshness == PresenceFreshness::Unavailable {
        return Vec::new();
    }

    let mut urls = Vec::new();

    for activity in select_display_activities(&snapshot.data.activities) {
        let Some(url) = resolve_artwork(activity).url else {
            continue;
        };

        if !urls.iter().any(|existing| existing == &url) {
            urls.push(url);
        }
    }

    urls
}

fn spotify_artwork_urls(runtime: &RuntimeState) -> Vec<String> {
    let PresenceState::Known(snapshot) = &runtime.presence else {
        return Vec::new();
    };

    if runtime.freshness == PresenceFreshness::Unavailable {
        return Vec::new();
    }

    spotify_activity(&snapshot.data.activities)
        .and_then(|spotify| spotify.cover_url)
        .into_iter()
        .collect()
}

fn visible_snapshot(runtime: &RuntimeState) -> VisibleSnapshot<'_> {
    match &runtime.presence {
        PresenceState::Unknown => VisibleSnapshot::Unknown,
        PresenceState::Known(snapshot) if runtime.freshness == PresenceFreshness::Unavailable => {
            VisibleSnapshot::Unavailable {
                snapshot_revision: snapshot.revision,
            }
        }
        PresenceState::Known(snapshot) => VisibleSnapshot::Known {
            activities: select_display_activities(&snapshot.data.activities),
            snapshot_revision: snapshot.revision,
            stale: runtime.freshness == PresenceFreshness::Stale,
            status: snapshot.data.status,
        },
    }
}

fn visible_spotify(runtime: &RuntimeState) -> VisibleSpotify {
    match &runtime.presence {
        PresenceState::Unknown => VisibleSpotify::Unavailable,
        PresenceState::Known(_) if runtime.freshness == PresenceFreshness::Unavailable => {
            VisibleSpotify::Unavailable
        }
        PresenceState::Known(snapshot) => match spotify_activity(&snapshot.data.activities) {
            Some(spotify) => VisibleSpotify::Track {
                spotify,
                stale: runtime.freshness == PresenceFreshness::Stale,
            },
            None => VisibleSpotify::Inactive,
        },
    }
}

enum VisibleSnapshot<'a> {
    Known {
        activities: Vec<&'a ActivitySnapshot>,
        snapshot_revision: u64,
        stale: bool,
        status: PresenceStatus,
    },
    Unavailable {
        snapshot_revision: u64,
    },
    Unknown,
}

enum VisibleSpotify {
    Track {
        spotify: SpotifyActivity,
        stale: bool,
    },
    Inactive,
    Unavailable,
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

fn format_duration(duration_ms: u64) -> String {
    let total_seconds = duration_ms / 1_000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes}:{seconds:02}")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        activity::{ArtworkSource, ResolvedArtwork},
        artwork_embed::EmbeddedArtworkBatch,
    };

    use crate::state::{
        ActivityAssetsSnapshot, ActivityKind, ActivitySnapshot, ClientStatusSnapshot,
        GatewayStatus, PresenceData, PresenceFreshness, PresenceSnapshot, PresenceState,
        PresenceStatus, RuntimeState,
    };

    use super::{SpotifyLayout, SvgRenderer, render_artwork};

    fn runtime(freshness: PresenceFreshness, stream_revision: u64) -> RuntimeState {
        RuntimeState {
            freshness,
            gateway_status: GatewayStatus::Live,
            last_target_event_unix_ms: Some(1_234),
            presence: PresenceState::Known(PresenceSnapshot {
                data: PresenceData {
                    activities: vec![ActivitySnapshot {
                        application_id: Some("333".to_owned()),
                        assets: Some(ActivityAssetsSnapshot {
                            large_image: Some("../../bad".to_owned()),
                            large_text: None,
                            small_image: None,
                            small_text: None,
                        }),
                        details: Some("Editing <main>& tests".to_owned()),
                        kind: ActivityKind::Playing,
                        name: "Code <script>alert(1)</script>".to_owned(),
                        state: Some("repo \"profile\"".to_owned()),
                        timestamps: None,
                    }],
                    client_status: ClientStatusSnapshot {
                        desktop: Some(PresenceStatus::Online),
                        mobile: None,
                        web: None,
                    },
                    status: PresenceStatus::Online,
                },
                observed_at_unix_ms: 1_234,
                revision: 7,
            }),
            stream_revision,
            unvalidated_since_unix_ms: None,
        }
    }

    #[tokio::test]
    async fn escapes_external_text_before_inserting_it_into_svg() {
        let renderer = SvgRenderer::new().unwrap();
        let document = renderer
            .render_presence(&runtime(PresenceFreshness::Fresh, 8))
            .await;

        assert!(!document.body().contains("<script>"));
        assert!(document.body().contains("&lt;script&gt;"));
        assert!(document.body().contains("Editing &lt;main&gt;&amp; tests"));
        assert!(document.body().contains("repo &quot;profile&quot;"));
    }

    #[tokio::test]
    async fn unchanged_stream_revision_reuses_the_rendered_document() {
        let renderer = SvgRenderer::new().unwrap();
        let state = runtime(PresenceFreshness::Fresh, 8);
        let first = renderer.render_presence(&state).await;
        let second = renderer.render_presence(&state).await;

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.etag(), second.etag());
    }

    #[tokio::test]
    async fn stale_state_keeps_lkg_but_unavailable_state_is_neutral() {
        let renderer = SvgRenderer::new().unwrap();
        let stale = renderer
            .render_presence(&runtime(PresenceFreshness::Stale, 9))
            .await;
        assert!(stale.body().contains("last known"));
        assert!(stale.body().contains("Code &lt;script&gt;"));

        let unavailable = renderer
            .render_presence(&runtime(PresenceFreshness::Unavailable, 10))
            .await;
        assert!(
            unavailable
                .body()
                .contains("Presence temporarily unavailable")
        );
        assert!(!unavailable.body().contains("Code &lt;script&gt;"));
    }

    #[tokio::test]
    async fn spotify_layouts_have_stable_dimensions() {
        let renderer = SvgRenderer::new().unwrap();
        let state = runtime(PresenceFreshness::Fresh, 8);

        assert!(
            renderer
                .render_spotify(&state, SpotifyLayout::Mini)
                .await
                .body()
                .contains("viewBox=\"0 0 420 96\"")
        );
        assert!(
            renderer
                .render_spotify(&state, SpotifyLayout::Compact)
                .await
                .body()
                .contains("viewBox=\"0 0 560 128\"")
        );
        assert!(
            renderer
                .render_spotify(&state, SpotifyLayout::Wide)
                .await
                .body()
                .contains("viewBox=\"0 0 720 152\"")
        );
    }

    #[test]
    fn remote_artwork_is_never_emitted_directly() {
        let artwork = ResolvedArtwork {
            fallback_key: "rustrover",
            source: ArtworkSource::DiscordMediaProxy,
            url: Some("https://media.discordapp.net/external/hash/icon.png".to_owned()),
        };
        let mut body = String::new();

        render_artwork(
            &mut body,
            &artwork,
            0,
            0,
            42,
            &EmbeddedArtworkBatch::default(),
        );

        assert!(!body.contains("https://media.discordapp.net"));
        assert!(body.contains(">R</text>"));
    }
}
