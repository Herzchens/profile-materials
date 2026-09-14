use std::sync::{Arc, Mutex};

use crate::{
    activity::{ResolvedActivityArtwork, is_spotify_activity, resolve_activity_artwork},
    artwork_embed::{ArtworkEmbedder, EmbeddedArtworkBatch},
    clock,
    discord::identity::DiscordIdentitySnapshot,
    state::{
        ActivityKind, ActivitySnapshot, ClientStatusSnapshot, PresenceFreshness, PresenceState,
        PresenceStatus, RuntimeState,
    },
};

const CARD_WIDTH: u32 = 1200;
const RENDERER_REVISION: u8 = 1;
const TIME_BUCKET_MS: u64 = 15_000;
const MAX_VISIBLE_ACTIVITIES: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PresenceCardCacheKey {
    identity_revision: u64,
    stream_revision: u64,
    time_bucket: u64,
}

#[derive(Debug)]
pub struct PresenceCardDocument {
    body: String,
    etag: String,
    stream_revision: u64,
}

impl PresenceCardDocument {
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

pub struct PresenceCardRenderer {
    artwork: ArtworkEmbedder,
    cache: Mutex<Option<(PresenceCardCacheKey, Arc<PresenceCardDocument>)>>,
}

impl PresenceCardRenderer {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            artwork: ArtworkEmbedder::new()?,
            cache: Mutex::new(None),
        })
    }

    pub async fn render(
        &self,
        runtime: &RuntimeState,
        identity: Option<&DiscordIdentitySnapshot>,
    ) -> Arc<PresenceCardDocument> {
        let now_unix_ms = clock::unix_time_millis().unwrap_or(0);
        let time_bucket = if has_visible_timestamps(runtime) {
            now_unix_ms / TIME_BUCKET_MS
        } else {
            0
        };
        let identity_revision = identity.map_or(0, |identity| identity.revision);
        let key = PresenceCardCacheKey {
            identity_revision,
            stream_revision: runtime.stream_revision,
            time_bucket,
        };

        if let Some(document) = self.cached(key) {
            return document;
        }

        let urls = card_artwork_urls(runtime, identity);
        let embedded = self.artwork.embed_all(&urls).await;
        let body = render_presence_card(runtime, identity, now_unix_ms, &embedded);
        let artwork_state = if embedded.complete() {
            "full"
        } else {
            "partial"
        };
        let document = Arc::new(PresenceCardDocument {
            body,
            etag: format!(
                "\"profile-presence-card-v{RENDERER_REVISION}-{}-{identity_revision}-{time_bucket}-{artwork_state}\"",
                runtime.stream_revision
            ),
            stream_revision: runtime.stream_revision,
        });

        if embedded.complete() {
            let mut cache = match self.cache.lock() {
                Ok(cache) => cache,
                Err(poisoned) => {
                    tracing::warn!(
                        "presence card cache mutex was poisoned; recovering cached entry"
                    );
                    poisoned.into_inner()
                }
            };
            *cache = Some((key, Arc::clone(&document)));
        }

        document
    }

    fn cached(&self, key: PresenceCardCacheKey) -> Option<Arc<PresenceCardDocument>> {
        let cache = match self.cache.lock() {
            Ok(cache) => cache,
            Err(poisoned) => {
                tracing::warn!("presence card cache mutex was poisoned; recovering cached entry");
                poisoned.into_inner()
            }
        };

        cache
            .as_ref()
            .and_then(|(cached_key, document)| (*cached_key == key).then(|| Arc::clone(document)))
    }
}

