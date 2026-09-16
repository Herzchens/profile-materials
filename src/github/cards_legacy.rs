use std::{fmt::Write as _, sync::OnceLock, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};

use super::stats::{GitHubSnapshot, is_stale};

const SVG_REVISION: u8 = 12;
const TITLE: &str = "#70A5FD";
const ICON: &str = "#BF91F3";
const TEXT: &str = "#38BDAE";
const BG: &str = "#1A1B27";
const MUTED: &str = "#A8A8A8";
const STREAK_MASCOT_LIT: &[u8] = include_bytes!("../../assets/streak/mascot-campfire-lit.webp");
const STREAK_MASCOT_OUT: &[u8] = include_bytes!("../../assets/streak/mascot-campfire-out.webp");
static STREAK_MASCOT_LIT_DATA_URI: OnceLock<String> = OnceLock::new();
static STREAK_MASCOT_OUT_DATA_URI: OnceLock<String> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitHubCardKind {
    Stats,
    Languages,
    Streak,
}

impl GitHubCardKind {
    const fn key(self) -> &'static str {
        match self {
            Self::Stats => "stats",
            Self::Languages => "languages",
            Self::Streak => "streak",
        }
    }

    const fn dimensions(self) -> (u32, u32) {
        match self {
            Self::Stats => (437, 195),
            Self::Languages => (300, 340),
            Self::Streak => (835, 370),
        }
    }
}

pub struct GitHubCardDocument {
    body: String,
    etag: String,
    revision: u64,
}

