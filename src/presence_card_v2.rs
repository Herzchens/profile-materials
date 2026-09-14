use std::sync::{Arc, Mutex};

use crate::{
    activity::{
        ResolvedActivityArtwork, activity_preference_key, is_spotify_activity,
        resolve_activity_artwork,
    },
    artwork_embed::{ArtworkEmbedder, EmbeddedArtworkBatch},
    clock,
    discord::identity::DiscordIdentitySnapshot,
    state::{
        ActivityKind, ActivitySnapshot, PresenceFreshness, PresenceState, PresenceStatus,
        RuntimeState,
    },
};

const CARD_WIDTH: u32 = 1774;
const RENDERER_REVISION: u8 = 13;
const TIME_BUCKET_MS: u64 = 15_000;
const MAX_VISIBLE_ACTIVITIES: usize = 4;
const META_ROW_GAP: u32 = 31;
const SPOTIFY_EXPIRY_GRACE_MS: u64 = 15_000;

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
                Err(poisoned) => poisoned.into_inner(),
            };
            *cache = Some((key, Arc::clone(&document)));
        }

        document
    }

    fn cached(&self, key: PresenceCardCacheKey) -> Option<Arc<PresenceCardDocument>> {
        let cache = match self.cache.lock() {
            Ok(cache) => cache,
            Err(poisoned) => poisoned.into_inner(),
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
    let mut selected = visible_activities(runtime);
    selected.retain(|activity| !spotify_activity_expired(activity, now_unix_ms));
    let shown = selected.len().min(MAX_VISIBLE_ACTIVITIES);
    let height: u32 = match shown {
        0 => 420,
        1 | 2 => 590,
        _ => 887,
    };
    let mut body = String::with_capacity(72_000);

    body.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{CARD_WIDTH}" height="{height}" viewBox="0 0 {CARD_WIDTH} {height}" role="img" aria-label="ItzHerzchen Discord activity"><defs>
<linearGradient id="canvas-bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#0f1320"/><stop offset=".52" stop-color="#15182a"/><stop offset="1" stop-color="#101522"/></linearGradient>
<linearGradient id="root-bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#0b101d"/><stop offset=".54" stop-color="#101528"/><stop offset="1" stop-color="#090e19"/></linearGradient>
<linearGradient id="fallback-art" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#262044"/><stop offset="1" stop-color="#111729"/></linearGradient>
<filter id="shadow" x="-20%" y="-30%" width="140%" height="160%"><feDropShadow dx="0" dy="7" stdDeviation="14" flood-color="#03050b" flood-opacity=".38"/></filter>
<filter id="art-blur" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="26"/></filter>
<filter id="art-blur-single" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="18"/></filter>
<clipPath id="avatar-clip"><circle cx="153" cy="134" r="63"/></clipPath>
</defs>"##
    ));

    body.push_str(&format!(
        r##"<g id="layout-shell"><rect x="0" y="0" width="{CARD_WIDTH}" height="{height}" fill="url(#canvas-bg)"/><rect x="42" y="42" width="1690" height="{}" rx="24" fill="url(#root-bg)" stroke="#6556bc" stroke-opacity=".64" stroke-width="1.5" filter="url(#shadow)"/><line x1="62" y1="208" x2="1712" y2="208" stroke="#8c91b1" stroke-opacity=".24"/></g>"##,
        height.saturating_sub(84)
    ));

    render_identity_header(&mut body, runtime, identity, embedded);

    body.push_str(r##"<g id="activity-grid">"##);
    match visible_presence(runtime) {
        VisiblePresence::Known => {
            if shown == 0 {
                empty_state(&mut body, height, "No current activity");
            } else {
                render_activity_grid(
                    &mut body,
                    runtime,
                    &selected[..shown],
                    now_unix_ms,
                    embedded,
                );
                if selected.len() > MAX_VISIBLE_ACTIVITIES {
                    text(
                        &mut body,
                        1690,
                        height.saturating_sub(55),
                        18,
                        "#8d94b8",
                        "600",
                        &format!("+{} more", selected.len() - MAX_VISIBLE_ACTIVITIES),
                        Some("end"),
                    );
                }
            }
        }
        VisiblePresence::Unavailable => empty_state(
            &mut body,
            height,
            "Live presence is temporarily unavailable",
        ),
        VisiblePresence::Unknown => empty_state(&mut body, height, "Waiting for Discord presence"),
    }
    body.push_str("</g></svg>");
    body
}