fn render_presence_card(
    runtime: &RuntimeState,
    identity: Option<&DiscordIdentitySnapshot>,
    now_unix_ms: u64,
    embedded: &EmbeddedArtworkBatch,
) -> String {
    let selected = visible_activities(runtime);
    let shown_count = selected.len().min(MAX_VISIBLE_ACTIVITIES);
    let height = match shown_count {
        0 => 300,
        1 => 520,
        2 => 500,
        _ => 760,
    };
    let mut body = String::with_capacity(48_000);

    push_svg_open(&mut body, CARD_WIDTH, height, "ItzHerzchen Discord status");
    body.push_str(
        r##"<defs>
<linearGradient id="card-bg" x1="0" x2="1" y1="0" y2="1"><stop offset="0" stop-color="#0e1220"/><stop offset="1" stop-color="#12172a"/></linearGradient>
<linearGradient id="panel-bg" x1="0" x2="1"><stop offset="0" stop-color="#11182a" stop-opacity=".98"/><stop offset="1" stop-color="#151a2a" stop-opacity=".92"/></linearGradient>
<linearGradient id="fallback-art" x1="0" x2="1" y1="0" y2="1"><stop offset="0" stop-color="#262044"/><stop offset="1" stop-color="#121a31"/></linearGradient>
<filter id="soft-shadow" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur in="SourceAlpha" stdDeviation="10" result="blur"/><feOffset dy="5" result="offset"/><feColorMatrix in="offset" values="0 0 0 0 0.02 0 0 0 0 0.01 0 0 0 0 0.08 0 0 0 .65 0"/><feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge></filter>
<filter id="art-blur" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="20"/></filter>
</defs>"##,
    );
    body.push_str(&format!(
        r##"<rect width="{CARD_WIDTH}" height="{height}" rx="34" fill="url(#card-bg)"/><rect x="1" y="1" width="1198" height="{}" rx="33" fill="none" stroke="#7c6fc5" stroke-opacity=".5" stroke-width="2"/>"##,
        height.saturating_sub(2)
    ));

    render_identity_header(&mut body, runtime, identity, embedded);
    body.push_str(
        r##"<line x1="40" y1="155" x2="1160" y2="155" stroke="#8d93b5" stroke-opacity=".24"/>"##,
    );

    match visible_presence(runtime) {
        VisiblePresence::Known { stale, .. } => {
            if stale {
                push_text(
                    &mut body,
                    1148,
                    42,
                    13,
                    "#d8b46b",
                    "700",
                    "LAST KNOWN",
                    Some("end"),
                );
            }

            if shown_count == 0 {
                render_empty_state(&mut body, height, "No current activity");
            } else {
                render_activity_grid(
                    &mut body,
                    runtime,
                    &selected[..shown_count],
                    now_unix_ms,
                    embedded,
                );
                if selected.len() > MAX_VISIBLE_ACTIVITIES {
                    push_text(
                        &mut body,
                        1148,
                        height.saturating_sub(18),
                        13,
                        "#8e96b7",
                        "600",
                        &format!("+{} more", selected.len() - MAX_VISIBLE_ACTIVITIES),
                        Some("end"),
                    );
                }
            }
        }
        VisiblePresence::Unavailable => render_empty_state(
            &mut body,
            height,
            "Live presence is temporarily unavailable",
        ),
        VisiblePresence::Unknown => {
            render_empty_state(&mut body, height, "Waiting for Discord presence")
        }
    }

    body.push_str("</svg>");
    body
}