impl GitHubCardDocument {
    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn etag(&self) -> &str {
        &self.etag
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

pub fn render_card(
    kind: GitHubCardKind,
    snapshot: Option<&GitHubSnapshot>,
    now_unix_ms: u64,
    stale_after: Duration,
) -> GitHubCardDocument {
    match snapshot {
        Some(snapshot) => {
            let stale = is_stale(snapshot, now_unix_ms, stale_after);
            let body = match kind {
                GitHubCardKind::Stats => render_stats(snapshot, stale),
                GitHubCardKind::Languages => render_languages(snapshot, stale),
                GitHubCardKind::Streak => render_streak(snapshot, stale),
            };
            GitHubCardDocument {
                body,
                etag: format!(
                    "\"profile-github-card-v{SVG_REVISION}-{}-{}-{}\"",
                    kind.key(),
                    snapshot.revision,
                    if stale { "stale" } else { "fresh" }
                ),
                revision: snapshot.revision,
            }
        }
        None => GitHubCardDocument {
            body: render_unavailable(kind),
            etag: format!(
                "\"profile-github-card-v{SVG_REVISION}-{}-unavailable\"",
                kind.key()
            ),
            revision: 0,
        },
    }
}

fn render_stats(snapshot: &GitHubSnapshot, stale: bool) -> String {
    let mut body = String::with_capacity(8_000);
    push_open(&mut body, 437, 195, "GitHub statistics");
    push_tokyonight_style(&mut body);
    let _ = write!(
        body,
        r##"<rect width="437" height="195" rx="4.5" fill="{BG}"/>"##
    );

    let possessive = if snapshot.login.to_ascii_lowercase().ends_with('s') {
        "'"
    } else {
        "'s"
    };
    let _ = write!(
        body,
        r##"<text x="25" y="35" class="header">{}{} GitHub Stats</text>"##,
        escape_xml(&snapshot.login),
        possessive
    );

    let stats = [
        ("★", "Total Stars", snapshot.overall.total_stars),
        ("↻", "Total Commits", snapshot.overall.total_commits),
        ("⑂", "Total PRs", snapshot.overall.total_pull_requests),
        ("!", "Total Issues", snapshot.overall.total_issues),
        ("◆", "Contributed to", snapshot.overall.contributed_to),
    ];
    for (index, (icon, label, value)) in stats.iter().enumerate() {
        let y = 63 + index as u32 * 25;
        let delay = 450 + index * 150;
        let _ = write!(
            body,
            r##"<g class="stagger" style="animation-delay:{delay}ms"><text x="25" y="{y}" class="stat-icon">{icon}</text><text x="50" y="{y}" class="stat">{label}:</text><text x="222" y="{y}" class="stat stat-value">{}</text></g>"##,
            format_count(*value)
        );
    }

    let circumference = 2.0 * std::f64::consts::PI * 40.0;
    let percentile = snapshot.overall.rank.percentile.clamp(0.0, 100.0);
    let dash_offset = circumference * percentile / 100.0;
    let _ = write!(
        body,
        r##"<g transform="translate(362,98)"><circle class="rank-rim" r="40"/><circle class="rank-ring" r="40" stroke-dasharray="{circumference:.2}" stroke-dashoffset="{dash_offset:.2}"/><text x="0" y="7" text-anchor="middle" class="rank-level">{}</text><text x="0" y="28" text-anchor="middle" class="rank-percent">top {:.1}%</text></g>"##,
        escape_xml(&snapshot.overall.rank.level),
        percentile
    );

    if stale {
        body.push_str(
            r##"<text x="425" y="187" text-anchor="end" class="stale">last known</text>"##,
        );
    }
    body.push_str("</svg>");
    body
}

fn render_languages(snapshot: &GitHubSnapshot, stale: bool) -> String {
    let rows = snapshot.languages.len().min(20);
    let height = 90 + rows.div_ceil(2) as u32 * 25;
    let mut body = String::with_capacity(10_000);
    push_open(&mut body, 300, height, "Most used languages");
    push_tokyonight_style(&mut body);
    let _ = write!(
        body,
        r##"<rect width="300" height="{height}" rx="4.5" fill="{BG}"/>"##
    );
    body.push_str(r##"<text x="25" y="35" class="header">Most Used Languages</text><defs><clipPath id="lang-bar"><rect x="25" y="54" width="250" height="8" rx="4"/></clipPath></defs>"##);

    let mut bar_x = 25.0_f64;
    for language in snapshot.languages.iter().take(20) {
        let segment = language.percent.clamp(0.0, 100.0) * 2.5;
        if segment <= 0.0 {
            continue;
        }
        let color = language.color.as_deref().unwrap_or("#858585");
        let _ = write!(
            body,
            r##"<rect clip-path="url(#lang-bar)" x="{bar_x:.2}" y="54" width="{segment:.2}" height="8" fill="{}"/>"##,
            escape_xml(color)
        );
        bar_x += segment;
    }

    if rows == 0 {
        body.push_str(r##"<text x="25" y="95" class="lang">Language data is unavailable.</text>"##);
    } else {
        let split = rows.div_ceil(2);
        for (index, language) in snapshot.languages.iter().take(rows).enumerate() {
            let second_column = index >= split;
            let row = if second_column { index - split } else { index };
            let x = if second_column { 165 } else { 25 };
            let y = 92 + row as u32 * 25;
            let color = language.color.as_deref().unwrap_or("#858585");
            let delay = 450 + index * 100;
            let _ = write!(
                body,
                r##"<g class="stagger" style="animation-delay:{delay}ms"><circle cx="{x}" cy="{}" r="5" fill="{}"/><text x="{}" y="{y}" class="lang">{} {:.2}%</text></g>"##,
                y - 4,
                escape_xml(color),
                x + 15,
                escape_xml(&language.name),
                language.percent
            );
        }
    }

    if stale {
        let _ = write!(
            body,
            r##"<text x="288" y="{}" text-anchor="end" class="stale">last known</text>"##,
            height - 8
        );
    }
    body.push_str("</svg>");
    body
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CampfireState {
    Lit,
    Out,
    Unknown,
}

fn campfire_state(snapshot: &GitHubSnapshot, stale: bool) -> CampfireState {
    if stale {
        return CampfireState::Unknown;
    }
    match snapshot.contributions.today_contributions {
        Some(count) if count > 0 => CampfireState::Lit,
        Some(_) => CampfireState::Out,
        None => CampfireState::Unknown,
    }
}

fn render_streak(snapshot: &GitHubSnapshot, stale: bool) -> String {
    let mut body = String::with_capacity(54_000);
    push_open(&mut body, 835, 370, "GitHub contribution streak");
    push_streak_defs(&mut body);

    body.push_str(
        r##"<rect width="835" height="370" rx="24" fill="url(#streak-bg)"/><rect x="1" y="1" width="833" height="368" rx="23" fill="none" stroke="#6d5ed0" stroke-opacity=".58" stroke-width="1.5"/><ellipse cx="505" cy="225" rx="168" ry="120" fill="url(#center-ambient)" opacity=".72"/><line x1="210" y1="78" x2="210" y2="318" stroke="url(#divider-left)"/><line x1="625" y1="78" x2="625" y2="318" stroke="url(#divider-right)"/><line x1="245" y1="33" x2="337" y2="33" stroke="#685ac1" stroke-opacity=".42"/><line x1="498" y1="33" x2="590" y2="33" stroke="#685ac1" stroke-opacity=".42"/><text x="417.5" y="40" text-anchor="middle" class="streak-title">Contribution Streak</text><circle cx="82" cy="52" r="1.5" class="star"/><circle cx="183" cy="301" r="1.2" class="star"/><circle cx="652" cy="68" r="1.4" class="star"/><circle cx="780" cy="292" r="1.3" class="star"/>"##,
    );

    let total_range = format_date_range(
        snapshot.contributions.calendar_start.as_deref(),
        snapshot.contributions.calendar_end.as_deref(),
        true,
    );
    let current_range = format_date_range(
        snapshot.contributions.current_streak_start.as_deref(),
        snapshot.contributions.current_streak_end.as_deref(),
        false,
    );
    let longest_range = format_date_range(
        snapshot.contributions.longest_streak_start.as_deref(),
        snapshot.contributions.longest_streak_end.as_deref(),
        false,
    );

    let _ = write!(
        body,
        r##"<g class="fade"><text x="105" y="151" text-anchor="middle" class="side-number">{}</text><text x="105" y="187" text-anchor="middle" class="side-label">Total Contributions</text><text x="105" y="216" text-anchor="middle" class="range">{}</text></g>"##,
        snapshot.contributions.total,
        escape_xml(&total_range)
    );
    let _ = write!(
        body,
        r##"<g class="fade" style="animation-delay:180ms"><text x="730" y="151" text-anchor="middle" class="side-number">{}</text><text x="730" y="187" text-anchor="middle" class="side-label">Longest Streak</text><text x="730" y="216" text-anchor="middle" class="range">{}</text></g>"##,
        snapshot.contributions.longest_streak_days,
        escape_xml(&longest_range)
    );

    let state = campfire_state(snapshot, stale);
    let mascot_uri = streak_mascot_data_uri(state);
    let mascot_state = if state == CampfireState::Lit {
        "lit"
    } else {
        "out"
    };
    let _ = write!(
        body,
        r##"<image data-mascot-state="{mascot_state}" x="222" y="55" width="292" height="292" href="{}" preserveAspectRatio="xMidYMid meet" opacity=".98"/>"##,
        escape_xml(mascot_uri)
    );

    render_campfire(&mut body, state);
    let current_class = if state == CampfireState::Lit {
        "current-number current-number-lit"
    } else {
        "current-number current-number-muted"
    };
    let _ = write!(
        body,
        r##"<text x="515" y="128" text-anchor="middle" class="{current_class}">{}</text><text x="515" y="309" text-anchor="middle" class="current-label">Current Streak</text><text x="515" y="336" text-anchor="middle" class="range current-range">{}</text>"##,
        snapshot.contributions.current_streak_days,
        escape_xml(&current_range)
    );

    if stale {
        body.push_str(
            r##"<text x="817" y="352" text-anchor="end" class="stale-note">last known</text>"##,
        );
    }
    body.push_str("</svg>");
    body
}

fn push_streak_defs(body: &mut String) {
    body.push_str(
        r##"<defs>
<linearGradient id="streak-bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#090d18"/><stop offset=".5" stop-color="#101225"/><stop offset="1" stop-color="#090d18"/></linearGradient>
<radialGradient id="center-ambient"><stop offset="0" stop-color="#432b72" stop-opacity=".42"/><stop offset=".56" stop-color="#2a2254" stop-opacity=".16"/><stop offset="1" stop-color="#0b0f1d" stop-opacity="0"/></radialGradient>
<linearGradient id="divider-left" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#8c91b1" stop-opacity="0"/><stop offset=".22" stop-color="#8c91b1" stop-opacity=".36"/><stop offset=".78" stop-color="#8c91b1" stop-opacity=".36"/><stop offset="1" stop-color="#8c91b1" stop-opacity="0"/></linearGradient>
<linearGradient id="divider-right" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#8c91b1" stop-opacity="0"/><stop offset=".22" stop-color="#8c91b1" stop-opacity=".36"/><stop offset=".78" stop-color="#8c91b1" stop-opacity=".36"/><stop offset="1" stop-color="#8c91b1" stop-opacity="0"/></linearGradient>
<linearGradient id="flame-outer" x1="0" y1="1" x2="0" y2="0"><stop offset="0" stop-color="#ff6b3d"/><stop offset=".52" stop-color="#ff9f43"/><stop offset="1" stop-color="#ffd66b"/></linearGradient>
<linearGradient id="flame-inner" x1="0" y1="1" x2="0" y2="0"><stop offset="0" stop-color="#8e5cff"/><stop offset=".54" stop-color="#ff7f50"/><stop offset="1" stop-color="#fff0a7"/></linearGradient>
<linearGradient id="number-gradient" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#b68cff"/><stop offset=".55" stop-color="#d48cff"/><stop offset="1" stop-color="#ffbd68"/></linearGradient>
<linearGradient id="smoke-plume" x1="0" y1="6" x2="0" y2="-78" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#777d91" stop-opacity=".34"/><stop offset=".48" stop-color="#9299b1" stop-opacity=".22"/><stop offset="1" stop-color="#c0c6db" stop-opacity="0"/></linearGradient>
<linearGradient id="smoke-wisp" x1="0" y1="5" x2="0" y2="-66" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#6f758a" stop-opacity=".26"/><stop offset=".5" stop-color="#9ba2ba" stop-opacity=".16"/><stop offset="1" stop-color="#c4cadc" stop-opacity="0"/></linearGradient>
<filter id="fire-glow" x="-80%" y="-80%" width="260%" height="260%"><feGaussianBlur stdDeviation="10"/></filter>
<filter id="number-glow" x="-30%" y="-70%" width="160%" height="240%"><feDropShadow dx="0" dy="0" stdDeviation="6" flood-color="#ff9b55" flood-opacity=".42"/></filter>
<filter id="smoke-soft" x="-40%" y="-30%" width="180%" height="170%"><feGaussianBlur stdDeviation="1.35"/></filter>
</defs>
<style>
.streak-title{font:700 19px 'Segoe UI',Ubuntu,Sans-Serif;fill:#eef0ff;letter-spacing:.2px}
.side-number{font:700 39px 'Segoe UI',Ubuntu,Sans-Serif;fill:#76a7ff}
.side-label{font:600 17px 'Segoe UI',Ubuntu,Sans-Serif;fill:#76a7ff}
.current-number{font:800 48px 'Segoe UI',Ubuntu,Sans-Serif}
.current-number-lit{fill:url(#number-gradient);filter:url(#number-glow)}
.current-number-muted{fill:#8d93a8}
.current-label{font:700 18px 'Segoe UI',Ubuntu,Sans-Serif;fill:#c38cff}
.range{font:500 12px 'Segoe UI',Ubuntu,Sans-Serif;fill:#46d4cc}
.current-range{font-size:13px}
.stale-note{font:500 10px 'Segoe UI',Ubuntu,Sans-Serif;fill:#8d94b8}
.star{fill:#aa92ff;opacity:.62}
.fade{opacity:0;animation:fade-in .48s ease-out forwards}
.fire-ignite{transform-box:fill-box;transform-origin:center bottom;animation:ignite .78s cubic-bezier(.18,.78,.25,1) both}
.flame-motion{transform-box:fill-box;transform-origin:center bottom;animation:flame-body 2.8s .78s ease-in-out infinite}
.flame-outer-motion{transform-box:fill-box;transform-origin:center bottom;animation:flame-outer 2.8s .78s ease-in-out infinite}
.flame-inner-motion{transform-box:fill-box;transform-origin:center bottom;animation:flame-inner 2.8s .78s ease-in-out infinite}
.fire-glow-motion{transform-box:fill-box;transform-origin:center;animation:glow-pulse 2.8s .78s ease-in-out infinite}
.ember{opacity:0;animation:ember-rise 2.2s ease-out infinite}.ember-2{animation-delay:.55s}.ember-3{animation-delay:1.15s}.ember-4{animation-delay:1.6s}
@keyframes fade-in{from{opacity:0}to{opacity:1}}
@keyframes ignite{0%{opacity:.12;transform:translateY(6px) scale(.68,.58)}55%{opacity:1;transform:translateY(-2px) scale(1.06,1.1)}100%{opacity:1;transform:translateY(0) scale(1)}}
@keyframes flame-body{0%{transform:translateY(0) rotate(-.7deg) scale(1)}45%{transform:translateY(-1.5px) rotate(.8deg) scale(1.015,1.025)}75%{transform:translateY(-.5px) rotate(.15deg) scale(1.008,1.012)}100%{transform:translateY(0) rotate(-.7deg) scale(1)}}
@keyframes flame-outer{0%{opacity:.97;transform:scale(1)}45%{opacity:1;transform:scale(1.015,1.03)}75%{opacity:.99;transform:scale(1.008,1.015)}100%{opacity:.97;transform:scale(1)}}
@keyframes flame-inner{0%{opacity:.92;transform:scale(1)}45%{opacity:1;transform:scale(1.01,1.045)}75%{opacity:.96;transform:scale(1.005,1.02)}100%{opacity:.92;transform:scale(1)}}
@keyframes glow-pulse{0%{opacity:.34;transform:scale(.96,.94)}45%{opacity:.52;transform:scale(1.05,1.02)}75%{opacity:.42;transform:scale(1,.98)}100%{opacity:.34;transform:scale(.96,.94)}}
@keyframes ember-rise{0%{opacity:0;transform:translate(0,0)}18%{opacity:.95}100%{opacity:0;transform:translate(8px,-58px)}}
</style>"##,
    );
}

fn render_campfire(body: &mut String, state: CampfireState) {
    body.push_str(
        r##"<g transform="translate(515 238)"><ellipse cx="0" cy="29" rx="58" ry="17" fill="#060810" opacity=".72"/><ellipse cx="-31" cy="23" rx="19" ry="9" fill="#343545"/><ellipse cx="31" cy="23" rx="19" ry="9" fill="#343545"/><ellipse cx="-50" cy="27" rx="16" ry="8" fill="#292b38"/><ellipse cx="50" cy="27" rx="16" ry="8" fill="#292b38"/><rect x="-41" y="14" width="83" height="12" rx="6" fill="#5a352b" transform="rotate(16)"/><rect x="-41" y="14" width="83" height="12" rx="6" fill="#6a3f2e" transform="rotate(-16)"/>"##,
    );
    match state {
        CampfireState::Lit => body.push_str(
            r##"<g id="campfire-lit"><ellipse class="fire-glow-motion" cx="0" cy="0" rx="64" ry="45" fill="#ff7b4d" opacity=".42" filter="url(#fire-glow)"/><g class="fire-ignite"><g transform="scale(1.22 1)"><g class="flame-motion"><g class="flame-outer-motion"><path d="M0 19C-26 12-31-10-18-29C-10-41-8-52-7-65C7-55 16-43 15-29C25-23 31-11 26 2C22 13 12 19 0 19Z" fill="url(#flame-outer)"/></g><g class="flame-inner-motion"><path d="M1 16C-13 10-16-3-9-14C-3-23-2-30 0-39C10-30 15-20 11-11C18-6 18 5 13 11C10 14 6 16 1 16Z" fill="url(#flame-inner)"/></g></g></g></g><circle class="ember" cx="-18" cy="-19" r="3" fill="#ffcf66"/><circle class="ember ember-2" cx="14" cy="-14" r="2.5" fill="#ff8b4b"/><circle class="ember ember-3" cx="-4" cy="-24" r="2" fill="#ffd979"/><circle class="ember ember-4" cx="22" cy="-9" r="2" fill="#f7a5ff"/></g>"##,
        ),
        CampfireState::Out => body.push_str(
            r##"<g id="campfire-out"><ellipse cx="0" cy="4" rx="31" ry="12" fill="#424550" opacity=".46"/><path id="smoke-plume-main" d="M-7 3C-16-8-14-19-4-27C5-35 7-43 2-51C-4-60-1-69 8-76C5-65 15-59 14-48C13-36 2-32 1-22C0-13 8-7 7 3Z" fill="url(#smoke-plume)" filter="url(#smoke-soft)"/><path id="smoke-plume-side" d="M13 5C6-5 7-14 14-22C21-30 22-38 17-45C13-51 15-58 21-63C19-54 27-49 25-40C23-31 15-28 15-20C15-11 20-4 19 5Z" fill="url(#smoke-wisp)" filter="url(#smoke-soft)"/><circle cx="-6" cy="-1" r="3" fill="#777b86" opacity=".5"/></g>"##,
        ),
        CampfireState::Unknown => body.push_str(
            r##"<g id="campfire-unknown"><ellipse cx="0" cy="3" rx="34" ry="13" fill="#4d4962" opacity=".34"/><path id="smoke-plume-main-unknown" d="M-6 3C-14-7-13-17-4-25C4-32 6-40 2-47C-3-55-1-63 7-69C4-60 13-54 12-44C11-34 2-29 1-20C0-12 7-6 6 3Z" fill="url(#smoke-plume)" opacity=".62" filter="url(#smoke-soft)"/><path d="M12 5C7-4 8-13 14-20C19-27 20-34 17-40C14-46 15-52 20-57C18-49 24-45 23-37C21-29 15-26 15-18C15-10 19-4 18 5Z" fill="url(#smoke-wisp)" opacity=".46" filter="url(#smoke-soft)"/></g>"##,
        ),
    }
    body.push_str("</g>");
}

fn streak_mascot_data_uri(state: CampfireState) -> &'static str {
    let (asset, cache) = match state {
        CampfireState::Lit => (STREAK_MASCOT_LIT, &STREAK_MASCOT_LIT_DATA_URI),
        CampfireState::Out | CampfireState::Unknown => {
            (STREAK_MASCOT_OUT, &STREAK_MASCOT_OUT_DATA_URI)
        }
    };

    cache
        .get_or_init(|| format!("data:image/webp;base64,{}", BASE64_STANDARD.encode(asset)))
        .as_str()
}

fn format_date_range(start: Option<&str>, end: Option<&str>, always_year: bool) -> String {
    let (Some(start), Some(end)) = (start.and_then(parse_iso_date), end.and_then(parse_iso_date))
    else {
        return "range unavailable".to_owned();
    };

    if start == end {
        return format_date_parts(start, true);
    }

    let include_year = always_year || start.0 != end.0;
    if !include_year && start.1 == end.1 {
        return format!("{} {} – {}", month_name(start.1), start.2, end.2);
    }

    format!(
        "{} – {}",
        format_date_parts(start, include_year),
        format_date_parts(end, include_year)
    )
}

fn parse_iso_date(value: &str) -> Option<(i32, u32, u32)> {
    let mut parts = value.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

fn format_date_parts(date: (i32, u32, u32), include_year: bool) -> String {
    if include_year {
        format!("{} {}, {}", month_name(date.1), date.2, date.0)
    } else {
        format!("{} {}", month_name(date.1), date.2)
    }
}

fn month_name(month: u32) -> &'static str {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS[(month.saturating_sub(1).min(11)) as usize]
}

fn render_unavailable(kind: GitHubCardKind) -> String {
    let (width, height) = kind.dimensions();
    let mut body = String::with_capacity(1_000);
    push_open(&mut body, width, height, "GitHub stats unavailable");
    push_tokyonight_style(&mut body);
    let _ = write!(
        body,
        r##"<rect width="{width}" height="{height}" rx="{}" fill="{BG}"/>"##,
        if kind == GitHubCardKind::Streak {
            24
        } else {
            5
        }
    );
    body.push_str(r##"<text x="25" y="42" class="header">GitHub stats</text><text x="25" y="76" class="stat">Stats are temporarily unavailable.</text>"##);
    body.push_str("</svg>");
    body
}

fn push_open(body: &mut String, width: u32, height: u32, label: &str) {
    let _ = write!(
        body,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-label="{}">"##,
        escape_xml(label)
    );
}

fn push_tokyonight_style(body: &mut String) {
    let _ = write!(
        body,
        r##"<style>
        .header{{font:600 18px 'Segoe UI',Ubuntu,'Helvetica Neue',Sans-Serif;fill:{TITLE};opacity:0;animation:fade .8s ease-in-out forwards}}
        .stat{{font:600 14px 'Segoe UI',Ubuntu,'Helvetica Neue',Sans-Serif;fill:{TEXT}}}
        .stat-value{{font-weight:700}}
        .stat-icon{{font:700 15px 'Segoe UI Symbol','Segoe UI',Sans-Serif;fill:{ICON}}}
        .lang{{font:400 11px 'Segoe UI',Ubuntu,'Helvetica Neue',Sans-Serif;fill:{TEXT}}}
        .rank-rim{{fill:none;stroke:{TITLE};stroke-width:6;opacity:.2}}
        .rank-ring{{fill:none;stroke:{TITLE};stroke-width:6;stroke-linecap:round;opacity:.8;transform:rotate(-90deg);transform-origin:center}}
        .rank-level{{font:800 24px 'Segoe UI',Ubuntu,Sans-Serif;fill:{TEXT}}}
        .rank-percent{{font:600 10px 'Segoe UI',Ubuntu,Sans-Serif;fill:{MUTED}}}
        .stale{{font:400 10px 'Segoe UI',Ubuntu,Sans-Serif;fill:{MUTED}}}
        .stagger{{opacity:0;animation:fade .3s ease-in-out forwards}}
        @keyframes fade{{from{{opacity:0}}to{{opacity:1}}}}
        </style>"##
    );
}

fn format_count(value: u64) -> String {
    if value >= 1_000_000 {
        trim_one_decimal(value as f64 / 1_000_000.0, "m")
    } else if value >= 1_000 {
        trim_one_decimal(value as f64 / 1_000.0, "k")
    } else {
        value.to_string()
    }
}

fn trim_one_decimal(value: f64, suffix: &str) -> String {
    let mut formatted = format!("{value:.1}");
    if formatted.ends_with(".0") {
        formatted.truncate(formatted.len() - 2);
    }
    formatted.push_str(suffix);
    formatted
}

fn escape_xml(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            _ => output.push(character),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{GitHubCardKind, format_count, render_card};
    use crate::github::stats::{
        ContributionSummary, GitHubSnapshot, LanguageShare, OverallStats, RankSnapshot,
        RateLimitSnapshot,
    };

    #[test]
    fn compact_number_format_matches_readme_style() {
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1k");
        assert_eq!(format_count(1_250), "1.2k");
        assert_eq!(format_count(1_000_000), "1m");
    }

    #[test]
    fn cards_do_not_expose_private_repository_details() {
        let snapshot = snapshot();
        for kind in [
            GitHubCardKind::Stats,
            GitHubCardKind::Languages,
            GitHubCardKind::Streak,
        ] {
            let card = render_card(kind, Some(&snapshot), 100, Duration::from_secs(60));
            assert!(!card.body().contains("private-repo"));
        }
    }

    #[test]
    fn active_day_renders_animated_lit_campfire_and_embedded_mascot() {
        let card = render_card(
            GitHubCardKind::Streak,
            Some(&snapshot()),
            100,
            Duration::from_secs(60),
        );
        assert!(card.body().contains("id=\"campfire-lit\""));
        assert!(card.body().contains("@keyframes ignite"));
        assert!(card.body().contains("@keyframes flame-body"));
        assert!(!card.body().contains("<animateMotion"));
        assert!(!card.body().contains("side-orbit"));
        assert!(!card.body().contains("side-drift-"));
        assert!(!card.body().contains("@keyframes drift-x-left"));
        assert!(!card.body().contains("@keyframes drift-y-left"));
        assert!(!card.body().contains("@keyframes drift-x-right"));
        assert!(!card.body().contains("@keyframes drift-y-right"));
        assert!(!card.body().contains("class=\"spark\""));
        assert!(!card.body().contains("M230 84h10M235 79v10"));
        assert!(card.body().contains("class=\"flame-motion\""));
        assert!(card.body().contains("scale(1.22 1)"));
        assert!(card.body().contains("data:image/webp;base64,"));
        assert!(card.body().contains("data-mascot-state=\"lit\""));
        assert!(
            card.body()
                .contains("x=\"222\" y=\"55\" width=\"292\" height=\"292\"")
        );
        assert!(!card.body().contains("mascot-mask"));
        assert!(card.body().contains(">Sep 8 – 14</text>"));
        assert!(card.body().contains(">4376</text>"));
        assert!(!card.body().contains(">4.4k</text>"));
    }

    #[test]
    fn empty_current_day_extinguishes_campfire_without_erasing_streak() {
        let mut snapshot = snapshot();
        snapshot.contributions.today_contributions = Some(0);
        let card = render_card(
            GitHubCardKind::Streak,
            Some(&snapshot),
            100,
            Duration::from_secs(60),
        );
        assert!(card.body().contains("id=\"campfire-out\""));
        assert!(!card.body().contains("id=\"campfire-lit\""));
        assert!(card.body().contains("id=\"smoke-plume-main\""));
        assert!(card.body().contains("id=\"smoke-plume-side\""));
        assert!(card.body().contains("data-mascot-state=\"out\""));
        assert!(
            card.body()
                .contains("class=\"current-number current-number-muted\"")
        );
        assert!(!card.body().contains("animation:smoke"));
        assert!(card.body().contains(">7</text>"));
    }

    #[test]
    fn stale_snapshot_uses_neutral_campfire_state() {
        let card = render_card(
            GitHubCardKind::Streak,
            Some(&snapshot()),
            61_000,
            Duration::from_secs(60),
        );
        assert!(card.body().contains("id=\"campfire-unknown\""));
        assert!(card.body().contains("id=\"smoke-plume-main-unknown\""));
        assert!(card.body().contains("data-mascot-state=\"out\""));
        assert!(
            card.body()
                .contains("class=\"current-number current-number-muted\"")
        );
        assert!(card.body().contains("last known"));
    }

    fn snapshot() -> GitHubSnapshot {
        GitHubSnapshot {
            revision: 3,
            collected_at_unix_ms: 100,
            login: "Herzchens".to_owned(),
            url: "https://github.com/Herzchens".to_owned(),
            contributions: ContributionSummary {
                total: 4_376,
                commits: 200,
                issues: 10,
                pull_requests: 20,
                reviews: 5,
                active_days: 80,
                current_streak_days: 7,
                longest_streak_days: 21,
                calendar_start: Some("2025-09-15".to_owned()),
                calendar_end: Some("2026-09-14".to_owned()),
                today_contributions: Some(2),
                current_streak_start: Some("2026-09-08".to_owned()),
                current_streak_end: Some("2026-09-14".to_owned()),
                longest_streak_start: Some("2026-08-01".to_owned()),
                longest_streak_end: Some("2026-08-21".to_owned()),
                restricted_contributions: 10,
                includes_restricted_contributions: true,
            },
            overall: OverallStats {
                total_commits: 2_345,
                total_pull_requests: 42,
                total_reviews: 18,
                total_issues: 31,
                total_stars: 19,
                contributed_to: 11,
                followers: 17,
                rank: RankSnapshot {
                    level: "A".to_owned(),
                    percentile: 22.5,
                },
            },
            languages: vec![LanguageShare {
                name: "Go".to_owned(),
                color: Some("#00ADD8".to_owned()),
                bytes: 1_000,
                repository_count: 4,
                score: 63.0,
                percent: 63.0,
            }],
            projects: Vec::new(),
            repository_count: 8,
            source_truncated: false,
            rate_limit: RateLimitSnapshot {
                cost: 1,
                remaining: 4_999,
                reset_at: "2026-09-12T20:00:00Z".to_owned(),
            },
        }
    }
}