fn render_identity_header(
    body: &mut String,
    runtime: &RuntimeState,
    identity: Option<&DiscordIdentitySnapshot>,
    embedded: &EmbeddedArtworkBatch,
) {
    let avatar_x = 90;
    let avatar_y = 71;
    let avatar_size = 126;

    body.push_str(r##"<g id="identity-static">"##);
    if let Some(identity) = identity {
        if let Some(data_uri) = embedded.href(&identity.data.avatar_url) {
            body.push_str(&format!(
                r##"<image x="{avatar_x}" y="{avatar_y}" width="{avatar_size}" height="{avatar_size}" href="{}" preserveAspectRatio="xMidYMid slice" clip-path="url(#avatar-clip)"/>"##,
                escape_xml(data_uri)
            ));
        } else {
            avatar_fallback(body, avatar_x, avatar_y, avatar_size);
        }

        if let Some(url) = identity.data.avatar_decoration_url.as_deref()
            && let Some(data_uri) = embedded.href(url)
        {
            body.push_str(&format!(
                r##"<image x="73" y="54" width="160" height="160" href="{}" preserveAspectRatio="xMidYMid meet"/>"##,
                escape_xml(data_uri)
            ));
        }

        text(
            body,
            271,
            144,
            45,
            "#f7f7ff",
            "800",
            &truncate_chars(&identity.data.username, 28),
            None,
        );

        let mut badge_x = 624;
        if let Some(guild) = identity.data.primary_guild.as_ref() {
            let badge_href = guild
                .badge_url
                .as_deref()
                .and_then(|url| embedded.href(url));
            const BADGE_SIDE_PADDING: u32 = 16;
            const BADGE_ICON_WIDTH: u32 = 30;
            const BADGE_ICON_GAP: u32 = 10;
            const TAG_CHAR_WIDTH: u32 = 15;

            let tag_width = u32::try_from(guild.tag.chars().count()).unwrap_or(4) * TAG_CHAR_WIDTH;
            let text_offset = if badge_href.is_some() {
                BADGE_SIDE_PADDING + BADGE_ICON_WIDTH + BADGE_ICON_GAP
            } else {
                BADGE_SIDE_PADDING
            };
            let width = text_offset + tag_width + BADGE_SIDE_PADDING;
            body.push_str(&format!(
                r##"<rect x="{badge_x}" y="103" width="{width}" height="53" rx="15" fill="#1b1b31" stroke="#6657b8" stroke-opacity=".62"/>"##
            ));
            if let Some(data_uri) = badge_href {
                body.push_str(&format!(
                    r##"<image x="{}" y="115" width="30" height="30" href="{}" preserveAspectRatio="xMidYMid meet"/>"##,
                    badge_x + BADGE_SIDE_PADDING,
                    escape_xml(data_uri)
                ));
            }
            text(
                body,
                badge_x + text_offset,
                139,
                24,
                "#efeaff",
                "700",
                &guild.tag,
                None,
            );
            badge_x += width + 20;
        }

        for badge in public_badges(identity.data.public_flags)
            .into_iter()
            .take(2)
        {
            body.push_str(&format!(
                r##"<circle cx="{}" cy="129" r="25" fill="#fb696f" fill-opacity=".95"/><circle cx="{}" cy="129" r="11" fill="#11162a"/>"##,
                badge_x + 25,
                badge_x + 25
            ));
            text(
                body,
                badge_x + 25,
                181,
                11,
                "#7f88a7",
                "600",
                badge,
                Some("middle"),
            );
            badge_x += 70;
        }
    } else {
        avatar_fallback(body, avatar_x, avatar_y, avatar_size);
        text(
            body,
            271,
            144,
            45,
            "#f7f7ff",
            "800",
            "Discord profile",
            None,
        );
    }
    body.push_str("</g>");

    body.push_str(r##"<g id="presence-header-dynamic">"##);
    let status_color = status_color(runtime);
    body.push_str(&format!(
        r##"<circle cx="202" cy="176" r="22" fill="#101624"/><circle id="presence-status-dot" cx="202" cy="176" r="15" fill="{status_color}"/>"##
    ));

    if let Some(custom) = custom_status(runtime) {
        text(
            body,
            271,
            179,
            18,
            "#8d95b3",
            "500",
            &truncate_chars(custom, 78),
            None,
        );
    }
    body.push_str("</g>");
}

#[derive(Clone, Copy)]
struct Panel {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
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
            Panel {
                x: 62,
                y: 228,
                width: 1650,
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
                    Panel {
                        x: 62 + u32::try_from(index).unwrap_or(0) * 835,
                        y: 228,
                        width: 815,
                        height: 300,
                    },
                    index,
                    now_unix_ms,
                    embedded,
                );
            }
        }
        _ => {
            for (index, activity) in activities.iter().enumerate() {
                let col = u32::try_from(index % 2).unwrap_or(0);
                let row = u32::try_from(index / 2).unwrap_or(0);
                render_activity_panel(
                    body,
                    runtime,
                    activity,
                    Panel {
                        x: 62 + col * 835,
                        y: 228 + row * 300,
                        width: 815,
                        height: 280,
                    },
                    index,
                    now_unix_ms,
                    embedded,
                );
            }
        }
    }
}

