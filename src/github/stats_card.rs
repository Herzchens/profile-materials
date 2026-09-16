use std::{fmt::Write as _, time::Duration};

use crate::github::stats::{GitHubSnapshot, is_stale};

const WIDTH: u32 = 835;
const HEIGHT: u32 = 300;
const STATS_REVISION: u8 = 6;

pub(super) struct RenderedStatsCard {
    pub(super) body: String,
    pub(super) etag: String,
    pub(super) revision: u64,
}

pub(super) fn render(
    snapshot: Option<&GitHubSnapshot>,
    now_unix_ms: u64,
    stale_after: Duration,
) -> RenderedStatsCard {
    let stale = snapshot.is_some_and(|snapshot| is_stale(snapshot, now_unix_ms, stale_after));
    let revision = snapshot.map_or(0, |snapshot| snapshot.revision);
    let freshness = match snapshot {
        None => "unavailable",
        Some(_) if stale => "stale",
        Some(_) => "fresh",
    };

    RenderedStatsCard {
        body: render_svg(snapshot, stale),
        etag: format!("\"profile-github-stats-v{STATS_REVISION}-{revision}-{freshness}\""),
        revision,
    }
}

fn render_svg(snapshot: Option<&GitHubSnapshot>, stale: bool) -> String {
    let mut body = String::with_capacity(22_000);
    push_open(&mut body);
    push_defs_and_style(&mut body);
    body.push_str(
        r##"<rect width="835" height="300" rx="24" fill="url(#stats-bg)"/><rect x="1" y="1" width="833" height="298" rx="23" fill="none" stroke="#6558be" stroke-opacity=".68" stroke-width="1.5"/><text x="36" y="48" class="title">GitHub Statistics</text><line x1="36" y1="68" x2="799" y2="68" class="rule"/><path d="M650 297c38-21 74-25 104-17 21 6 37 2 52-10" class="canvas-wave"/>"##,
    );

    match snapshot {
        Some(snapshot) => render_snapshot(&mut body, snapshot, stale),
        None => render_unavailable(&mut body),
    }

    body.push_str("</svg>");
    body
}

fn render_snapshot(body: &mut String, snapshot: &GitHubSnapshot, stale: bool) {
    body.push_str(
        r##"<g data-stats-primary="commits"><rect x="28" y="84" width="244" height="180" rx="18" class="primary-card"/><g clip-path="url(#primary-clip)"><path d="M28 248C64 229 96 232 129 246C160 260 183 261 209 239C231 221 252 215 272 220V264H28Z" class="primary-wave-fill"/><path d="M28 257C66 242 97 241 128 252C160 264 191 257 216 244C240 232 256 230 272 233" class="primary-wave-secondary"/><path d="M28 248C64 229 96 232 129 246C160 260 183 261 209 239C231 221 252 215 272 220" class="primary-wave"/></g>"##,
    );
    render_stat_icon(body, Icon::Commit, 51.0, 108.0, 21.0);
    let _ = write!(
        body,
        r##"<text x="82" y="124" class="primary-label">TOTAL COMMITS</text><text x="47" y="205" class="primary-value">{}</text></g>"##,
        format_stat_number(snapshot.overall.total_commits)
    );

    let metrics = [
        Metric::new(
            "Pull Requests",
            snapshot.overall.total_pull_requests,
            Icon::PullRequest,
        ),
        Metric::new("Code Reviews", snapshot.overall.total_reviews, Icon::Review),
        Metric::new("Issues", snapshot.overall.total_issues, Icon::Issue),
        Metric::new("Stars Received", snapshot.overall.total_stars, Icon::Star),
        Metric::new(
            "Contributed To",
            snapshot.overall.contributed_to,
            Icon::Repository,
        ),
        Metric::new("Followers", snapshot.overall.followers, Icon::Followers),
    ];
    let positions = [
        (286.0, 84.0),
        (465.0, 84.0),
        (286.0, 146.0),
        (465.0, 146.0),
        (286.0, 208.0),
        (465.0, 208.0),
    ];
    for (index, (metric, (x, y))) in metrics.iter().zip(positions).enumerate() {
        render_metric(body, metric, x, y, 169.0, 56.0, 70 + index * 45);
    }

    render_rank(body, snapshot);

    if stale {
        body.push_str(
            r##"<text x="799" y="48" text-anchor="end" class="stale">LAST KNOWN</text>"##,
        );
    }
}