fn render_identity_header(
    body: &mut String,
    runtime: &RuntimeState,
    identity: Option<&DiscordIdentitySnapshot>,
    embedded: &EmbeddedArtworkBatch,
) {
    let avatar_x = 54;
    let avatar_y = 31;
    let avatar_size = 92;
    body.push_str(&format!(
        r##"<defs><clipPath id="avatar-clip"><circle cx="{}" cy="{}" r="46"/></clipPath></defs>"##,
        avatar_x + avatar_size / 2,
        avatar_y + avatar_size / 2
    ));

    if let Some(identity) = identity {
        if let Some(data_uri) = embedded.href(&identity.data.avatar_url) {
            body.push_str(&format!(
                r##"<image x="{avatar_x}" y="{avatar_y}" width="{avatar_size}" height="{avatar_size}" href="{}" preserveAspectRatio="xMidYMid slice" clip-path="url(#avatar-clip)"/>"##,
                escape_xml(data_uri)
            ));
        } else {
            render_avatar_fallback(body, avatar_x, avatar_y, avatar_size);
        }

        if let Some(decoration_url) = identity.data.avatar_decoration_url.as_deref()
            && let Some(data_uri) = embedded.href(decoration_url)
        {
            body.push_str(&format!(
                r##"<image x="42" y="19" width="116" height="116" href="{}" preserveAspectRatio="xMidYMid meet"/>"##,
                escape_xml(data_uri)
            ));
        }

        let display_name = truncate_chars(&identity.data.display_name, 24);
        push_text(body, 174, 70, 31, "#f7f7ff", "800", &display_name, None);
        push_text(
            body,
            174,
            105,
            16,
            "#9ba4c6",
            "500",
            &format!("@{}", truncate_chars(&identity.data.username, 30)),
            None,
        );

        let mut badge_x = 690;
        if let Some(guild) = identity.data.primary_guild.as_ref() {
            let pill_width = 74 + u32::try_from(guild.tag.chars().count()).unwrap_or(4) * 11;
            body.push_str(&format!(
                r##"<rect x="{badge_x}" y="47" width="{pill_width}" height="42" rx="14" fill="#181a2a" stroke="#8875d6" stroke-opacity=".7"/>"##
            ));
            if let Some(badge_url) = guild.badge_url.as_deref()
                && let Some(data_uri) = embedded.href(badge_url)
            {
                body.push_str(&format!(
                    r##"<image x="{}" y="57" width="22" height="22" href="{}" preserveAspectRatio="xMidYMid meet"/>"##,
                    badge_x + 12,
                    escape_xml(data_uri)
                ));
            }
            push_text(
                body,
                badge_x + 42,
                75,
                18,
                "#f0eaff",
                "700",
                &guild.tag,
                None,
            );
            badge_x = badge_x.saturating_add(pill_width + 12);
        }

        for badge in public_badges(identity.data.public_flags)
            .into_iter()
            .take(2)
        {
            let pill_width = 38 + u32::try_from(badge.chars().count()).unwrap_or(3) * 9;
            body.push_str(&format!(
                r##"<rect x="{badge_x}" y="51" width="{pill_width}" height="34" rx="12" fill="#20243a" stroke="#6e769d" stroke-opacity=".5"/>"##
            ));
            push_text(
                body,
                badge_x + pill_width / 2,
                73,
                12,
                "#c9d0ea",
                "700",
                badge,
                Some("middle"),
            );
            badge_x = badge_x.saturating_add(pill_width + 10);
        }
    } else {
        render_avatar_fallback(body, avatar_x, avatar_y, avatar_size);
        push_text(body, 174, 72, 29, "#f7f7ff", "800", "Discord profile", None);
        push_text(
            body,
            174,
            105,
            16,
            "#9ba4c6",
            "500",
            "Loading profile identity",
            None,
        );
    }

    let (status_label, status_color) = status_visual(runtime);
    body.push_str(&format!(
        r##"<circle cx="134" cy="116" r="14" fill="#0e1220"/><circle cx="134" cy="116" r="10" fill="{status_color}"/>"##
    ));
    push_text(
        body,
        1148,
        105,
        16,
        status_color,
        "700",
        status_label,
        Some("end"),
    );

    if let Some(custom_status) = custom_status(runtime) {
        push_text(
            body,
            174,
            132,
            14,
            "#7f89aa",
            "500",
            &truncate_chars(custom_status, 72),
            None,
        );
    }
}

fn render_activity_grid(
    body: &mut String,
    runtime: &RuntimeState,
    activities: &[&ActivitySnapshot],
    now_unix_ms: u64,
    embedded: &EmbeddedArtworkBatch,
) {
    match activities.len() {
        1 => render_activity_panel(
            body,
            runtime,
            activities[0],
            PanelGeometry {
                x: 40,
                y: 180,
                width: 1120,
                height: 300,
            },
            0,
            now_unix_ms,
            embedded,
        ),
        2 => {
            for (index, activity) in activities.iter().enumerate() {
                render_activity_panel(
                    body,
                    runtime,
                    activity,
                    PanelGeometry {
                        x: 40 + u32::try_from(index).unwrap_or(0) * 570,
                        y: 180,
                        width: 550,
                        height: 280,
                    },
                    index,
                    now_unix_ms,
                    embedded,
                );
            }
        }
        _ => {
            for (index, activity) in activities.iter().enumerate() {
                let column = u32::try_from(index % 2).unwrap_or(0);
                let row = u32::try_from(index / 2).unwrap_or(0);
                render_activity_panel(
                    body,
                    runtime,
                    activity,
                    PanelGeometry {
                        x: 40 + column * 570,
                        y: 180 + row * 270,
                        width: 550,
                        height: 250,
                    },
                    index,
                    now_unix_ms,
                    embedded,
                );
            }
        }
    }
}