fn render_activity_panel(
    body: &mut String,
    runtime: &RuntimeState,
    activity: &ActivitySnapshot,
    panel: Panel,
    index: usize,
    now_unix_ms: u64,
    embedded: &EmbeddedArtworkBatch,
) {
    let artwork = resolve_activity_artwork(activity);
    let spotify = is_spotify_activity(activity);
    let listening = activity.kind == ActivityKind::Listening;
    let activity_focus = !listening;
    let wide_focus = panel.width > 1000 && activity_focus;
    let art_size = if listening {
        if panel.width > 1000 { 188 } else { 150 }
    } else if panel.width > 1000 {
        228
    } else {
        230
    };
    let art_x = panel.x + 26;
    let art_y = if listening {
        panel.y + 24
    } else {
        panel.y + (panel.height - art_size) / 2
    };
    let text_x = art_x + art_size + if listening { 24 } else { 31 };
    let panel_clip = format!("panel-clip-{index}");
    let art_clip = format!("art-clip-{index}");
    let panel_tint = format!("panel-tint-{index}");
    let accent = activity_accent(activity);
    let (tint_mid_opacity, tint_end_opacity, backdrop_filter, backdrop_opacity) = if activity_focus
    {
        (".64", ".72", "art-blur-single", ".74")
    } else {
        (".70", ".82", "art-blur", ".66")
    };

    body.push_str(&format!(
        r##"<g id="activity-panel-{index}"><defs><clipPath id="{panel_clip}"><rect x="{}" y="{}" width="{}" height="{}" rx="21"/></clipPath><clipPath id="{art_clip}"><rect x="{art_x}" y="{art_y}" width="{art_size}" height="{art_size}" rx="17"/></clipPath><linearGradient id="{panel_tint}" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="{accent}" stop-opacity=".13"/><stop offset=".43" stop-color="#0b101a" stop-opacity="{tint_mid_opacity}"/><stop offset="1" stop-color="#090e18" stop-opacity="{tint_end_opacity}"/></linearGradient></defs>"##,
        panel.x, panel.y, panel.width, panel.height
    ));

    if let Some(data_uri) = artwork
        .large
        .url
        .as_deref()
        .and_then(|url| embedded.href(url))
    {
        body.push_str(&format!(
            r##"<image x="{}" y="{}" width="{}" height="{}" href="{}" preserveAspectRatio="xMidYMid slice" filter="url(#{backdrop_filter})" opacity="{backdrop_opacity}" clip-path="url(#{panel_clip})"/>"##,
            panel.x,
            panel.y,
            panel.width,
            panel.height,
            escape_xml(data_uri)
        ));
    }
    body.push_str(&format!(
        r##"<rect x="{}" y="{}" width="{}" height="{}" rx="21" fill="url(#{panel_tint})"/><rect x="{}" y="{}" width="{}" height="{}" rx="21" fill="none" stroke="{accent}" stroke-opacity=".38" stroke-width="1.4"/>"##,
        panel.x, panel.y, panel.width, panel.height,
        panel.x, panel.y, panel.width, panel.height
    ));

    render_large_artwork(
        body, activity, &artwork, art_x, art_y, art_size, &art_clip, embedded, accent,
    );
    if !spotify {
        render_small_artwork(
            body, &artwork, art_x, art_y, art_size, index, embedded, accent,
        );
    }
    render_app_title(body, activity, text_x, panel.y + 31, panel);

    if spotify {
        render_spotify_content(body, activity, panel, text_x, index, now_unix_ms, accent);
        body.push_str("</g>");
        return;
    }

    let mut content_y = if wide_focus {
        panel.y + 150
    } else if activity_focus {
        panel.y + 126
    } else {
        panel.y + 111
    };
    let content_width = panel
        .x
        .saturating_add(panel.width)
        .saturating_sub(26)
        .saturating_sub(text_x)
        .max(1);
    if let Some(details) = non_empty(activity.details.as_deref()) {
        if listening {
            fitted_text(
                body,
                text_x,
                content_y,
                21,
                "#d4d9ed",
                "500",
                details,
                content_width,
                None,
            );
        } else {
            text(
                body,
                text_x,
                content_y,
                21,
                "#d4d9ed",
                "500",
                &truncate_chars(details, if panel.width > 1000 { 60 } else { 34 }),
                None,
            );
        }
        content_y += 31;
    }
    if let Some(state) = non_empty(activity.state.as_deref())
        && Some(state) != non_empty(activity.details.as_deref())
    {
        if listening {
            fitted_text(
                body,
                text_x,
                content_y,
                20,
                "#b9c1dd",
                "500",
                state,
                content_width,
                None,
            );
        } else {
            text(
                body,
                text_x,
                content_y,
                20,
                "#b9c1dd",
                "500",
                &truncate_chars(state, if panel.width > 1000 { 64 } else { 36 }),
                None,
            );
        }
    }

    if listening && media_progress(activity, now_unix_ms).is_some() {
        render_media_progress(body, activity, panel, index, now_unix_ms, accent);
    } else {
        render_standard_metadata(body, runtime, activity, panel, text_x, index, now_unix_ms);
    }
    body.push_str("</g>");
}

fn render_spotify_content(
    body: &mut String,
    activity: &ActivitySnapshot,
    panel: Panel,
    text_x: u32,
    index: usize,
    now_unix_ms: u64,
    accent: &str,
) {
    let right_edge = panel.x + panel.width - 26;
    let text_width = right_edge.saturating_sub(text_x).max(1);

    if let Some(details) = non_empty(activity.details.as_deref()) {
        fitted_text(
            body,
            text_x,
            panel.y + 96,
            if panel.width > 1000 { 22 } else { 20 },
            "#dce2f6",
            "700",
            details,
            text_width,
            None,
        );
    }

    if let Some(state) = non_empty(activity.state.as_deref())
        && Some(state) != non_empty(activity.details.as_deref())
    {
        fitted_text(
            body,
            text_x,
            panel.y + 128,
            if panel.width > 1000 { 20 } else { 18 },
            "#b9c1dd",
            "500",
            state,
            text_width,
            None,
        );
    }

    if let Some(album) = activity
        .assets
        .as_ref()
        .and_then(|assets| non_empty(assets.large_text.as_deref()))
    {
        fitted_text(
            body,
            text_x,
            panel.y + 158,
            if panel.width > 1000 { 18 } else { 16 },
            "#9fa8c8",
            "500",
            &format!("Album · {album}"),
            text_width,
            None,
        );
    }

    render_media_progress(body, activity, panel, index, now_unix_ms, accent);
}