fn render_unavailable(body: &mut String) {
    body.push_str(
        r##"<rect x="28" y="84" width="779" height="180" rx="18" class="empty-card"/><text x="54" y="155" class="empty-title">GitHub statistics are temporarily unavailable.</text><text x="54" y="188" class="empty-copy">The service will keep the last known snapshot when one is available.</text>"##,
    );
}

fn render_rank(body: &mut String, snapshot: &GitHubSnapshot) {
    let percentile = snapshot.overall.rank.percentile.clamp(0.0, 100.0);
    let progress = (100.0 - percentile).clamp(0.0, 100.0);
    let remaining = 100.0 - progress;
    let _ = write!(
        body,
        r##"<g data-stats-rank="true" data-rank-progress="{progress:.1}"><rect x="648" y="84" width="159" height="180" rx="18" class="rank-card"/><g transform="translate(727.5 142)"><circle r="41" class="rank-rim"/><circle r="41" class="rank-ring" pathLength="100" stroke-dasharray="{progress:.1} {remaining:.1}" transform="rotate(-90)"/><text x="0" y="11" text-anchor="middle" class="rank-level">{}</text></g><text x="727.5" y="220" text-anchor="middle" class="rank-percent">TOP {:.1}%</text><text x="727.5" y="244" text-anchor="middle" class="rank-label">GITHUB RANK</text></g>"##,
        escape_xml(&snapshot.overall.rank.level),
        percentile
    );
}

fn render_metric(
    body: &mut String,
    metric: &Metric<'_>,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    delay_ms: usize,
) {
    let _ = write!(
        body,
        r##"<g class="metric-enter" style="animation-delay:{delay_ms}ms"><rect x="{x:.1}" y="{y:.1}" width="{width:.1}" height="{height:.1}" rx="14" class="metric-card"/>"##,
    );
    render_stat_icon(body, metric.icon, x + 20.0, y + 17.0, 20.0);
    let _ = write!(
        body,
        r##"<text x="{:.1}" y="{:.1}" class="metric-label">{}</text><text x="{:.1}" y="{:.1}" class="metric-value">{}</text></g>"##,
        x + 52.0,
        y + 23.0,
        escape_xml(metric.label),
        x + 52.0,
        y + 47.0,
        format_stat_number(metric.value)
    );
}

#[derive(Clone, Copy)]
struct Metric<'a> {
    label: &'a str,
    value: u64,
    icon: Icon,
}