#[derive(Clone, Copy)]
struct PanelGeometry {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

fn render_activity_panel(
    body: &mut String,
    runtime: &RuntimeState,
    activity: &ActivitySnapshot,
    panel: PanelGeometry,
    index: usize,
    now_unix_ms: u64,
    embedded: &EmbeddedArtworkBatch,
) {
    let artwork = resolve_activity_artwork(activity);
    let art_size = if panel.width > 800 { 220 } else { 148 };
    let art_x = panel.x + 20;
    let art_y = panel.y + (panel.height.saturating_sub(art_size)) / 2;
    let text_x = art_x + art_size + 34;
    let panel_clip = format!("panel-clip-{index}");
    let art_clip = format!("art-clip-{index}");

    body.push_str(&format!(
        r##"<defs><clipPath id="{panel_clip}"><rect x="{}" y="{}" width="{}" height="{}" rx="24"/></clipPath><clipPath id="{art_clip}"><rect x="{art_x}" y="{art_y}" width="{art_size}" height="{art_size}" rx="20"/></clipPath></defs>"##,
        panel.x, panel.y, panel.width, panel.height
    ));
    body.push_str(&format!(
        r##"<rect x="{}" y="{}" width="{}" height="{}" rx="24" fill="url(#panel-bg)" filter="url(#soft-shadow)"/>"##,
        panel.x, panel.y, panel.width, panel.height
    ));

    if let Some(data_uri) = artwork
        .large
        .url
        .as_deref()
        .and_then(|url| embedded.href(url))
    {
        body.push_str(&format!(
            r##"<image x="{}" y="{}" width="{}" height="{}" href="{}" preserveAspectRatio="xMidYMid slice" filter="url(#art-blur)" opacity=".22" clip-path="url(#{panel_clip})"/><rect x="{}" y="{}" width="{}" height="{}" rx="24" fill="#0a0e1a" fill-opacity=".72"/>"##,
            panel.x,
            panel.y,
            panel.width,
            panel.height,
            escape_xml(data_uri),
            panel.x,
            panel.y,
            panel.width,
            panel.height
        ));
    }

    body.push_str(&format!(
        r##"<rect x="{}" y="{}" width="{}" height="{}" rx="24" fill="none" stroke="#7c75a9" stroke-opacity=".35"/>"##,
        panel.x, panel.y, panel.width, panel.height
    ));
    render_large_artwork(
        body, activity, &artwork, art_x, art_y, art_size, &art_clip, embedded,
    );
    render_small_artwork(body, &artwork, art_x, art_y, art_size, index, embedded);

    let compact = panel.width <= 600;
    let kind_color = activity_kind_color(activity.kind);
    push_text(
        body,
        text_x,
        panel.y + 48,
        if compact { 14 } else { 16 },
        kind_color,
        "800",
        &activity_kind_name(activity.kind).to_ascii_uppercase(),
        None,
    );
    push_text(
        body,
        text_x,
        panel.y + 88,
        if compact { 25 } else { 31 },
        "#f8f8ff",
        "800",
        &truncate_chars(activity.name.trim(), if compact { 22 } else { 32 }),
        None,
    );

    let mut line_y = panel.y + 126;
    if let Some(details) = non_empty_trimmed(activity.details.as_deref()) {
        push_text(
            body,
            text_x,
            line_y,
            if compact { 16 } else { 18 },
            "#cbd2ec",
            "500",
            &truncate_chars(details, if compact { 28 } else { 52 }),
            None,
        );
        line_y += 30;
    }
    if let Some(state) = non_empty_trimmed(activity.state.as_deref())
        && Some(state) != non_empty_trimmed(activity.details.as_deref())
    {
        push_text(
            body,
            text_x,
            line_y,
            if compact { 15 } else { 17 },
            "#aeb7d8",
            "500",
            &truncate_chars(state, if compact { 30 } else { 56 }),
            None,
        );
        line_y += 28;
    }
    if is_spotify_activity(activity)
        && let Some(album) = activity
            .assets
            .as_ref()
            .and_then(|assets| non_empty_trimmed(assets.large_text.as_deref()))
    {
        push_text(
            body,
            text_x,
            line_y,
            13,
            "#8994b8",
            "500",
            &truncate_chars(album, if compact { 32 } else { 56 }),
            None,
        );
    }

    let meta_y = panel.y + panel.height - 28;
    let mut meta_x = text_x;
    if let Some(time) = activity_time(activity, now_unix_ms) {
        push_text(
            body,
            meta_x,
            meta_y,
            14,
            "#c7cee8",
            "600",
            &format!("◷ {time}"),
            None,
        );
        meta_x = meta_x.saturating_add(if compact { 160 } else { 190 });
    }
    if let Some(party) = activity.party.as_ref() {
        push_text(
            body,
            meta_x,
            meta_y,
            14,
            "#b7c0df",
            "600",
            &format!("Party {} / {}", party.current_size, party.max_size),
            None,
        );
        meta_x = meta_x.saturating_add(if compact { 100 } else { 130 });
    }
    if let Some(platform) = client_platform(runtime) {
        push_text(
            body,
            meta_x,
            meta_y,
            14,
            "#9ca6c9",
            "600",
            &format!("▣ {platform}"),
            None,
        );
    }

    if is_spotify_activity(activity)
        && let Some(progress) = activity_progress(activity, now_unix_ms)
    {
        let bar_width = if compact { 230 } else { 390 };
        let bar_x = text_x;
        let bar_y = meta_y.saturating_sub(24);
        body.push_str(&format!(
            r##"<rect x="{bar_x}" y="{bar_y}" width="{bar_width}" height="5" rx="3" fill="#555d7a" fill-opacity=".55"/><rect x="{bar_x}" y="{bar_y}" width="{}" height="5" rx="3" fill="{kind_color}"/>"##,
            (f64::from(bar_width) * progress).round() as u32
        ));
    }
}

fn render_large_artwork(
    body: &mut String,
    activity: &ActivitySnapshot,
    artwork: &ResolvedActivityArtwork,
    x: u32,
    y: u32,
    size: u32,
    clip_id: &str,
    embedded: &EmbeddedArtworkBatch,
) {
    if let Some(data_uri) = artwork
        .large
        .url
        .as_deref()
        .and_then(|url| embedded.href(url))
    {
        body.push_str(&format!(
            r##"<image x="{x}" y="{y}" width="{size}" height="{size}" href="{}" preserveAspectRatio="xMidYMid slice" clip-path="url(#{clip_id})"/>"##,
            escape_xml(data_uri)
        ));
        return;
    }

    body.push_str(&format!(
        r##"<rect x="{x}" y="{y}" width="{size}" height="{size}" rx="20" fill="url(#fallback-art)"/>"##
    ));
    push_text(
        body,
        x + size / 2,
        y + size / 2 + 12,
        if size > 180 { 42 } else { 30 },
        "#c9c5f2",
        "800",
        &fallback_label(&activity.name),
        Some("middle"),
    );
}

fn render_small_artwork(
    body: &mut String,
    artwork: &ResolvedActivityArtwork,
    art_x: u32,
    art_y: u32,
    art_size: u32,
    index: usize,
    embedded: &EmbeddedArtworkBatch,
) {
    let Some(small) = artwork.small.as_ref() else {
        return;
    };
    let Some(data_uri) = small.url.as_deref().and_then(|url| embedded.href(url)) else {
        return;
    };

    let size = if art_size > 180 { 58 } else { 46 };
    let x = art_x + art_size - size + 8;
    let y = art_y + art_size - size + 8;
    let clip = format!("small-art-clip-{index}");
    body.push_str(&format!(
        r##"<defs><clipPath id="{clip}"><circle cx="{}" cy="{}" r="{}"/></clipPath></defs><circle cx="{}" cy="{}" r="{}" fill="#0d1120" stroke="#8e7ce8" stroke-width="2"/><image x="{x}" y="{y}" width="{size}" height="{size}" href="{}" preserveAspectRatio="xMidYMid slice" clip-path="url(#{clip})"/>"##,
        x + size / 2,
        y + size / 2,
        size / 2 - 3,
        x + size / 2,
        y + size / 2,
        size / 2,
        escape_xml(data_uri)
    ));
}

fn render_empty_state(body: &mut String, height: u32, message: &str) {
    body.push_str(&format!(
        r##"<rect x="40" y="180" width="1120" height="{}" rx="24" fill="#12182a" fill-opacity=".75" stroke="#6f789b" stroke-opacity=".25"/>"##,
        height.saturating_sub(220)
    ));
    push_text(
        body,
        600,
        235,
        18,
        "#9ba5c7",
        "600",
        message,
        Some("middle"),
    );
}

fn card_artwork_urls(
    runtime: &RuntimeState,
    identity: Option<&DiscordIdentitySnapshot>,
) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(identity) = identity {
        push_unique(&mut urls, identity.data.avatar_url.clone());
        if let Some(url) = identity.data.avatar_decoration_url.clone() {
            push_unique(&mut urls, url);
        }
        if let Some(url) = identity
            .data
            .primary_guild
            .as_ref()
            .and_then(|guild| guild.badge_url.clone())
        {
            push_unique(&mut urls, url);
        }
    }