fn render_app_title(body: &mut String, activity: &ActivitySnapshot, x: u32, y: u32, panel: Panel) {
    let (verb, verb_color, name_color) = activity_title_style(activity);
    let max_chars = if panel.width > 1000 { 38 } else { 28 };
    let name = truncate_chars(activity.name.trim(), max_chars);

    if activity.kind != ActivityKind::Listening {
        let compact = panel.width <= 1000;
        text(
            body,
            x,
            y + if compact { 15 } else { 17 },
            if compact { 16 } else { 18 },
            verb_color,
            "700",
            verb,
            None,
        );
        let max_width = panel
            .x
            .saturating_add(panel.width)
            .saturating_sub(26)
            .saturating_sub(x)
            .max(1);
        fitted_text(
            body,
            x,
            y + if compact { 55 } else { 66 },
            if compact { 34 } else { 42 },
            name_color,
            "800",
            &name,
            max_width,
            None,
        );
        return;
    }

    let font_size = if panel.width > 1000 { 31 } else { 28 };
    body.push_str(&format!(
        r##"<text x="{x}" y="{}" font-family="Inter,Segoe UI,Arial,sans-serif" font-size="{font_size}" font-weight="800"><tspan fill="{verb_color}">{}</tspan><tspan fill="{name_color}"> {}</tspan></text>"##,
        y + 29,
        escape_xml(verb),
        escape_xml(&name),
    ));
}

fn activity_title_style(activity: &ActivitySnapshot) -> (&'static str, &'static str, &'static str) {
    match activity.kind {
        ActivityKind::Listening => (
            "Listening to",
            "#b9c1dd",
            listening_service_color(&activity.name),
        ),
        ActivityKind::Playing | ActivityKind::Competing => {
            ("Playing", "#f8f8ff", activity_accent(activity))
        }
        ActivityKind::Streaming => ("Streaming", "#f8f8ff", activity_accent(activity)),
        ActivityKind::Watching => ("Watching", "#f8f8ff", activity_accent(activity)),
        ActivityKind::Custom | ActivityKind::Unknown(_) => {
            ("Using", "#f8f8ff", activity_accent(activity))
        }
    }
}

fn listening_service_color(name: &str) -> &'static str {
    let name = name.trim();
    if name.eq_ignore_ascii_case("spotify") {
        "#31d978"
    } else if name.eq_ignore_ascii_case("soundcloud") {
        "#ff6b35"
    } else if name.eq_ignore_ascii_case("youtube music") || name.eq_ignore_ascii_case("yt music") {
        "#ff3b30"
    } else {
        "#f8f8ff"
    }
}

fn render_standard_metadata(
    body: &mut String,
    _runtime: &RuntimeState,
    activity: &ActivitySnapshot,
    panel: Panel,
    text_x: u32,
    index: usize,
    now_unix_ms: u64,
) {
    let time = activity_time(activity, now_unix_ms);
    let party = activity.party.as_ref();
    let row_count = usize::from(time.is_some()) + usize::from(party.is_some());
    if row_count == 0 {
        return;
    }

    let prominent = activity.kind != ActivityKind::Listening;
    let row_gap = if prominent { 38 } else { META_ROW_GAP };
    let last_baseline = panel.y + panel.height - 30;
    let first_baseline = last_baseline
        .saturating_sub(u32::try_from(row_count.saturating_sub(1)).unwrap_or(0) * row_gap);

    let mut baseline = first_baseline;
    if let Some((value, kind, anchor_ms)) = time {
        let (icon, color) = if kind == "elapsed" && activity.kind == ActivityKind::Listening {
            (MetaIcon::Music, activity_accent(activity))
        } else if kind == "elapsed" {
            (MetaIcon::Gamepad, "#43c987")
        } else {
            (MetaIcon::Clock, "#f3bf4f")
        };
        render_meta_icon(body, icon, text_x, baseline, color, prominent);
        dynamic_time_text(
            body,
            text_x + if prominent { 38 } else { 30 },
            baseline,
            index,
            kind,
            anchor_ms,
            &value,
            color,
            if prominent { 25 } else { 17 },
            if prominent { "700" } else { "600" },
        );
        baseline += row_gap;
    }

    if let Some(party) = party {
        render_meta_icon(
            body,
            MetaIcon::Party,
            text_x,
            baseline,
            "#c0c8e2",
            prominent,
        );
        text(
            body,
            text_x + if prominent { 38 } else { 32 },
            baseline,
            if prominent { 20 } else { 18 },
            "#c0c8e2",
            "500",
            &format!("Party {} / {}", party.current_size, party.max_size),
            None,
        );
    }
}