impl<'a> Metric<'a> {
    const fn new(label: &'a str, value: u64, icon: Icon) -> Self {
        Self { label, value, icon }
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Commit,
    PullRequest,
    Review,
    Issue,
    Star,
    Repository,
    Followers,
}

fn render_stat_icon(body: &mut String, icon: Icon, x: f64, y: f64, size: f64) {
    let color = icon.color();
    let _ = write!(
        body,
        r##"<g data-stat-icon="{}" data-icon-color="{color}"><svg x="{x:.1}" y="{y:.1}" width="{size:.1}" height="{size:.1}" viewBox="0 0 24 24" aria-hidden="true" fill="{color}" class="stat-icon">"##,
        icon.key(),
    );
    match icon {
        Icon::Commit => body.push_str(
            r##"<path d="M16.944 11h4.306a.75.75 0 0 1 0 1.5h-4.306a5.001 5.001 0 0 1-9.888 0H2.75a.75.75 0 0 1 0-1.5h4.306a5.001 5.001 0 0 1 9.888 0Zm-1.444.75a3.5 3.5 0 1 0-7 0 3.5 3.5 0 0 0 7 0Z"/>"##,
        ),
        Icon::PullRequest => body.push_str(
            r##"<path d="M16 19.25a3.25 3.25 0 1 1 6.5 0 3.25 3.25 0 0 1-6.5 0Zm-14.5 0a3.25 3.25 0 1 1 6.5 0 3.25 3.25 0 0 1-6.5 0Zm0-14.5a3.25 3.25 0 1 1 6.5 0 3.25 3.25 0 0 1-6.5 0ZM4.75 3a1.75 1.75 0 1 0 .001 3.501A1.75 1.75 0 0 0 4.75 3Zm0 14.5a1.75 1.75 0 1 0 .001 3.501A1.75 1.75 0 0 0 4.75 17.5Zm14.5 0a1.75 1.75 0 1 0 .001 3.501 1.75 1.75 0 0 0-.001-3.501Z"/><path d="M13.405 1.72a.75.75 0 0 1 0 1.06L12.185 4h4.065A3.75 3.75 0 0 1 20 7.75v8.75a.75.75 0 0 1-1.5 0V7.75a2.25 2.25 0 0 0-2.25-2.25h-4.064l1.22 1.22a.75.75 0 0 1-1.061 1.06l-2.5-2.5a.75.75 0 0 1 0-1.06l2.5-2.5a.75.75 0 0 1 1.06 0ZM4.75 7.25A.75.75 0 0 1 5.5 8v8A.75.75 0 0 1 4 16V8a.75.75 0 0 1 .75-.75Z"/>"##,
        ),
        Icon::Review => body.push_str(
            r##"<path d="M10.3 6.74a.75.75 0 0 1-.04 1.06l-2.908 2.7 2.908 2.7a.75.75 0 1 1-1.02 1.1l-3.5-3.25a.75.75 0 0 1 0-1.1l3.5-3.25a.75.75 0 0 1 1.06.04Zm3.44 1.06a.75.75 0 1 1 1.02-1.1l3.5 3.25a.75.75 0 0 1 0 1.1l-3.5 3.25a.75.75 0 1 1-1.02-1.1l2.908-2.7-2.908-2.7Z"/><path d="M1.5 4.25c0-.966.784-1.75 1.75-1.75h17.5c.966 0 1.75.784 1.75 1.75v12.5a1.75 1.75 0 0 1-1.75 1.75h-9.69l-3.573 3.573A1.458 1.458 0 0 1 5 21.043V18.5H3.25a1.75 1.75 0 0 1-1.75-1.75ZM3.25 4a.25.25 0 0 0-.25.25v12.5c0 .138.112.25.25.25h2.5a.75.75 0 0 1 .75.75v3.19l3.72-3.72a.749.749 0 0 1 .53-.22h10a.25.25 0 0 0 .25-.25V4.25a.25.25 0 0 0-.25-.25Z"/>"##,
        ),
        Icon::Issue => body.push_str(
            r##"<path d="M12 1c6.075 0 11 4.925 11 11s-4.925 11-11 11S1 18.075 1 12 5.925 1 12 1ZM2.5 12a9.5 9.5 0 0 0 9.5 9.5 9.5 9.5 0 0 0 9.5-9.5A9.5 9.5 0 0 0 12 2.5 9.5 9.5 0 0 0 2.5 12Zm9.5 2a2 2 0 1 1-.001-3.999A2 2 0 0 1 12 14Z"/>"##,
        ),
        Icon::Star => body.push_str(
            r##"<path d="M12 .25a.75.75 0 0 1 .673.418l3.058 6.197 6.839.994a.75.75 0 0 1 .415 1.279l-4.948 4.823 1.168 6.811a.751.751 0 0 1-1.088.791L12 18.347l-6.117 3.216a.75.75 0 0 1-1.088-.79l1.168-6.812-4.948-4.823a.75.75 0 0 1 .416-1.28l6.838-.993L11.328.668A.75.75 0 0 1 12 .25Zm0 2.445L9.44 7.882a.75.75 0 0 1-.565.41l-5.725.832 4.143 4.038a.748.748 0 0 1 .215.664l-.978 5.702 5.121-2.692a.75.75 0 0 1 .698 0l5.12 2.692-.977-5.702a.748.748 0 0 1 .215-.664l4.143-4.038-5.725-.831a.75.75 0 0 1-.565-.41L12 2.694Z"/>"##,
        ),
        Icon::Repository => body.push_str(
            r##"<path d="M3 2.75A2.75 2.75 0 0 1 5.75 0h14.5a.75.75 0 0 1 .75.75v20.5a.75.75 0 0 1-.75.75h-6a.75.75 0 0 1 0-1.5h5.25v-4H6A1.5 1.5 0 0 0 4.5 18v.75c0 .716.43 1.334 1.05 1.605a.75.75 0 0 1-.6 1.374A3.251 3.251 0 0 1 3 18.75ZM19.5 1.5H5.75c-.69 0-1.25.56-1.25 1.25v12.651A2.989 2.989 0 0 1 6 15h13.5Z"/><path d="M7 18.25a.25.25 0 0 1 .25-.25h5a.25.25 0 0 1 .25.25v5.01a.25.25 0 0 1-.397.201l-2.206-1.604a.25.25 0 0 0-.294 0L7.397 23.46a.25.25 0 0 1-.397-.2v-5.01Z"/>"##,
        ),
        Icon::Followers => body.push_str(
            r##"<path d="M3.5 8a5.5 5.5 0 1 1 8.596 4.547 9.005 9.005 0 0 1 5.9 8.18.751.751 0 0 1-1.5.045 7.5 7.5 0 0 0-14.993 0 .75.75 0 0 1-1.499-.044 9.005 9.005 0 0 1 5.9-8.181A5.496 5.496 0 0 1 3.5 8ZM9 4a4 4 0 1 0 0 8 4 4 0 0 0 0-8Zm8.29 4c-.148 0-.292.01-.434.03a.75.75 0 1 1-.212-1.484 4.53 4.53 0 0 1 3.38 8.097 6.69 6.69 0 0 1 3.956 6.107.75.75 0 0 1-1.5 0 5.193 5.193 0 0 0-3.696-4.972l-.534-.16v-1.676l.41-.209A3.03 3.03 0 0 0 17.29 8Z"/>"##,
        ),
    }
    body.push_str("</svg></g>");
}

impl Icon {
    const fn key(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::PullRequest => "pull-request",
            Self::Review => "review",
            Self::Issue => "issue",
            Self::Star => "star",
            Self::Repository => "repository",
            Self::Followers => "followers",
        }
    }

