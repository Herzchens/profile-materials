use std::{fmt::Write as _, time::Duration};

use super::stats::{GitHubSnapshot, is_stale};

const SVG_REVISION: u8 = 1;
const TITLE: &str = "#70A5FD";
const ICON: &str = "#BF91F3";
const TEXT: &str = "#38BDAE";
const BG: &str = "#1A1B27";
const MUTED: &str = "#A8A8A8";

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
            Self::Streak => (495, 195),
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

fn render_streak(snapshot: &GitHubSnapshot, stale: bool) -> String {
    let mut body = String::with_capacity(6_000);
    push_open(&mut body, 495, 195, "GitHub contribution streak");
    body.push_str(
        r##"<style>
        .num{font:600 28px 'Segoe UI',Ubuntu,Sans-Serif;fill:#70A5FD}
        .current{font:700 30px 'Segoe UI',Ubuntu,Sans-Serif;fill:#BF91F3}
        .label{font:600 14px 'Segoe UI',Ubuntu,Sans-Serif;fill:#70A5FD}
        .current-label{font:600 14px 'Segoe UI',Ubuntu,Sans-Serif;fill:#BF91F3}
        .date{font:400 12px 'Segoe UI',Ubuntu,Sans-Serif;fill:#38BDAE}
        .stale{font:400 10px 'Segoe UI',Ubuntu,Sans-Serif;fill:#A8A8A8}
        @keyframes fade{from{opacity:0}to{opacity:1}}.fade{opacity:0;animation:fade .45s ease-out forwards}
        </style>
        <line x1="165" y1="32" x2="165" y2="163" stroke="#A8A8A8" stroke-opacity=".24"/>
        <line x1="330" y1="32" x2="330" y2="163" stroke="#A8A8A8" stroke-opacity=".24"/>"##,
    );

    let _ = write!(
        body,
        r##"<g class="fade"><text x="82.5" y="78" text-anchor="middle" class="num">{}</text><text x="82.5" y="108" text-anchor="middle" class="label">Total Contributions</text><text x="82.5" y="132" text-anchor="middle" class="date">last 12 months</text></g>"##,
        format_count(snapshot.contributions.total)
    );
    let _ = write!(
        body,
        r##"<g class="fade" style="animation-delay:120ms"><circle cx="247.5" cy="74" r="42" fill="none" stroke="#70A5FD" stroke-width="6" stroke-opacity=".25"/><path d="M247.5 25 A49 49 0 0 1 289 99" fill="none" stroke="#70A5FD" stroke-width="6" stroke-linecap="round"/><text x="247.5" y="83" text-anchor="middle" class="current">{}</text><text x="247.5" y="132" text-anchor="middle" class="current-label">Current Streak</text><text x="247.5" y="154" text-anchor="middle" class="date">days</text></g>"##,
        snapshot.contributions.current_streak_days
    );
    let _ = write!(
        body,
        r##"<g class="fade" style="animation-delay:240ms"><text x="412.5" y="78" text-anchor="middle" class="num">{}</text><text x="412.5" y="108" text-anchor="middle" class="label">Longest Streak</text><text x="412.5" y="132" text-anchor="middle" class="date">days</text></g>"##,
        snapshot.contributions.longest_streak_days
    );

    if stale {
        body.push_str(
            r##"<text x="485" y="187" text-anchor="end" class="stale">last known</text>"##,
        );
    }
    body.push_str("</svg>");
    body
}

fn render_unavailable(kind: GitHubCardKind) -> String {
    let (width, height) = kind.dimensions();
    let mut body = String::with_capacity(1_000);
    push_open(&mut body, width, height, "GitHub stats unavailable");
    push_tokyonight_style(&mut body);
    if kind != GitHubCardKind::Streak {
        let _ = write!(
            body,
            r##"<rect width="{width}" height="{height}" rx="4.5" fill="{BG}"/>"##
        );
    }
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

    fn snapshot() -> GitHubSnapshot {
        GitHubSnapshot {
            revision: 3,
            collected_at_unix_ms: 100,
            login: "Herzchens".to_owned(),
            url: "https://github.com/Herzchens".to_owned(),
            contributions: ContributionSummary {
                total: 321,
                commits: 200,
                issues: 10,
                pull_requests: 20,
                reviews: 5,
                active_days: 80,
                current_streak_days: 7,
                longest_streak_days: 21,
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