fn render_media_progress(
    body: &mut String,
    activity: &ActivitySnapshot,
    panel: Panel,
    index: usize,
    now_unix_ms: u64,
    accent: &str,
) {
    let Some((start, end, elapsed, duration, progress)) = media_progress(activity, now_unix_ms)
    else {
        return;
    };
    let bar_x = panel.x + 26;
    let bar_width = panel.width.saturating_sub(52);
    let bar_y = panel.y + panel.height - 62;
    let label_y = bar_y + 35;
    let time_id = format!("activity-time-{index}");
    let progress_id = format!("activity-progress-{index}");

    body.push_str(&format!(
        r##"<rect x="{bar_x}" y="{bar_y}" width="{bar_width}" height="9" rx="5" fill="#626881" fill-opacity=".55"/><rect id="{progress_id}" data-max-width="{bar_width}" x="{bar_x}" y="{bar_y}" width="{}" height="9" rx="5" fill="{accent}"/>"##,
        (f64::from(bar_width) * progress).round() as u32
    ));
    body.push_str(&format!(
        r##"<text id="{time_id}" data-time-kind="media-current" data-start-ms="{start}" data-end-ms="{end}" data-progress-id="{progress_id}" x="{bar_x}" y="{label_y}" fill="#c8cee7" font-family="Inter,Segoe UI,Arial,sans-serif" font-size="17" font-weight="600">{}</text>"##,
        escape_xml(&format_clock(elapsed))
    ));
    text(
        body,
        panel.x + panel.width - 26,
        label_y,
        17,
        "#c8cee7",
        "600",
        &format_clock(duration),
        Some("end"),
    );
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
    accent: &str,
) {
    // Discord application artwork can contain a baked-in edge just inside the bitmap.
    // Crop a few source pixels while clipping every visual layer to the same rounded
    // geometry so that edge/antialias colors cannot leak at the thumbnail corners.
    let crop = (size / 40).clamp(3, 6);
    let image_x = x.saturating_sub(crop);
    let image_y = y.saturating_sub(crop);
    let image_size = size.saturating_add(crop.saturating_mul(2));
    body.push_str(&format!(
        r##"<rect x="{x}" y="{y}" width="{size}" height="{size}" rx="17" fill="{accent}" fill-opacity=".18" clip-path="url(#{clip_id})"/>"##
    ));
    if let Some(data_uri) = artwork
        .large
        .url
        .as_deref()
        .and_then(|url| embedded.href(url))
    {
        body.push_str(&format!(
            r##"<image x="{image_x}" y="{image_y}" width="{image_size}" height="{image_size}" href="{}" preserveAspectRatio="xMidYMid slice" clip-path="url(#{clip_id})"/>"##,
            escape_xml(data_uri)
        ));
    } else {
        body.push_str(&format!(
            r##"<rect x="{x}" y="{y}" width="{size}" height="{size}" rx="17" fill="url(#fallback-art)" clip-path="url(#{clip_id})"/>"##
        ));
        text(
            body,
            x + size / 2,
            y + size / 2 + 12,
            36,
            "#cbc7f3",
            "800",
            &fallback_label(&activity.name),
            Some("middle"),
        );
    }

    let border_x = x.saturating_add(1);
    let border_y = y.saturating_add(1);
    let border_size = size.saturating_sub(2);
    body.push_str(&format!(
        r##"<rect x="{border_x}" y="{border_y}" width="{border_size}" height="{border_size}" rx="16" fill="none" stroke="{accent}" stroke-opacity=".66" stroke-width="1.5" clip-path="url(#{clip_id})"/>"##
    ));
}

fn render_small_artwork(
    body: &mut String,
    artwork: &ResolvedActivityArtwork,
    art_x: u32,
    art_y: u32,
    art_size: u32,
    index: usize,
    embedded: &EmbeddedArtworkBatch,
    accent: &str,
) {
    let Some(small) = artwork.small.as_ref() else {
        return;
    };
    let Some(data_uri) = small.url.as_deref().and_then(|url| embedded.href(url)) else {
        return;
    };
    let size = 72;
    let x = art_x + art_size - size + 7;
    let y = art_y + art_size - size + 7;
    let clip = format!("small-clip-{index}");
    body.push_str(&format!(
        r##"<defs><clipPath id="{clip}"><rect x="{x}" y="{y}" width="{size}" height="{size}" rx="17"/></clipPath></defs><rect x="{x}" y="{y}" width="{size}" height="{size}" rx="17" fill="#0b0f1a" stroke="{accent}" stroke-width="1.4"/><image x="{x}" y="{y}" width="{size}" height="{size}" href="{}" preserveAspectRatio="xMidYMid slice" clip-path="url(#{clip})"/>"##,
        escape_xml(data_uri)
    ));
}

#[derive(Clone, Copy)]
enum MetaIcon {
    Clock,
    Gamepad,
    Music,
    Party,
}