    const fn color(self) -> &'static str {
        match self {
            Self::Commit => "#58A6FF",
            Self::PullRequest => "#3FB950",
            Self::Review => "#A371F7",
            Self::Issue => "#F85149",
            Self::Star => "#E3B341",
            Self::Repository => "#39C5CF",
            Self::Followers => "#DB61A2",
        }
    }
}

fn push_open(body: &mut String) {
    let _ = write!(
        body,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" viewBox="0 0 {WIDTH} {HEIGHT}" role="img" aria-label="GitHub statistics">"##,
    );
}

fn push_defs_and_style(body: &mut String) {
    body.push_str(
        r##"<defs>
<linearGradient id="stats-bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#080d19"/><stop offset=".48" stop-color="#0d1428"/><stop offset="1" stop-color="#080d19"/></linearGradient>
<linearGradient id="rank-gradient" x1="1" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#BF91F3"/><stop offset=".48" stop-color="#927BFF"/><stop offset="1" stop-color="#58A6FF"/></linearGradient>
<linearGradient id="primary-wave-fill" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#3978ff" stop-opacity=".14"/><stop offset="1" stop-color="#3978ff" stop-opacity=".015"/></linearGradient>
<clipPath id="primary-clip"><rect x="28" y="84" width="244" height="180" rx="18"/></clipPath>
<filter id="soft-glow" x="-80%" y="-80%" width="260%" height="260%"><feGaussianBlur stdDeviation="2.5" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>
<filter id="wave-glow" x="-20%" y="-180%" width="140%" height="460%"><feGaussianBlur stdDeviation="1.4" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>
</defs><style>
.title{font:750 27px 'Segoe UI',Ubuntu,'Helvetica Neue',Arial,sans-serif;fill:#f1f5ff;letter-spacing:.1px}.rule{stroke:#6d82bd;stroke-opacity:.34}.primary-card,.metric-card,.rank-card,.empty-card{fill:#0f1830;stroke:#5473c7;stroke-opacity:.55;stroke-width:1.1}.primary-card{fill:#0d1730}.metric-card{fill:#10192f}.rank-card{fill:#0f172d}.primary-label{font:750 12px 'Segoe UI',Ubuntu,Arial,sans-serif;fill:#aeb9df;letter-spacing:1px}.primary-value{font:800 60px 'Segoe UI',Ubuntu,Arial,sans-serif;fill:#f5f7ff;letter-spacing:-1.8px}.metric-label{font:650 10.7px 'Segoe UI',Ubuntu,Arial,sans-serif;fill:#aab5d7}.metric-value{font:800 23px 'Segoe UI',Ubuntu,Arial,sans-serif;fill:#f0f4ff}.stat-icon{shape-rendering:geometricPrecision}.rank-rim{fill:none;stroke:#27324f;stroke-width:9}.rank-ring{fill:none;stroke:url(#rank-gradient);stroke-width:9;stroke-linecap:round;filter:url(#soft-glow)}.rank-level{font:800 35px 'Segoe UI',Ubuntu,Arial,sans-serif;fill:#f5f7ff}.rank-percent{font:800 14px 'Segoe UI',Ubuntu,Arial,sans-serif;fill:#c08ff8}.rank-label{font:700 9.5px 'Segoe UI',Ubuntu,Arial,sans-serif;fill:#8791b3;letter-spacing:1.3px}.stale{font:700 9.5px 'Segoe UI',Ubuntu,Arial,sans-serif;fill:#a07ad9;letter-spacing:1px}.empty-title{font:650 18px 'Segoe UI',Ubuntu,Arial,sans-serif;fill:#dfe7fb}.empty-copy{font:500 12px 'Segoe UI',Ubuntu,Arial,sans-serif;fill:#8792b2}.primary-wave-fill{fill:url(#primary-wave-fill)}.primary-wave-secondary{fill:none;stroke:#5c79df;stroke-opacity:.22;stroke-width:.85}.primary-wave{fill:none;stroke:#3d73e8;stroke-opacity:.74;stroke-width:1.25;filter:url(#wave-glow)}.canvas-wave{fill:none;stroke:#2947a1;stroke-opacity:.24;stroke-width:1}@keyframes enter{from{opacity:0;transform:translateY(3px)}to{opacity:1;transform:translateY(0)}}.metric-enter{opacity:0;animation:enter .26s ease-out forwards}@media (prefers-reduced-motion:reduce){.metric-enter{opacity:1;animation:none}}
</style>"##,
    );
}

fn format_stat_number(value: u64) -> String {
    value.to_string()
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

    use super::{format_stat_number, render};
    use crate::github::stats::{
        ContributionSummary, GitHubSnapshot, OverallStats, RankSnapshot, RateLimitSnapshot,
    };

    #[test]
    fn stats_card_matches_polished_layout_and_real_snapshot_metrics() {
        let card = render(Some(&snapshot()), 100, Duration::from_secs(60));
        assert!(card.body.contains("GitHub Statistics"));
        assert!(card.body.contains("data-stat-icon=\"commit\""));
        assert!(card.body.contains("data-stat-icon=\"pull-request\""));
        assert!(card.body.contains("data-stat-icon=\"review\""));
        assert!(card.body.contains("data-stat-icon=\"issue\""));
        assert!(card.body.contains("data-stat-icon=\"star\""));
        assert!(card.body.contains("data-stat-icon=\"repository\""));
        assert!(card.body.contains("data-stat-icon=\"followers\""));
        assert!(card.body.contains("data-icon-color=\"#58A6FF\""));
        assert!(card.body.contains("data-icon-color=\"#3FB950\""));
        assert!(card.body.contains("data-icon-color=\"#F85149\""));
        assert!(card.body.contains("data-icon-color=\"#E3B341\""));
        assert!(card.body.contains(">2345</text>"));
        assert!(!card.body.contains(">2,345</text>"));
        assert!(card.body.contains(">42</text>"));
        assert!(card.body.contains(">18</text>"));
        assert!(card.body.contains(">31</text>"));
        assert!(card.body.contains(">19</text>"));
        assert!(card.body.contains(">11</text>"));
        assert!(card.body.contains(">17</text>"));
        assert!(card.body.contains(">A</text>"));
        assert!(card.body.contains("TOP 22.5%"));
        assert!(card.body.contains("data-rank-progress=\"77.5\""));
        assert!(card.body.contains("pathLength=\"100\""));
        assert!(card.body.contains("stroke-dasharray=\"77.5 22.5\""));
        assert!(card.body.contains("class=\"primary-wave-fill\""));
        assert!(card.body.contains("class=\"primary-wave-secondary\""));
        assert!(card.body.contains("class=\"primary-wave\""));
        assert!(!card.body.contains("fill-opacity=\".07\""));
        assert!(
            !card
                .body
                .contains("stroke-opacity=\".74\" stroke-width=\"1.1\"/><svg")
        );
        assert!(!card.body.contains("Account output · live GitHub collector"));
        assert!(!card.body.contains("Across accessible repositories"));
        assert!(!card.body.contains("public repositories indexed"));
        assert!(!card.body.contains("vs. last year"));
    }

    #[test]
    fn unavailable_stats_card_fails_closed() {
        let card = render(None, 100, Duration::from_secs(60));
        assert!(card.body.contains("temporarily unavailable"));
        assert!(!card.body.contains("data-stats-primary=\"commits\""));
        assert!(card.etag.contains("unavailable"));
    }

    #[test]
    fn stale_snapshot_is_marked_without_changing_metrics() {
        let card = render(Some(&snapshot()), 100_000, Duration::from_secs(1));
        assert!(card.body.contains("LAST KNOWN"));
        assert!(card.etag.contains("stale"));
        assert!(card.body.contains(">2345</text>"));
    }

    #[test]
    fn stat_numbers_do_not_use_thousands_separators() {
        assert_eq!(format_stat_number(0), "0");
        assert_eq!(format_stat_number(999), "999");
        assert_eq!(format_stat_number(1_000), "1000");
        assert_eq!(format_stat_number(1_234_567), "1234567");
    }

    fn snapshot() -> GitHubSnapshot {
        GitHubSnapshot {
            revision: 7,
            collected_at_unix_ms: 100,
            login: "Herzchens".to_owned(),
            url: "https://github.com/Herzchens".to_owned(),
            contributions: ContributionSummary {
                total: 4_396,
                commits: 200,
                issues: 10,
                pull_requests: 20,
                reviews: 5,
                active_days: 80,
                current_streak_days: 140,
                longest_streak_days: 140,
                calendar_start: Some("2024-04-12".to_owned()),
                calendar_end: Some("2026-09-15".to_owned()),
                today_contributions: Some(2),
                current_streak_start: Some("2026-04-29".to_owned()),
                current_streak_end: Some("2026-09-15".to_owned()),
                longest_streak_start: Some("2026-04-29".to_owned()),
                longest_streak_end: Some("2026-09-15".to_owned()),
                restricted_contributions: 0,
                includes_restricted_contributions: false,
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
            languages: Vec::new(),
            projects: Vec::new(),
            repository_count: 8,
            source_truncated: false,
            rate_limit: RateLimitSnapshot {
                cost: 1,
                remaining: 4_999,
                reset_at: "2026-09-15T12:00:00Z".to_owned(),
            },
        }
    }
}
