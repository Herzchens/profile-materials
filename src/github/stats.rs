use std::{cmp::Ordering, collections::HashMap, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};

use super::client::{RawContributionDay, RawContributions, RawProfile, RawRepository, RawUser};

const MAX_FEATURED_PROJECTS: usize = 3;
const MAX_LANGUAGE_ROWS: usize = 20;
const LANGUAGE_SIZE_WEIGHT: f64 = 0.5;
const LANGUAGE_COUNT_WEIGHT: f64 = 0.5;
const SVG_REVISION: u8 = 2;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GitHubSnapshot {
    pub revision: u64,
    pub collected_at_unix_ms: u64,
    pub login: String,
    pub url: String,
    pub contributions: ContributionSummary,
    pub overall: OverallStats,
    pub languages: Vec<LanguageShare>,
    pub projects: Vec<ProjectMetadata>,
    pub repository_count: u64,
    pub source_truncated: bool,
    pub rate_limit: RateLimitSnapshot,
}

impl GitHubSnapshot {
    pub fn semantic_eq(&self, other: &Self) -> bool {
        self.login == other.login
            && self.url == other.url
            && self.contributions == other.contributions
            && self.overall == other.overall
            && self.languages == other.languages
            && self.projects == other.projects
            && self.repository_count == other.repository_count
            && self.source_truncated == other.source_truncated
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContributionSummary {
    pub total: u64,
    pub commits: u64,
    pub issues: u64,
    pub pull_requests: u64,
    pub reviews: u64,
    pub active_days: u64,
    pub current_streak_days: u64,
    pub longest_streak_days: u64,
    pub restricted_contributions: u64,
    pub includes_restricted_contributions: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OverallStats {
    pub total_commits: u64,
    pub total_pull_requests: u64,
    pub total_reviews: u64,
    pub total_issues: u64,
    pub total_stars: u64,
    pub contributed_to: u64,
    pub followers: u64,
    pub rank: RankSnapshot,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RankSnapshot {
    pub level: String,
    pub percentile: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LanguageShare {
    pub name: String,
    pub color: Option<String>,
    pub bytes: u64,
    pub repository_count: u64,
    pub score: f64,
    pub percent: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PublicLanguageShare {
    pub name: String,
    pub color: Option<String>,
    pub percent: f64,
}

impl From<&LanguageShare> for PublicLanguageShare {
    fn from(language: &LanguageShare) -> Self {
        Self {
            name: language.name.clone(),
            color: language.color.clone(),
            percent: language.percent,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProjectMetadata {
    pub name: String,
    pub name_with_owner: String,
    pub url: String,
    pub description: Option<String>,
    pub stars: u64,
    pub forks: u64,
    pub pushed_at: Option<String>,
    pub primary_language: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RateLimitSnapshot {
    pub cost: u64,
    pub remaining: u64,
    pub reset_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PublicGitHubSnapshot {
    pub revision: u64,
    pub collected_at_unix_ms: u64,
    pub login: String,
    pub url: String,
    pub contributions: ContributionSummary,
    pub overall: OverallStats,
    pub languages: Vec<PublicLanguageShare>,
    pub projects: Vec<ProjectMetadata>,
    pub repository_count: u64,
    pub source_truncated: bool,
}

impl From<&GitHubSnapshot> for PublicGitHubSnapshot {
    fn from(snapshot: &GitHubSnapshot) -> Self {
        Self {
            revision: snapshot.revision,
            collected_at_unix_ms: snapshot.collected_at_unix_ms,
            login: snapshot.login.clone(),
            url: snapshot.url.clone(),
            contributions: snapshot.contributions.clone(),
            overall: snapshot.overall.clone(),
            languages: snapshot
                .languages
                .iter()
                .map(PublicLanguageShare::from)
                .collect(),
            projects: snapshot.projects.clone(),
            repository_count: snapshot.repository_count,
            source_truncated: snapshot.source_truncated,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct GitHubPublicResponse {
    pub availability: &'static str,
    pub stale: bool,
    pub snapshot: Option<PublicGitHubSnapshot>,
}

impl GitHubPublicResponse {
    pub fn new(
        snapshot: Option<Arc<GitHubSnapshot>>,
        now_unix_ms: u64,
        stale_after: Duration,
    ) -> Self {
        let stale = snapshot
            .as_ref()
            .is_some_and(|snapshot| is_stale(snapshot, now_unix_ms, stale_after));
        Self {
            availability: if snapshot.is_some() {
                "ready"
            } else {
                "unavailable"
            },
            stale,
            snapshot: snapshot.as_deref().map(PublicGitHubSnapshot::from),
        }
    }
}

pub struct GitHubSvgDocument {
    body: String,
    etag: String,
    revision: u64,
}

impl GitHubSvgDocument {
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

pub fn build_snapshot(
    raw: RawProfile,
    featured_repositories: &[String],
    collected_at_unix_ms: u64,
) -> GitHubSnapshot {
    let user = raw.user;
    let source_truncated = user.repositories.page_info.has_next_page;
    let repositories = user
        .repositories
        .nodes
        .iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    let repository_count = repositories
        .iter()
        .filter(|repository| !repository.is_private)
        .count() as u64;
    let days = user
        .contributions
        .calendar
        .weeks
        .iter()
        .flat_map(|week| week.contribution_days.iter().cloned())
        .collect::<Vec<_>>();
    let contributions = build_contribution_summary(&days, &user.contributions);
    let overall = build_overall_stats(&user, &repositories, raw.total_commits_all);
    let languages = build_language_summary(&repositories);
    let projects = select_projects(&repositories, featured_repositories);

    GitHubSnapshot {
        revision: 0,
        collected_at_unix_ms,
        login: user.login,
        url: user.url,
        contributions,
        overall,
        languages,
        projects,
        repository_count,
        source_truncated,
        rate_limit: RateLimitSnapshot {
            cost: raw.rate_limit.cost,
            remaining: raw.rate_limit.remaining,
            reset_at: raw.rate_limit.reset_at,
        },
    }
}

pub fn render_svg(
    snapshot: Option<&GitHubSnapshot>,
    now_unix_ms: u64,
    stale_after: Duration,
) -> GitHubSvgDocument {
    match snapshot {
        Some(snapshot) => {
            let stale = is_stale(snapshot, now_unix_ms, stale_after);
            GitHubSvgDocument {
                body: render_ready_svg(snapshot, stale),
                etag: format!(
                    "\"profile-github-v{SVG_REVISION}-{}-{}\"",
                    snapshot.revision,
                    if stale { "stale" } else { "fresh" }
                ),
                revision: snapshot.revision,
            }
        }
        None => GitHubSvgDocument {
            body: render_unavailable_svg(),
            etag: format!("\"profile-github-v{SVG_REVISION}-unavailable\""),
            revision: 0,
        },
    }
}

pub fn is_stale(snapshot: &GitHubSnapshot, now_unix_ms: u64, stale_after: Duration) -> bool {
    now_unix_ms.saturating_sub(snapshot.collected_at_unix_ms)
        >= stale_after.as_millis().min(u128::from(u64::MAX)) as u64
}

fn build_contribution_summary(
    days: &[RawContributionDay],
    raw: &RawContributions,
) -> ContributionSummary {
    let mut parsed_days = days
        .iter()
        .filter_map(|day| day_number(&day.date).map(|ordinal| (ordinal, day.contribution_count)))
        .collect::<Vec<_>>();
    parsed_days.sort_unstable_by_key(|(ordinal, _)| *ordinal);
    parsed_days.dedup_by_key(|(ordinal, _)| *ordinal);

    let active_days = parsed_days.iter().filter(|(_, count)| *count > 0).count() as u64;

    let mut longest = 0_u64;
    let mut running = 0_u64;
    let mut previous_active_ordinal = None;
    for (ordinal, count) in &parsed_days {
        if *count == 0 {
            running = 0;
            previous_active_ordinal = None;
            continue;
        }

        running = if previous_active_ordinal.is_some_and(|previous| *ordinal == previous + 1) {
            running + 1
        } else {
            1
        };
        longest = longest.max(running);
        previous_active_ordinal = Some(*ordinal);
    }

    ContributionSummary {
        total: raw.calendar.total_contributions,
        commits: raw.total_commit_contributions,
        issues: raw.total_issue_contributions,
        pull_requests: raw.total_pull_request_contributions,
        reviews: raw.total_pull_request_review_contributions,
        active_days,
        current_streak_days: current_streak(&parsed_days),
        longest_streak_days: longest,
        restricted_contributions: raw.restricted_contributions_count,
        includes_restricted_contributions: raw.has_any_restricted_contributions,
    }
}

fn current_streak(days: &[(i64, u64)]) -> u64 {
    let Some((latest_ordinal, latest_count)) = days.last().copied() else {
        return 0;
    };

    let mut expected = if latest_count == 0 {
        latest_ordinal - 1
    } else {
        latest_ordinal
    };
    let mut streak = 0_u64;

    for (ordinal, count) in days.iter().rev() {
        if *ordinal > expected {
            continue;
        }
        if *ordinal != expected || *count == 0 {
            break;
        }
        streak += 1;
        expected -= 1;
    }

    streak
}

fn build_overall_stats(
    user: &RawUser,
    repositories: &[RawRepository],
    total_commits_all: u64,
) -> OverallStats {
    let total_stars = repositories
        .iter()
        .map(|repository| repository.stargazer_count)
        .sum::<u64>();
    let total_issues = user
        .open_issues
        .total_count
        .saturating_add(user.closed_issues.total_count);
    let rank = calculate_rank(
        total_commits_all,
        user.pull_requests.total_count,
        total_issues,
        user.contributions.total_pull_request_review_contributions,
        total_stars,
        user.followers.total_count,
    );

    OverallStats {
        total_commits: total_commits_all,
        total_pull_requests: user.pull_requests.total_count,
        total_reviews: user.contributions.total_pull_request_review_contributions,
        total_issues,
        total_stars,
        contributed_to: user.repositories_contributed_to.total_count,
        followers: user.followers.total_count,
        rank,
    }
}

fn calculate_rank(
    commits: u64,
    pull_requests: u64,
    issues: u64,
    reviews: u64,
    stars: u64,
    followers: u64,
) -> RankSnapshot {
    const COMMITS_MEDIAN: f64 = 1_000.0;
    const PRS_MEDIAN: f64 = 50.0;
    const ISSUES_MEDIAN: f64 = 25.0;
    const REVIEWS_MEDIAN: f64 = 2.0;
    const STARS_MEDIAN: f64 = 50.0;
    const FOLLOWERS_MEDIAN: f64 = 10.0;
    const TOTAL_WEIGHT: f64 = 12.0;
    const THRESHOLDS: [f64; 9] = [1.0, 12.5, 25.0, 37.5, 50.0, 62.5, 75.0, 87.5, 100.0];
    const LEVELS: [&str; 9] = ["S", "A+", "A", "A-", "B+", "B", "B-", "C+", "C"];

    let exponential_cdf = |value: f64| 1.0 - 2_f64.powf(-value);
    let log_normal_cdf = |value: f64| value / (1.0 + value);

    let rank = 1.0
        - (2.0 * exponential_cdf(commits as f64 / COMMITS_MEDIAN)
            + 3.0 * exponential_cdf(pull_requests as f64 / PRS_MEDIAN)
            + exponential_cdf(issues as f64 / ISSUES_MEDIAN)
            + exponential_cdf(reviews as f64 / REVIEWS_MEDIAN)
            + 4.0 * log_normal_cdf(stars as f64 / STARS_MEDIAN)
            + log_normal_cdf(followers as f64 / FOLLOWERS_MEDIAN))
            / TOTAL_WEIGHT;
    let percentile = rank * 100.0;
    let level = THRESHOLDS
        .iter()
        .position(|threshold| percentile <= *threshold)
        .map(|index| LEVELS[index])
        .unwrap_or("C");

    RankSnapshot {
        level: level.to_owned(),
        percentile,
    }
}

fn build_language_summary(repositories: &[RawRepository]) -> Vec<LanguageShare> {
    let mut totals = HashMap::<String, (u64, u64, Option<String>)>::new();

    for repository in repositories.iter().filter(|repository| !repository.is_fork) {
        for edge in &repository.languages.edges {
            let entry =
                totals
                    .entry(edge.node.name.clone())
                    .or_insert((0, 0, edge.node.color.clone()));
            entry.0 = entry.0.saturating_add(edge.size);
            entry.1 = entry.1.saturating_add(1);
            if entry.2.is_none() {
                entry.2 = edge.node.color.clone();
            }
        }
    }

    let mut output = totals
        .into_iter()
        .map(|(name, (bytes, repository_count, color))| {
            let score = (bytes as f64).powf(LANGUAGE_SIZE_WEIGHT)
                * (repository_count as f64).powf(LANGUAGE_COUNT_WEIGHT);
            LanguageShare {
                name,
                color,
                bytes,
                repository_count,
                score,
                percent: 0.0,
            }
        })
        .collect::<Vec<_>>();

    output.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    output.truncate(MAX_LANGUAGE_ROWS);

    let total_score = output.iter().map(|language| language.score).sum::<f64>();
    for language in &mut output {
        language.percent = if total_score == 0.0 {
            0.0
        } else {
            ((language.score / total_score) * 10_000.0).round() / 100.0
        };
    }

    output
}

fn select_projects(
    repositories: &[RawRepository],
    featured_repositories: &[String],
) -> Vec<ProjectMetadata> {
    let public_projects = repositories
        .iter()
        .filter(|repository| {
            !repository.is_private && !repository.is_fork && !repository.is_archived
        })
        .collect::<Vec<_>>();

    if featured_repositories.is_empty() {
        let mut candidates = public_projects;
        candidates.sort_by(|left, right| {
            right
                .stargazer_count
                .cmp(&left.stargazer_count)
                .then_with(|| compare_optional_desc(&left.pushed_at, &right.pushed_at))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        return candidates
            .into_iter()
            .take(MAX_FEATURED_PROJECTS)
            .map(project_metadata)
            .collect();
    }

    featured_repositories
        .iter()
        .filter_map(|requested| {
            public_projects.iter().find(|repository| {
                repository.name.eq_ignore_ascii_case(requested)
                    || repository.name_with_owner.eq_ignore_ascii_case(requested)
            })
        })
        .take(MAX_FEATURED_PROJECTS)
        .map(|repository| project_metadata(repository))
        .collect()
}

fn compare_optional_desc(left: &Option<String>, right: &Option<String>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.cmp(left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn project_metadata(repository: &RawRepository) -> ProjectMetadata {
    ProjectMetadata {
        name: repository.name.clone(),
        name_with_owner: repository.name_with_owner.clone(),
        url: repository.url.clone(),
        description: repository.description.clone(),
        stars: repository.stargazer_count,
        forks: repository.fork_count,
        pushed_at: repository.pushed_at.clone(),
        primary_language: repository
            .primary_language
            .as_ref()
            .map(|language| language.name.clone()),
    }
}

fn day_number(date: &str) -> Option<i64> {
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || day == 0 {
        return None;
    }
    let max_day = days_in_month(year, month)?;
    if day > max_day {
        return None;
    }

    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let month_index = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_index + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era)
}

fn days_in_month(year: i64, month: u32) -> Option<u32> {
    Some(match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => return None,
    })
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn render_ready_svg(snapshot: &GitHubSnapshot, stale: bool) -> String {
    let height = 250 + (snapshot.projects.len().min(MAX_FEATURED_PROJECTS) as u32 * 34);
    let mut body = String::with_capacity(6_000);
    push_svg_open(&mut body, 760, height, "GitHub stats");
    body.push_str(r##"<rect width="760" height="100%" rx="20" fill="#0b1020"/>"##);
    body.push_str(
        r##"<rect x="1" y="1" width="758" height="100%" rx="19" fill="none" stroke="#293659"/>"##,
    );
    body.push_str(r##"<text x="28" y="42" fill="#f4f7ff" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="23" font-weight="700">GitHub stats</text>"##);
    body.push_str(&format!(
        r##"<text x="28" y="68" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="14">@{} · {}{}</text>"##,
        escape_xml(&snapshot.login),
        snapshot.contributions.total,
        if stale { " contributions · last known" } else { " contributions" }
    ));
    body.push_str(&format!(
        r##"<text x="28" y="102" fill="#d7def2" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="15">commits {} · stars {} · PRs {} · issues {}</text>"##,
        snapshot.overall.total_commits,
        snapshot.overall.total_stars,
        snapshot.overall.total_pull_requests,
        snapshot.overall.total_issues
    ));
    body.push_str(&format!(
        r##"<text x="28" y="129" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="14">reviews {} · contributed to {} · followers {} · rank {}</text>"##,
        snapshot.overall.total_reviews,
        snapshot.overall.contributed_to,
        snapshot.overall.followers,
        escape_xml(&snapshot.overall.rank.level)
    ));
    body.push_str(&format!(
        r##"<text x="28" y="162" fill="#d7def2" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="15">streak {}d · longest {}d · active {}d</text>"##,
        snapshot.contributions.current_streak_days,
        snapshot.contributions.longest_streak_days,
        snapshot.contributions.active_days
    ));

    let language_line = if snapshot.languages.is_empty() {
        "languages unavailable".to_owned()
    } else {
        snapshot
            .languages
            .iter()
            .take(4)
            .map(|language| format!("{} {:.1}%", language.name, language.percent))
            .collect::<Vec<_>>()
            .join(" · ")
    };
    body.push_str(&format!(
        r##"<text x="28" y="196" fill="#d7def2" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="14">{}</text>"##,
        escape_xml(&truncate_chars(&language_line, 86))
    ));

    let mut y = 236;
    for project in snapshot.projects.iter().take(MAX_FEATURED_PROJECTS) {
        body.push_str(&format!(
            r##"<text x="28" y="{}" fill="#f4f7ff" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="15" font-weight="600">{}</text>"##,
            y,
            escape_xml(&truncate_chars(&project.name, 34))
        ));
        body.push_str(&format!(
            r##"<text x="310" y="{}" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="13">★ {} · forks {}{}</text>"##,
            y,
            project.stars,
            project.forks,
            project
                .primary_language
                .as_deref()
                .map(|language| format!(" · {}", escape_xml(language)))
                .unwrap_or_default()
        ));
        y += 34;
    }

    body.push_str("</svg>");
    body
}

fn render_unavailable_svg() -> String {
    let mut body = String::with_capacity(1_000);
    push_svg_open(&mut body, 760, 150, "GitHub stats unavailable");
    body.push_str(r##"<rect width="760" height="150" rx="20" fill="#0b1020"/>"##);
    body.push_str(
        r##"<rect x="1" y="1" width="758" height="148" rx="19" fill="none" stroke="#293659"/>"##,
    );
    body.push_str(r##"<text x="28" y="54" fill="#f4f7ff" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="23" font-weight="700">GitHub stats</text>"##);
    body.push_str(r##"<text x="28" y="93" fill="#9fb0d9" font-family="ui-monospace, SFMono-Regular, Consolas, monospace" font-size="15">Stats are temporarily unavailable.</text>"##);
    body.push_str("</svg>");
    body
}

fn push_svg_open(body: &mut String, width: u32, height: u32, label: &str) {
    body.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-label="{}">"#,
        escape_xml(label)
    ));
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

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut characters = value.chars();
    let prefix = characters.by_ref().take(max_chars).collect::<String>();
    if characters.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_contribution_summary, build_language_summary, calculate_rank, current_streak,
        day_number, select_projects,
    };
    use crate::github::client::{
        RawContributionCalendar, RawContributionDay, RawContributionWeek, RawContributions,
        RawLanguage, RawLanguageConnection, RawLanguageEdge, RawRepository,
    };

    #[test]
    fn streak_allows_today_to_be_empty() {
        let days = vec![
            (day_number("2026-09-09").unwrap(), 1),
            (day_number("2026-09-10").unwrap(), 2),
            (day_number("2026-09-11").unwrap(), 1),
            (day_number("2026-09-12").unwrap(), 0),
        ];
        assert_eq!(current_streak(&days), 3);
    }

    #[test]
    fn contribution_summary_tracks_longest_and_current_streaks() {
        let days = [
            RawContributionDay {
                contribution_count: 1,
                date: "2026-09-01".to_owned(),
            },
            RawContributionDay {
                contribution_count: 1,
                date: "2026-09-02".to_owned(),
            },
            RawContributionDay {
                contribution_count: 0,
                date: "2026-09-03".to_owned(),
            },
            RawContributionDay {
                contribution_count: 1,
                date: "2026-09-04".to_owned(),
            },
            RawContributionDay {
                contribution_count: 1,
                date: "2026-09-05".to_owned(),
            },
            RawContributionDay {
                contribution_count: 1,
                date: "2026-09-06".to_owned(),
            },
        ];
        let raw = RawContributions {
            calendar: RawContributionCalendar {
                total_contributions: 5,
                weeks: vec![RawContributionWeek {
                    contribution_days: days.to_vec(),
                }],
            },
            total_commit_contributions: 3,
            total_issue_contributions: 1,
            total_pull_request_contributions: 1,
            total_pull_request_review_contributions: 0,
            restricted_contributions_count: 0,
            has_any_restricted_contributions: false,
        };
        let summary = build_contribution_summary(&days, &raw);
        assert_eq!(summary.active_days, 5);
        assert_eq!(summary.current_streak_days, 3);
        assert_eq!(summary.longest_streak_days, 3);
    }

    #[test]
    fn language_policy_matches_half_size_half_repo_count_weighting() {
        let rust = repository(
            "rust",
            false,
            false,
            false,
            vec![("Rust", 400, Some("#dea584"))],
        );
        let go_a = repository(
            "go-a",
            true,
            false,
            false,
            vec![("Go", 100, Some("#00ADD8"))],
        );
        let go_b = repository(
            "go-b",
            true,
            false,
            false,
            vec![("Go", 100, Some("#00ADD8"))],
        );
        let languages = build_language_summary(&[rust, go_a, go_b]);
        assert_eq!(languages.len(), 2);

        let rust = languages
            .iter()
            .find(|language| language.name == "Rust")
            .unwrap();
        let go = languages
            .iter()
            .find(|language| language.name == "Go")
            .unwrap();

        assert!((rust.score - 20.0).abs() < 0.000_001);
        assert!((go.score - 20.0).abs() < 0.000_001);
        assert_eq!(go.repository_count, 2);
        assert!((rust.percent - 50.0).abs() < 0.01);
        assert!((go.percent - 50.0).abs() < 0.01);
    }

    #[test]
    fn language_policy_ignores_forks_but_keeps_private_repositories() {
        let private = repository(
            "private-go",
            true,
            false,
            false,
            vec![("Go", 200, Some("#00ADD8"))],
        );
        let fork = repository(
            "forked-rust",
            false,
            true,
            false,
            vec![("Rust", 10_000, Some("#dea584"))],
        );
        let languages = build_language_summary(&[private, fork]);
        assert_eq!(languages.len(), 1);
        assert_eq!(languages[0].name, "Go");
    }

    #[test]
    fn private_repositories_never_become_featured_projects() {
        let private = repository("secret", true, false, false, Vec::new());
        let public = repository("public", false, false, false, Vec::new());
        let selected = select_projects(
            &[private, public],
            &["secret".to_owned(), "public".to_owned()],
        );
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "public");
    }

    #[test]
    fn rank_uses_include_all_commits_median() {
        let rank = calculate_rank(1_000, 50, 25, 2, 50, 10);
        assert!(rank.percentile > 0.0);
        assert!(!rank.level.is_empty());
    }

    #[test]
    fn svg_text_is_xml_escaped() {
        assert_eq!(
            super::escape_xml("<tag>&\"'"),
            "&lt;tag&gt;&amp;&quot;&apos;"
        );
    }

    fn repository(
        name: &str,
        is_private: bool,
        is_fork: bool,
        is_archived: bool,
        languages: Vec<(&str, u64, Option<&str>)>,
    ) -> RawRepository {
        RawRepository {
            name: name.to_owned(),
            name_with_owner: format!("Herzchens/{name}"),
            url: format!("https://github.com/Herzchens/{name}"),
            description: None,
            is_private,
            is_fork,
            is_archived,
            stargazer_count: 0,
            fork_count: 0,
            pushed_at: Some("2026-09-12T00:00:00Z".to_owned()),
            primary_language: None,
            languages: RawLanguageConnection {
                edges: languages
                    .into_iter()
                    .map(|(name, size, color)| RawLanguageEdge {
                        size,
                        node: RawLanguage {
                            name: name.to_owned(),
                            color: color.map(str::to_owned),
                        },
                    })
                    .collect(),
            },
        }
    }
}