fn render_meta_icon(
    body: &mut String,
    icon: MetaIcon,
    x: u32,
    baseline: u32,
    color: &str,
    prominent: bool,
) {
    let cy = baseline.saturating_sub(if prominent { 7 } else { 6 });
    match icon {
        MetaIcon::Clock => {
            let radius = if prominent { 11 } else { 9 };
            let hand = if prominent { 6 } else { 5 };
            let minute = if prominent { 5 } else { 4 };
            body.push_str(&format!(
                r##"<circle cx="{}" cy="{cy}" r="{radius}" fill="none" stroke="{color}" stroke-width="{}"/><path d="M {} {} V {} M {} {} L {} {}" fill="none" stroke="{color}" stroke-width="{}" stroke-linecap="round"/>"##,
                x + 12,
                if prominent { 2.4 } else { 2.0 },
                x + 12,
                cy,
                cy.saturating_sub(hand),
                x + 12,
                cy,
                x + 12 + minute,
                cy + 2,
                if prominent { 2.4 } else { 2.0 },
            ));
        }
        MetaIcon::Gamepad => {
            let width = if prominent { 30 } else { 24 };
            let height = if prominent { 25 } else { 20 };
            let y = cy.saturating_sub(if prominent { 15 } else { 12 });
            body.push_str(&format!(
                r##"<svg x="{x}" y="{y}" width="{width}" height="{height}" viewBox="0 0 24 24" overflow="visible"><path d="M21.58 16.09 20.49 8.43A5.02 5.02 0 0 0 15.54 4H8.46A5.02 5.02 0 0 0 3.51 8.43l-1.09 7.66A3 3 0 0 0 7.51 18.12L9.63 16h4.74l2.12 2.12a3 3 0 0 0 5.09-2.03ZM10 11H8v2H6v-2H4V9h2V7h2v2h2v2Zm5.5 2a1.5 1.5 0 1 1 0-3 1.5 1.5 0 0 1 0 3Zm3-3a1.5 1.5 0 1 1 0-3 1.5 1.5 0 0 1 0 3Z" fill="{color}"/></svg>"##
            ));
        }
        MetaIcon::Music => body.push_str(&format!(
            r##"<path d="M {} {} V {} L {} {} V {}" fill="none" stroke="{color}" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/><circle cx="{}" cy="{}" r="3.2" fill="{color}"/><circle cx="{}" cy="{}" r="3.2" fill="{color}"/>"##,
            x + 10, cy + 3, cy.saturating_sub(7), x + 21, cy.saturating_sub(10), cy,
            x + 7, cy + 5, x + 18, cy + 2
        )),
        MetaIcon::Party => body.push_str(&format!(
            r##"<circle cx="{}" cy="{}" r="4" fill="none" stroke="{color}" stroke-width="2"/><circle cx="{}" cy="{}" r="3.5" fill="none" stroke="{color}" stroke-width="2"/><path d="M {} {} Q {} {}, {} {} M {} {} Q {} {}, {} {}" fill="none" stroke="{color}" stroke-width="2" stroke-linecap="round"/>"##,
            x + 8, cy.saturating_sub(5), x + 18, cy.saturating_sub(4),
            x + 1, cy + 7, x + 8, cy, x + 15, cy + 7,
            x + 11, cy + 7, x + 18, cy + 1, x + 24, cy + 7
        )),
    }
}

fn dynamic_time_text(
    body: &mut String,
    x: u32,
    y: u32,
    index: usize,
    kind: &str,
    anchor_ms: u64,
    value: &str,
    color: &str,
    font_size: u32,
    weight: &str,
) {
    body.push_str(&format!(
        r##"<text id="activity-time-{index}" data-time-kind="{kind}" data-anchor-ms="{anchor_ms}" x="{x}" y="{y}" fill="{color}" font-family="Inter,Segoe UI,Arial,sans-serif" font-size="{font_size}" font-weight="{weight}">{}</text>"##,
        escape_xml(value)
    ));
}

fn card_artwork_urls(
    runtime: &RuntimeState,
    identity: Option<&DiscordIdentitySnapshot>,
) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(identity) = identity {
        urls.push(identity.data.avatar_url.clone());
        if let Some(url) = identity.data.avatar_decoration_url.as_ref() {
            urls.push(url.clone());
        }
        if let Some(url) = identity
            .data
            .primary_guild
            .as_ref()
            .and_then(|guild| guild.badge_url.as_ref())
        {
            urls.push(url.clone());
        }
    }
    for activity in visible_activities(runtime)
        .into_iter()
        .take(MAX_VISIBLE_ACTIVITIES)
    {
        let artwork = resolve_activity_artwork(activity);
        if let Some(url) = artwork.large.url {
            urls.push(url);
        }
        if let Some(url) = artwork.small.and_then(|small| small.url) {
            urls.push(url);
        }
    }
    urls.sort();
    urls.dedup();
    urls
}

fn visible_activities(runtime: &RuntimeState) -> Vec<&ActivitySnapshot> {
    let PresenceState::Known(snapshot) = &runtime.presence else {
        return Vec::new();
    };
    let mut selected: Vec<&ActivitySnapshot> = Vec::with_capacity(snapshot.data.activities.len());

    for activity in snapshot
        .data
        .activities
        .iter()
        .filter(|activity| activity.kind != ActivityKind::Custom)
    {
        if let Some(index) = selected.iter().position(|candidate| {
            candidate
                .name
                .trim()
                .eq_ignore_ascii_case(activity.name.trim())
        }) {
            if activity_preference_key(activity) > activity_preference_key(selected[index]) {
                selected[index] = activity;
            }
        } else {
            selected.push(activity);
        }
    }

    selected.sort_by_key(|activity| activity_priority(activity.kind));
    selected
}

fn spotify_activity_expired(activity: &ActivitySnapshot, now_unix_ms: u64) -> bool {
    if !is_spotify_activity(activity) {
        return false;
    }

    activity
        .timestamps
        .as_ref()
        .and_then(|timestamps| timestamps.end)
        .is_some_and(|end| now_unix_ms > end.saturating_add(SPOTIFY_EXPIRY_GRACE_MS))
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

fn visible_presence(runtime: &RuntimeState) -> VisiblePresence {
    match (&runtime.presence, runtime.freshness) {
        (PresenceState::Known(_), PresenceFreshness::Unavailable) => VisiblePresence::Unavailable,
        (PresenceState::Known(_), _) => VisiblePresence::Known,
        (PresenceState::Unknown, _) => VisiblePresence::Unknown,
    }
}

#[derive(Clone, Copy)]
enum VisiblePresence {
    Known,
    Unavailable,
    Unknown,
}

fn custom_status(runtime: &RuntimeState) -> Option<&str> {
    let PresenceState::Known(snapshot) = &runtime.presence else {
        return None;
    };
    snapshot
        .data
        .activities
        .iter()
        .find(|activity| activity.kind == ActivityKind::Custom)
        .and_then(|activity| {
            non_empty(activity.state.as_deref()).or_else(|| non_empty(activity.details.as_deref()))
        })
}

fn status_color(runtime: &RuntimeState) -> &'static str {
    let status = match &runtime.presence {
        PresenceState::Known(snapshot) if runtime.freshness != PresenceFreshness::Unavailable => {
            snapshot.data.status
        }
        _ => PresenceStatus::Offline,
    };
    match status {
        PresenceStatus::Online => "#43c987",
        PresenceStatus::Idle => "#f3bf4f",
        PresenceStatus::DoNotDisturb => "#ef5f67",
        PresenceStatus::Invisible | PresenceStatus::Offline => "#80869a",
    }
}