    for activity in visible_activities(runtime)
        .into_iter()
        .take(MAX_VISIBLE_ACTIVITIES)
    {
        let artwork = resolve_activity_artwork(activity);
        if let Some(url) = artwork.large.url {
            push_unique(&mut urls, url);
        }
        if let Some(url) = artwork.small.and_then(|artwork| artwork.url) {
            push_unique(&mut urls, url);
        }
    }

    urls
}

fn push_unique(urls: &mut Vec<String>, url: String) {
    if !urls.iter().any(|existing| existing == &url) {
        urls.push(url);
    }
}

fn visible_activities(runtime: &RuntimeState) -> Vec<&ActivitySnapshot> {
    let PresenceState::Known(snapshot) = &runtime.presence else {
        return Vec::new();
    };
    if runtime.freshness == PresenceFreshness::Unavailable {
        return Vec::new();
    }

    let mut selected: Vec<&ActivitySnapshot> = Vec::with_capacity(snapshot.data.activities.len());
    for activity in &snapshot.data.activities {
        if activity.kind == ActivityKind::Custom {
            continue;
        }
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

    selected.sort_by(|left, right| {
        activity_priority(left.kind)
            .cmp(&activity_priority(right.kind))
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
    });
    selected
}

fn custom_status(runtime: &RuntimeState) -> Option<&str> {
    let PresenceState::Known(snapshot) = &runtime.presence else {
        return None;
    };
    if runtime.freshness == PresenceFreshness::Unavailable {
        return None;
    }

    snapshot
        .data
        .activities
        .iter()
        .find(|activity| activity.kind == ActivityKind::Custom)
        .and_then(|activity| {
            non_empty_trimmed(activity.state.as_deref())
                .or_else(|| non_empty_trimmed(activity.details.as_deref()))
        })
}

fn same_display_activity(left: &ActivitySnapshot, right: &ActivitySnapshot) -> bool {
    left.name.trim().eq_ignore_ascii_case(right.name.trim())
}

fn activity_richness(activity: &ActivitySnapshot) -> (u8, u8, u8, u8) {
    let descriptive_fields = u8::from(non_empty_trimmed(activity.details.as_deref()).is_some())
        + u8::from(non_empty_trimmed(activity.state.as_deref()).is_some());
    let asset_fields = activity.assets.as_ref().map_or(0, |assets| {
        u8::from(non_empty_trimmed(assets.large_image.as_deref()).is_some())
            + u8::from(non_empty_trimmed(assets.large_text.as_deref()).is_some())
            + u8::from(non_empty_trimmed(assets.small_image.as_deref()).is_some())
            + u8::from(non_empty_trimmed(assets.small_text.as_deref()).is_some())
    });
    let timestamp_fields = activity.timestamps.as_ref().map_or(0, |timestamps| {
        u8::from(timestamps.start.is_some()) + u8::from(timestamps.end.is_some())
    });
    let application_id = u8::from(
        activity
            .application_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
    );
    (
        descriptive_fields,
        asset_fields,
        timestamp_fields,
        application_id,
    )
}

fn activity_priority(kind: ActivityKind) -> u8 {
    match kind {
        ActivityKind::Competing => 0,
        ActivityKind::Playing => 1,
        ActivityKind::Streaming => 2,
        ActivityKind::Listening => 3,
        ActivityKind::Watching => 4,
        ActivityKind::Unknown(_) => 5,
        ActivityKind::Custom => 6,
    }
}

fn has_visible_timestamps(runtime: &RuntimeState) -> bool {
    visible_activities(runtime)
        .iter()
        .any(|activity| activity.timestamps.is_some())
}

fn activity_time(activity: &ActivitySnapshot, now_unix_ms: u64) -> Option<String> {
    let timestamps = activity.timestamps.as_ref()?;
    match (timestamps.start, timestamps.end) {
        (Some(start), Some(end)) if end > start => {
            let elapsed = now_unix_ms.saturating_sub(start).min(end - start);
            Some(format!(
                "{} / {}",
                format_duration(elapsed),
                format_duration(end - start)
            ))
        }
        (Some(start), _) => Some(format!(
            "{} elapsed",
            format_duration(now_unix_ms.saturating_sub(start))
        )),
        (None, Some(end)) if end > now_unix_ms => {
            Some(format!("{} remaining", format_duration(end - now_unix_ms)))
        }
        _ => None,
    }
}

fn activity_progress(activity: &ActivitySnapshot, now_unix_ms: u64) -> Option<f64> {
    let timestamps = activity.timestamps.as_ref()?;
    let start = timestamps.start?;
    let end = timestamps.end?;
    if end <= start {
        return None;
    }
    let elapsed = now_unix_ms.saturating_sub(start).min(end - start);
    Some(elapsed as f64 / (end - start) as f64)
}

fn client_platform(runtime: &RuntimeState) -> Option<String> {
    let PresenceState::Known(snapshot) = &runtime.presence else {
        return None;
    };
    if runtime.freshness == PresenceFreshness::Unavailable {
        return None;
    }
    platform_label(&snapshot.data.client_status)
}

fn platform_label(status: &ClientStatusSnapshot) -> Option<String> {
    let mut platforms = Vec::new();
    if status.desktop.is_some() {
        platforms.push("Desktop");
    }
    if status.mobile.is_some() {
        platforms.push("Mobile");
    }
    if status.web.is_some() {
        platforms.push("Web");
    }
    (!platforms.is_empty()).then(|| platforms.join(" · "))
}

fn visible_presence(runtime: &RuntimeState) -> VisiblePresence {
    match runtime.presence {
        PresenceState::Unknown => VisiblePresence::Unknown,
        PresenceState::Known(_) if runtime.freshness == PresenceFreshness::Unavailable => {
            VisiblePresence::Unavailable
        }
        PresenceState::Known(_) => VisiblePresence::Known {
            stale: runtime.freshness == PresenceFreshness::Stale,
        },
    }
}

enum VisiblePresence {
    Known { stale: bool },
    Unavailable,
    Unknown,
}

fn status_visual(runtime: &RuntimeState) -> (&'static str, &'static str) {
    match &runtime.presence {
        PresenceState::Known(snapshot) if runtime.freshness != PresenceFreshness::Unavailable => {
            match snapshot.data.status {
                PresenceStatus::Online => ("Online", "#3ecf8e"),
                PresenceStatus::Idle => ("Idle", "#f0b85c"),
                PresenceStatus::DoNotDisturb => ("Do Not Disturb", "#f06565"),
                PresenceStatus::Invisible | PresenceStatus::Offline => ("Offline", "#7f879f"),
            }
        }
        PresenceState::Known(_) => ("Unavailable", "#7f879f"),
        PresenceState::Unknown => ("Connecting", "#8e96b7"),
    }
}

fn public_badges(flags: u64) -> Vec<&'static str> {
    let known = [
        (1_u64 << 0, "STAFF"),
        (1_u64 << 1, "PARTNER"),
        (1_u64 << 14, "BUG II"),
        (1_u64 << 18, "MOD"),
        (1_u64 << 17, "DEV"),
        (1_u64 << 9, "EARLY"),
        (1_u64 << 3, "BUG"),
        (1_u64 << 2, "HS"),
        (1_u64 << 6, "BRAVERY"),
        (1_u64 << 7, "BRILLIANCE"),
        (1_u64 << 8, "BALANCE"),
    ];

    known
        .into_iter()
        .filter_map(|(flag, label)| (flags & flag != 0).then_some(label))
        .collect()
}

