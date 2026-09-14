use std::{fs, path::PathBuf, time::Duration};

use crate::github::{
    cards::{GitHubCardKind, render_card},
    stats::{
        ContributionSummary, GitHubSnapshot, LanguageShare, OverallStats, RankSnapshot,
        RateLimitSnapshot,
    },
};

const STALE_AFTER: Duration = Duration::from_secs(60);

#[test]
#[ignore = "developer-only streak SVG preview generator"]
fn generate_streak_state_previews() {
    let output_dir = std::env::var_os("STREAK_PREVIEW_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("profile-streak-preview"));
    fs::create_dir_all(&output_dir).expect("create streak preview output directory");

    let mut lit = preview_snapshot();
    lit.contributions.today_contributions = Some(2);
    write_preview(&output_dir, "github-streak-lit.svg", &lit, 100, "campfire-lit", "lit");

    let mut out = preview_snapshot();
    out.contributions.today_contributions = Some(0);
    write_preview(&output_dir, "github-streak-out.svg", &out, 100, "campfire-out", "out");

    let unknown = preview_snapshot();
    write_preview(
        &output_dir,
        "github-streak-unknown.svg",
        &unknown,
        61_000,
        "campfire-unknown",
        "out",
    );

    println!("streak previews written to {}", output_dir.display());
}

fn write_preview(
    output_dir: &std::path::Path,
    filename: &str,
    snapshot: &GitHubSnapshot,
    now_unix_ms: u64,
    expected_campfire_state: &str,
    expected_mascot_state: &str,
) {
    let card = render_card(
        GitHubCardKind::Streak,
        Some(snapshot),
        now_unix_ms,
        STALE_AFTER,
    );
    let campfire_marker = format!("id=\"{expected_campfire_state}\"");
    let mascot_marker = format!("data-mascot-state=\"{expected_mascot_state}\"");
    assert!(card.body().contains(&campfire_marker));
    assert!(card.body().contains(&mascot_marker));
    fs::write(output_dir.join(filename), card.body()).expect("write streak preview SVG");
}

fn preview_snapshot() -> GitHubSnapshot {
    GitHubSnapshot {
        revision: 3,
        collected_at_unix_ms: 100,
        login: "Herzchens".to_owned(),
        url: "https://github.com/Herzchens".to_owned(),
        contributions: ContributionSummary {
            total: 4_355,
            commits: 3_812,
            issues: 74,
            pull_requests: 228,
            reviews: 196,
            active_days: 349,
            current_streak_days: 139,
            longest_streak_days: 139,
            calendar_start: Some("2025-09-14".to_owned()),
            calendar_end: Some("2026-09-14".to_owned()),
            today_contributions: Some(2),
            current_streak_start: Some("2026-04-29".to_owned()),
            current_streak_end: Some("2026-09-14".to_owned()),
            longest_streak_start: Some("2026-04-29".to_owned()),
            longest_streak_end: Some("2026-09-14".to_owned()),
            restricted_contributions: 0,
            includes_restricted_contributions: true,
        },
        overall: OverallStats {
            total_commits: 3_812,
            total_pull_requests: 228,
            total_reviews: 196,
            total_issues: 74,
            total_stars: 139,
            contributed_to: 44,
            followers: 17,
            rank: RankSnapshot {
                level: "A+".to_owned(),
                percentile: 8.0,
            },
        },
        languages: vec![LanguageShare {
            name: "Rust".to_owned(),
            color: Some("#dea584".to_owned()),
            bytes: 1_000,
            repository_count: 4,
            score: 63.0,
            percent: 63.0,
        }],
        projects: Vec::new(),
        repository_count: 16,
        source_truncated: false,
        rate_limit: RateLimitSnapshot {
            cost: 1,
            remaining: 4_999,
            reset_at: "2026-09-14T17:00:00Z".to_owned(),
        },
    }
}