fn activity_time(activity: &ActivitySnapshot, now: u64) -> Option<(String, &'static str, u64)> {
    let timestamps = activity.timestamps.as_ref()?;
    if is_spotify_activity(activity) {
        return None;
    }
    if let Some(start) = timestamps.start
        && now >= start
    {
        return Some((format_duration(now - start), "elapsed", start));
    }
    if let Some(end) = timestamps.end
        && end >= now
    {
        return Some((format_duration(end - now), "remaining", end));
    }
    None
}

fn media_progress(activity: &ActivitySnapshot, now: u64) -> Option<(u64, u64, u64, u64, f64)> {
    let timestamps = activity.timestamps.as_ref()?;
    let start = timestamps.start?;
    let end = timestamps.end?;
    if end <= start {
        return None;
    }
    let duration = end - start;
    let elapsed = now.saturating_sub(start).min(duration);
    let progress = elapsed as f64 / duration as f64;
    Some((start, end, elapsed, duration, progress))
}

fn has_visible_timestamps(runtime: &RuntimeState) -> bool {
    visible_activities(runtime)
        .iter()
        .any(|activity| activity.timestamps.is_some())
}

fn activity_accent(activity: &ActivitySnapshot) -> &'static str {
    activity_accent_for(&activity.name, activity.kind)
}

fn activity_accent_for(name: &str, kind: ActivityKind) -> &'static str {
    let name = name.trim();
    if name.eq_ignore_ascii_case("spotify") {
        "#31d978"
    } else if name.eq_ignore_ascii_case("soundcloud") {
        "#ff6b35"
    } else if name.eq_ignore_ascii_case("youtube music") || name.eq_ignore_ascii_case("yt music") {
        "#ff3b30"
    } else if name.eq_ignore_ascii_case("genshin impact") || name.eq_ignore_ascii_case("genshin") {
        "#56c7ef"
    } else if name.eq_ignore_ascii_case("valorant") {
        "#8f7cff"
    } else if name.eq_ignore_ascii_case("rustrover") {
        "#ff675d"
    } else {
        activity_kind_color(kind)
    }
}

fn activity_kind_color(kind: ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Listening => "#31d978",
        ActivityKind::Playing | ActivityKind::Competing => "#8f7cff",
        ActivityKind::Streaming => "#c877ff",
        ActivityKind::Watching => "#56c7ef",
        ActivityKind::Custom | ActivityKind::Unknown(_) => "#8ea0c7",
    }
}

fn public_badges(flags: u64) -> Vec<&'static str> {
    let mut badges = Vec::new();
    if flags & (1 << 0) != 0 {
        badges.push("STAFF");
    }
    if flags & (1 << 1) != 0 {
        badges.push("PARTNER");
    }
    if flags & (1 << 3) != 0 {
        badges.push("BUG");
    }
    if flags & (1 << 9) != 0 {
        badges.push("EARLY");
    }
    if flags & (1 << 14) != 0 {
        badges.push("BUG II");
    }
    if flags & (1 << 17) != 0 {
        badges.push("DEV");
    }
    badges
}

fn avatar_fallback(body: &mut String, x: u32, y: u32, size: u32) {
    body.push_str(&format!(
        r##"<circle cx="{}" cy="{}" r="{}" fill="#232845"/>"##,
        x + size / 2,
        y + size / 2,
        size / 2
    ));
}

fn empty_state(body: &mut String, height: u32, message: &str) {
    text(
        body,
        CARD_WIDTH / 2,
        height / 2 + 50,
        28,
        "#a8b0ce",
        "600",
        message,
        Some("middle"),
    );
}

fn fallback_label(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(3)
        .collect::<String>()
        .to_ascii_uppercase()
}