fn activity_kind_name(kind: ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Competing => "competing",
        ActivityKind::Custom => "custom",
        ActivityKind::Listening => "listening",
        ActivityKind::Playing => "playing",
        ActivityKind::Streaming => "streaming",
        ActivityKind::Unknown(_) => "activity",
        ActivityKind::Watching => "watching",
    }
}

fn activity_kind_color(kind: ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Competing => "#ff7a72",
        ActivityKind::Listening => "#46d995",
        ActivityKind::Streaming => "#d58cff",
        ActivityKind::Watching => "#63b6ff",
        ActivityKind::Playing | ActivityKind::Unknown(_) | ActivityKind::Custom => "#8f87ff",
    }
}

fn fallback_label(name: &str) -> String {
    let words: Vec<&str> = name
        .split_whitespace()
        .filter(|word| !word.is_empty())
        .collect();
    if words.len() >= 2 {
        return words
            .iter()
            .take(2)
            .filter_map(|word| word.chars().next())
            .flat_map(char::to_uppercase)
            .collect();
    }

    name.chars()
        .filter(|character| character.is_alphanumeric())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect::<String>()
        .chars()
        .take(2)
        .collect()
}

fn non_empty_trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn format_duration(duration_ms: u64) -> String {
    let total_seconds = duration_ms / 1_000;
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn push_svg_open(body: &mut String, width: u32, height: u32, title: &str) {
    body.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img">"##
    ));
    body.push_str(&format!("<title>{}</title>", escape_xml(title)));
}