fn format_duration(ms: u64) -> String {
    let seconds = ms / 1000;
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn format_clock(ms: u64) -> String {
    format_duration(ms)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn truncate_chars(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let mut out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        if max > 1 {
            out.pop();
        }
        out.push('…');
    }
    out
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn text(
    body: &mut String,
    x: u32,
    y: u32,
    size: u32,
    fill: &str,
    weight: &str,
    value: &str,
    anchor: Option<&str>,
) {
    let anchor = anchor.unwrap_or("start");
    body.push_str(&format!(
        r##"<text x="{x}" y="{y}" fill="{fill}" font-family="Inter,Segoe UI,Arial,sans-serif" font-size="{size}" font-weight="{weight}" text-anchor="{anchor}">{}</text>"##,
        escape_xml(value)
    ));
}

fn fitted_text(
    body: &mut String,
    x: u32,
    y: u32,
    size: u32,
    fill: &str,
    weight: &str,
    value: &str,
    max_width: u32,
    anchor: Option<&str>,
) {
    let anchor = anchor.unwrap_or("start");
    let estimated_width = value.chars().count() as f64 * f64::from(size) * 0.56;
    let fit = estimated_width > f64::from(max_width);
    let length_attributes = if fit {
        format!(" textLength=\"{max_width}\" lengthAdjust=\"spacingAndGlyphs\"")
    } else {
        String::new()
    };
    body.push_str(&format!(
        r##"<text x="{x}" y="{y}" fill="{fill}" font-family="Inter,Segoe UI,Arial,sans-serif" font-size="{size}" font-weight="{weight}" text-anchor="{anchor}"{length_attributes}>{}</text>"##,
        escape_xml(value)
    ));
}

#[cfg(test)]
mod tests {
    use crate::state::{ActivityKind, ActivitySnapshot, ActivityTimestampsSnapshot};

    use super::{
        Panel, activity_accent_for, activity_title_style, fitted_text, format_duration,
        media_progress, render_app_title, spotify_activity_expired, truncate_chars,
    };

    #[test]
    fn formats_elapsed_time_for_activity_cards() {
        assert_eq!(format_duration(126_000), "02:06");
        assert_eq!(format_duration(7_337_000), "02:02:17");
    }

    #[test]
    fn expired_spotify_activity_is_not_rendered_forever() {
        let activity = ActivitySnapshot {
            application_id: None,
            assets: None,
            details: Some("Old track".to_owned()),
            kind: ActivityKind::Listening,
            name: "Spotify".to_owned(),
            party: None,
            state: Some("Artist".to_owned()),
            timestamps: Some(ActivityTimestampsSnapshot {
                start: Some(1_000),
                end: Some(181_000),
            }),
        };

        assert!(!spotify_activity_expired(&activity, 190_000));
        assert!(spotify_activity_expired(&activity, 200_000));
    }

    #[test]
    fn uses_app_specific_accents_when_the_design_calls_for_them() {
        assert_eq!(
            activity_accent_for("Spotify", ActivityKind::Listening),
            "#31d978"
        );
        assert_eq!(
            activity_accent_for("SoundCloud", ActivityKind::Listening),
            "#ff6b35"
        );
        assert_eq!(
            activity_accent_for("YouTube Music", ActivityKind::Listening),
            "#ff3b30"
        );
        assert_eq!(
            activity_accent_for("Genshin Impact", ActivityKind::Playing),
            "#56c7ef"
        );
    }

    #[test]
    fn playing_title_uses_neutral_verb_and_dynamic_name_accent() {
        let activity = ActivitySnapshot {
            application_id: None,
            assets: None,
            details: None,
            kind: ActivityKind::Playing,
            name: "Wuthering Waves".to_owned(),
            party: None,
            state: None,
            timestamps: None,
        };
        assert_eq!(
            activity_title_style(&activity),
            ("Playing", "#f8f8ff", "#8f7cff")
        );
    }

    #[test]
    fn compact_playing_title_uses_the_same_stacked_hierarchy() {
        let activity = ActivitySnapshot {
            application_id: None,
            assets: None,
            details: None,
            kind: ActivityKind::Playing,
            name: "Wuthering Waves".to_owned(),
            party: None,
            state: None,
            timestamps: None,
        };
        let mut svg = String::new();
        render_app_title(
            &mut svg,
            &activity,
            300,
            250,
            Panel {
                x: 62,
                y: 228,
                width: 815,
                height: 300,
            },
        );

        assert!(svg.contains("font-size=\"16\""));
        assert!(svg.contains(">Playing</text>"));
        assert!(svg.contains("font-size=\"34\""));
        assert!(svg.contains("fill=\"#8f7cff\""));
        assert!(svg.contains(">Wuthering Waves</text>"));
        assert!(!svg.contains("<tspan"));
    }

    #[test]
    fn truncates_long_labels_without_exceeding_limit() {
        assert_eq!(truncate_chars("abcdefgh", 5), "abcd…");
        assert_eq!(truncate_chars("abc", 5), "abc");
    }

    #[test]
    fn fitted_spotify_album_keeps_the_complete_value() {
        let album = "Album · Sơn Tùng M-TP - Chúng Ta Của Tương Lai Deluxe Edition";
        let mut svg = String::new();
        fitted_text(&mut svg, 10, 20, 16, "#fff", "500", album, 180, None);
        assert!(svg.contains(album));
        assert!(!svg.contains('…'));
        assert!(svg.contains("textLength=\"180\""));
    }

    #[test]
    fn listening_rpc_with_start_and_end_has_media_progress() {
        let activity = ActivitySnapshot {
            application_id: None,
            assets: None,
            details: Some("Track".to_owned()),
            kind: ActivityKind::Listening,
            name: "SoundCloud".to_owned(),
            party: None,
            state: Some("Artist".to_owned()),
            timestamps: Some(ActivityTimestampsSnapshot {
                start: Some(10_000),
                end: Some(250_000),
            }),
        };

        let (_, _, elapsed, duration, progress) =
            media_progress(&activity, 70_000).expect("listening progress");
        assert_eq!(elapsed, 60_000);
        assert_eq!(duration, 240_000);
        assert!((progress - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn semantic_title_replaces_brand_icon() {
        let activity = ActivitySnapshot {
            application_id: None,
            assets: None,
            details: None,
            kind: ActivityKind::Listening,
            name: "Spotify".to_owned(),
            party: None,
            state: None,
            timestamps: None,
        };
        assert_eq!(
            activity_title_style(&activity),
            ("Listening to", "#b9c1dd", "#31d978")
        );
    }
}