fn push_text(
    body: &mut String,
    x: u32,
    y: u32,
    size: u32,
    fill: &str,
    weight: &str,
    text: &str,
    anchor: Option<&str>,
) {
    body.push_str(&format!(
        r##"<text x="{x}" y="{y}"{} fill="{}" font-family="Inter, ui-sans-serif, system-ui, -apple-system, 'Segoe UI', sans-serif" font-size="{size}" font-weight="{}">{}</text>"##,
        anchor.map_or_else(String::new, |anchor| format!(" text-anchor=\"{}\"", escape_xml(anchor))),
        escape_xml(fill),
        escape_xml(weight),
        escape_xml(text)
    ));
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

fn render_avatar_fallback(body: &mut String, x: u32, y: u32, size: u32) {
    body.push_str(&format!(
        r##"<circle cx="{}" cy="{}" r="{}" fill="#252a43"/>"##,
        x + size / 2,
        y + size / 2,
        size / 2
    ));
    push_text(
        body,
        x + size / 2,
        y + size / 2 + 9,
        26,
        "#c6ccee",
        "800",
        "D",
        Some("middle"),
    );
}

#[cfg(test)]
mod tests {
    use crate::state::{
        ActivityAssetsSnapshot, ActivityKind, ActivitySnapshot, ClientStatusSnapshot,
        GatewayStatus, PresenceData, PresenceFreshness, PresenceSnapshot, PresenceState,
        PresenceStatus, RuntimeState,
    };

    use super::{fallback_label, visible_activities};

    fn activity(name: &str, kind: ActivityKind, details: Option<&str>) -> ActivitySnapshot {
        ActivitySnapshot {
            application_id: Some("123".to_owned()),
            assets: Some(ActivityAssetsSnapshot {
                large_image: None,
                large_text: None,
                small_image: None,
                small_text: None,
            }),
            details: details.map(ToOwned::to_owned),
            kind,
            name: name.to_owned(),
            party: None,
            state: None,
            timestamps: None,
        }
    }

    #[test]
    fn fallback_labels_are_compact() {
        assert_eq!(fallback_label("RustRover"), "RU");
        assert_eq!(fallback_label("Genshin Impact"), "GI");
    }

    #[test]
    fn activity_selection_excludes_custom_and_keeps_richest_duplicate() {
        let runtime = RuntimeState {
            freshness: PresenceFreshness::Fresh,
            gateway_status: GatewayStatus::Live,
            last_target_event_unix_ms: Some(10),
            presence: PresenceState::Known(PresenceSnapshot {
                data: PresenceData {
                    activities: vec![
                        activity("VALORANT", ActivityKind::Playing, None),
                        activity("valorant", ActivityKind::Playing, Some("Competitive")),
                        activity("Custom Status", ActivityKind::Custom, Some("hello")),
                        activity("Spotify", ActivityKind::Listening, Some("track")),
                    ],
                    client_status: ClientStatusSnapshot {
                        desktop: Some(PresenceStatus::Online),
                        mobile: None,
                        web: None,
                    },
                    status: PresenceStatus::Online,
                },
                observed_at_unix_ms: 10,
                revision: 2,
            }),
            stream_revision: 2,
            unvalidated_since_unix_ms: None,
        };

        let activities = visible_activities(&runtime);
        assert_eq!(activities.len(), 2);
        assert_eq!(activities[0].name, "valorant");
        assert_eq!(activities[1].name, "Spotify");
    }
}
