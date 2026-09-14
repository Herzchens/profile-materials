use std::{
    error::Error,
    fmt,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use reqwest::{Client, StatusCode, header};
use serde::{Deserialize, Serialize};

const GRAPHQL_ENDPOINT: &str = "https://api.github.com/graphql";
const COMMIT_SEARCH_ENDPOINT: &str = "https://api.github.com/search/commits";
const HISTORICAL_CONTRIBUTION_REFRESH: Duration = Duration::from_secs(6 * 60 * 60);
const PROFILE_QUERY: &str = r#"
query ProfileStats($login: String!) {
  user(login: $login) {
    login
    url
    contributionsCollection {
      contributionCalendar {
        totalContributions
        weeks {
          contributionDays {
            contributionCount
            date
          }
        }
      }
      totalCommitContributions
      totalIssueContributions
      totalPullRequestContributions
      totalPullRequestReviewContributions
      restrictedContributionsCount
      hasAnyRestrictedContributions
    }
    contributionHistory: contributionsCollection {
      contributionYears
    }
    followers {
      totalCount
    }
    repositoriesContributedTo(
      first: 1
      contributionTypes: [COMMIT, ISSUE, PULL_REQUEST, REPOSITORY]
    ) {
      totalCount
    }
    pullRequests(first: 1) {
      totalCount
    }
    openIssues: issues(states: OPEN) {
      totalCount
    }
    closedIssues: issues(states: CLOSED) {
      totalCount
    }
    repositories(
      first: 100
      ownerAffiliations: OWNER
      orderBy: {field: STARGAZERS, direction: DESC}
    ) {
      pageInfo {
        hasNextPage
      }
      nodes {
        name
        nameWithOwner
        url
        description
        isPrivate
        isFork
        isArchived
        stargazerCount
        forkCount
        pushedAt
        primaryLanguage {
          name
          color
        }
        languages(first: 10, orderBy: {field: SIZE, direction: DESC}) {
          edges {
            size
            node {
              name
              color
            }
          }
        }
      }
    }
  }
  rateLimit {
    cost
    remaining
    resetAt
  }
}
"#;
const CONTRIBUTION_YEAR_QUERY: &str = r#"
query ContributionYear($login: String!, $from: DateTime!, $to: DateTime!) {
  user(login: $login) {
    contributionsCollection(from: $from, to: $to) {
      contributionCalendar {
        totalContributions
        weeks {
          contributionDays {
            contributionCount
            date
          }
        }
      }
    }
  }
}
"#;

pub struct GitHubClient {
    client: Client,
    token: String,
    contribution_history: Mutex<ContributionHistoryCache>,
}

#[derive(Default)]
struct ContributionHistoryCache {
    historical_years: Vec<i32>,
    historical_days: Vec<RawContributionDay>,
    refreshed_at: Option<Instant>,
}

impl GitHubClient {
    pub fn new(token: String) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .timeout(Duration::from_secs(10))
            .user_agent("profile-materials/0.1 github-stats")
            .build()?;
        Ok(Self {
            client,
            token,
            contribution_history: Mutex::new(ContributionHistoryCache::default()),
        })
    }

    pub async fn fetch_profile(&self, login: &str) -> Result<RawProfile, ClientError> {
        let request_body = serde_json::to_vec(&GraphQlRequest {
            query: PROFILE_QUERY,
            variables: Variables { login },
        })
        .map_err(ClientError::Json)?;
        let response = self
            .client
            .post(GRAPHQL_ENDPOINT)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-github-api-version", "2022-11-28")
            .bearer_auth(&self.token)
            .body(request_body)
            .send()
            .await
            .map_err(ClientError::Request)?;

        let status = response.status();
        let rate_limit_delay = rate_limit_delay(response.headers());
        let body = response.text().await.map_err(ClientError::Request)?;
        if !status.is_success() {
            if let Some(delay) = rate_limit_delay {
                return Err(ClientError::RateLimited(delay));
            }
            return Err(ClientError::Http {
                status,
                body: truncate_for_error(&body),
            });
        }

        let envelope: GraphQlEnvelope = serde_json::from_str(&body).map_err(ClientError::Json)?;
        if let Some(errors) = envelope.errors
            && !errors.is_empty()
        {
            return Err(graphql_errors(errors, rate_limit_delay));
        }

        let data = envelope.data.ok_or(ClientError::MissingData)?;
        let mut user = data
            .user
            .ok_or_else(|| ClientError::UserNotFound(login.to_owned()))?;

        let current_end = latest_contribution_date(&user.contributions.calendar)
            .ok_or(ClientError::InvalidContributionCalendar)?
            .to_owned();
        let current_year =
            contribution_year(&current_end).ok_or(ClientError::InvalidContributionCalendar)?;
        let mut historical_years = user
            .contribution_history
            .contribution_years
            .iter()
            .copied()
            .filter(|year| *year < current_year)
            .collect::<Vec<_>>();
        historical_years.sort_unstable();
        historical_years.dedup();

        let historical_days = self
            .historical_contribution_days(login, &historical_years)
            .await?;
        let current_days = user
            .contributions
            .calendar
            .weeks
            .iter()
            .flat_map(|week| week.contribution_days.iter())
            .filter(|day| contribution_year(&day.date) == Some(current_year))
            .cloned()
            .collect::<Vec<_>>();
        if current_days.is_empty() {
            return Err(ClientError::InvalidContributionCalendar);
        }
        user.contributions.calendar = build_all_time_calendar(historical_days, current_days);

        let total_commits_all = self.fetch_total_commits(login).await?;

        Ok(RawProfile {
            user,
            rate_limit: data.rate_limit,
            total_commits_all,
        })
    }

    async fn historical_contribution_days(
        &self,
        login: &str,
        historical_years: &[i32],
    ) -> Result<Vec<RawContributionDay>, ClientError> {
        {
            let cache = match self.contribution_history.lock() {
                Ok(cache) => cache,
                Err(poisoned) => {
                    tracing::warn!(
                        "GitHub contribution-history cache lock was poisoned; recovering"
                    );
                    poisoned.into_inner()
                }
            };
            let cache_is_fresh = cache.historical_years == historical_years
                && cache.refreshed_at.is_some_and(|refreshed_at| {
                    refreshed_at.elapsed() < HISTORICAL_CONTRIBUTION_REFRESH
                });
            if cache_is_fresh {
                return Ok(cache.historical_days.clone());
            }
        }

        let mut historical_days = Vec::new();
        for year in historical_years {
            historical_days.extend(self.fetch_contribution_year(login, *year).await?);
        }

        let mut cache = match self.contribution_history.lock() {
            Ok(cache) => cache,
            Err(poisoned) => {
                tracing::warn!("GitHub contribution-history cache lock was poisoned; recovering");
                poisoned.into_inner()
            }
        };
        cache.historical_years = historical_years.to_vec();
        cache.historical_days = historical_days.clone();
        cache.refreshed_at = Some(Instant::now());
        Ok(historical_days)
    }

    async fn fetch_contribution_year(
        &self,
        login: &str,
        year: i32,
    ) -> Result<Vec<RawContributionDay>, ClientError> {
        let from = format!("{year:04}-01-01T00:00:00Z");
        let to = format!("{year:04}-12-31T23:59:59Z");
        let request_body = serde_json::to_vec(&GraphQlRequest {
            query: CONTRIBUTION_YEAR_QUERY,
            variables: ContributionYearVariables {
                login,
                from: &from,
                to: &to,
            },
        })
        .map_err(ClientError::Json)?;
        let response = self
            .client
            .post(GRAPHQL_ENDPOINT)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-github-api-version", "2022-11-28")
            .bearer_auth(&self.token)
            .body(request_body)
            .send()
            .await
            .map_err(ClientError::Request)?;

        let status = response.status();
        let rate_limit_delay = rate_limit_delay(response.headers());
        let body = response.text().await.map_err(ClientError::Request)?;
        if !status.is_success() {
            if let Some(delay) = rate_limit_delay {
                return Err(ClientError::RateLimited(delay));
            }
            return Err(ClientError::Http {
                status,
                body: truncate_for_error(&body),
            });
        }

        let envelope: ContributionYearEnvelope =
            serde_json::from_str(&body).map_err(ClientError::Json)?;
        if let Some(errors) = envelope.errors
            && !errors.is_empty()
        {
            return Err(graphql_errors(errors, rate_limit_delay));
        }
        let data = envelope.data.ok_or(ClientError::MissingData)?;
        let user = data
            .user
            .ok_or_else(|| ClientError::UserNotFound(login.to_owned()))?;
        Ok(user
            .contributions
            .calendar
            .weeks
            .into_iter()
            .flat_map(|week| week.contribution_days)
            .collect())
    }

    async fn fetch_total_commits(&self, login: &str) -> Result<u64, ClientError> {
        let mut url = reqwest::Url::parse(COMMIT_SEARCH_ENDPOINT)
            .expect("GitHub commit search endpoint must be a valid URL");
        url.query_pairs_mut()
            .append_pair("q", &format!("author:{login}"))
            .append_pair("per_page", "1");

        let response = self
            .client
            .get(url)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(ClientError::Request)?;

        let status = response.status();
        let rate_limit_delay = rate_limit_delay(response.headers());
        let body = response.text().await.map_err(ClientError::Request)?;
        if !status.is_success() {
            if let Some(delay) = rate_limit_delay {
                return Err(ClientError::RateLimited(delay));
            }
            return Err(ClientError::Http {
                status,
                body: truncate_for_error(&body),
            });
        }

        let search: CommitSearchResponse =
            serde_json::from_str(&body).map_err(ClientError::Json)?;
        Ok(search.total_count)
    }
}

fn graphql_errors(errors: Vec<GraphQlError>, rate_limit_delay: Option<Duration>) -> ClientError {
    let messages = errors
        .into_iter()
        .map(|error| error.message)
        .collect::<Vec<_>>()
        .join("; ");
    if let Some(delay) = rate_limit_delay {
        return ClientError::RateLimited(delay);
    }
    if messages
        .to_ascii_lowercase()
        .contains("secondary rate limit")
    {
        return ClientError::RateLimited(Duration::from_secs(60));
    }
    ClientError::GraphQl(messages)
}

fn latest_contribution_date(calendar: &RawContributionCalendar) -> Option<&str> {
    calendar
        .weeks
        .iter()
        .flat_map(|week| week.contribution_days.iter())
        .map(|day| day.date.as_str())
        .max()
}

fn contribution_year(date: &str) -> Option<i32> {
    let year = date.get(..4)?.parse::<i32>().ok()?;
    (year > 0).then_some(year)
}

fn build_all_time_calendar(
    mut historical_days: Vec<RawContributionDay>,
    current_days: Vec<RawContributionDay>,
) -> RawContributionCalendar {
    historical_days.extend(current_days);
    historical_days.sort_unstable_by(|left, right| left.date.cmp(&right.date));
    historical_days.dedup_by(|left, right| left.date == right.date);

    if let Some(first_active) = historical_days
        .iter()
        .position(|day| day.contribution_count > 0)
    {
        historical_days.drain(..first_active);
    } else if let Some(latest) = historical_days.pop() {
        historical_days.push(latest);
    }

    let total_contributions = historical_days.iter().fold(0_u64, |total, day| {
        total.saturating_add(day.contribution_count)
    });
    let weeks = if historical_days.is_empty() {
        Vec::new()
    } else {
        vec![RawContributionWeek {
            contribution_days: historical_days,
        }]
    };
    RawContributionCalendar {
        total_contributions,
        weeks,
    }
}

fn rate_limit_delay(headers: &header::HeaderMap) -> Option<Duration> {
    if let Some(retry_after) = headers
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        return Some(Duration::from_secs(retry_after.max(1)));
    }

    let remaining = headers
        .get("x-ratelimit-remaining")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())?;
    if remaining != 0 {
        return None;
    }

    let reset_at = headers
        .get("x-ratelimit-reset")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Duration::from_secs(reset_at.saturating_sub(now).max(60)))
}

fn truncate_for_error(value: &str) -> String {
    const LIMIT: usize = 256;
    let mut output = value.chars().take(LIMIT).collect::<String>();
    if value.chars().count() > LIMIT {
        output.push('…');
    }
    output
}

#[derive(Serialize)]
struct GraphQlRequest<V> {
    query: &'static str,
    variables: V,
}

#[derive(Serialize)]
struct Variables<'a> {
    login: &'a str,
}

#[derive(Serialize)]
struct ContributionYearVariables<'a> {
    login: &'a str,
    from: &'a str,
    to: &'a str,
}

#[derive(Deserialize)]
struct GraphQlEnvelope {
    data: Option<GraphQlData>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Deserialize)]
struct GraphQlData {
    user: Option<RawUser>,
    #[serde(rename = "rateLimit")]
    rate_limit: RawRateLimit,
}

#[derive(Deserialize)]
struct ContributionYearEnvelope {
    data: Option<ContributionYearData>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Deserialize)]
struct ContributionYearData {
    user: Option<ContributionYearUser>,
}

#[derive(Deserialize)]
struct ContributionYearUser {
    #[serde(rename = "contributionsCollection")]
    contributions: ContributionYearCollection,
}

#[derive(Deserialize)]
struct ContributionYearCollection {
    #[serde(rename = "contributionCalendar")]
    calendar: RawContributionCalendar,
}

#[derive(Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Deserialize)]
struct CommitSearchResponse {
    total_count: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawProfile {
    pub user: RawUser,
    pub rate_limit: RawRateLimit,
    pub total_commits_all: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawUser {
    pub login: String,
    pub url: String,
    #[serde(rename = "contributionsCollection")]
    pub contributions: RawContributions,
    #[serde(rename = "contributionHistory")]
    contribution_history: RawContributionHistory,
    pub followers: RawCount,
    #[serde(rename = "repositoriesContributedTo")]
    pub repositories_contributed_to: RawCount,
    #[serde(rename = "pullRequests")]
    pub pull_requests: RawCount,
    #[serde(rename = "openIssues")]
    pub open_issues: RawCount,
    #[serde(rename = "closedIssues")]
    pub closed_issues: RawCount,
    pub repositories: RawRepositoryConnection,
}

#[derive(Clone, Debug, Deserialize)]
struct RawContributionHistory {
    #[serde(rename = "contributionYears")]
    contribution_years: Vec<i32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawCount {
    #[serde(rename = "totalCount")]
    pub total_count: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawContributions {
    #[serde(rename = "contributionCalendar")]
    pub calendar: RawContributionCalendar,
    #[serde(rename = "totalCommitContributions")]
    pub total_commit_contributions: u64,
    #[serde(rename = "totalIssueContributions")]
    pub total_issue_contributions: u64,
    #[serde(rename = "totalPullRequestContributions")]
    pub total_pull_request_contributions: u64,
    #[serde(rename = "totalPullRequestReviewContributions")]
    pub total_pull_request_review_contributions: u64,
    #[serde(rename = "restrictedContributionsCount")]
    pub restricted_contributions_count: u64,
    #[serde(rename = "hasAnyRestrictedContributions")]
    pub has_any_restricted_contributions: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawContributionCalendar {
    #[serde(rename = "totalContributions")]
    pub total_contributions: u64,
    pub weeks: Vec<RawContributionWeek>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawContributionWeek {
    #[serde(rename = "contributionDays")]
    pub contribution_days: Vec<RawContributionDay>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawContributionDay {
    #[serde(rename = "contributionCount")]
    pub contribution_count: u64,
    pub date: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawRepositoryConnection {
    #[serde(rename = "pageInfo")]
    pub page_info: RawPageInfo,
    pub nodes: Vec<Option<RawRepository>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawPageInfo {
    #[serde(rename = "hasNextPage")]
    pub has_next_page: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawRepository {
    pub name: String,
    #[serde(rename = "nameWithOwner")]
    pub name_with_owner: String,
    pub url: String,
    pub description: Option<String>,
    #[serde(rename = "isPrivate")]
    pub is_private: bool,
    #[serde(rename = "isFork")]
    pub is_fork: bool,
    #[serde(rename = "isArchived")]
    pub is_archived: bool,
    #[serde(rename = "stargazerCount")]
    pub stargazer_count: u64,
    #[serde(rename = "forkCount")]
    pub fork_count: u64,
    #[serde(rename = "pushedAt")]
    pub pushed_at: Option<String>,
    #[serde(rename = "primaryLanguage")]
    pub primary_language: Option<RawLanguage>,
    pub languages: RawLanguageConnection,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawLanguageConnection {
    pub edges: Vec<RawLanguageEdge>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawLanguageEdge {
    pub size: u64,
    pub node: RawLanguage,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawLanguage {
    pub name: String,
    pub color: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawRateLimit {
    pub cost: u64,
    pub remaining: u64,
    #[serde(rename = "resetAt")]
    pub reset_at: String,
}

#[derive(Debug)]
pub enum ClientError {
    GraphQl(String),
    Http { status: StatusCode, body: String },
    InvalidContributionCalendar,
    Json(serde_json::Error),
    MissingData,
    RateLimited(Duration),
    Request(reqwest::Error),
    UserNotFound(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GraphQl(message) => write!(formatter, "GitHub GraphQL error: {message}"),
            Self::Http { status, body } => {
                write!(formatter, "GitHub HTTP error {status}: {body}")
            }
            Self::InvalidContributionCalendar => {
                formatter.write_str("GitHub contribution calendar did not contain a current date")
            }
            Self::Json(error) => write!(formatter, "invalid GitHub JSON: {error}"),
            Self::MissingData => formatter.write_str("GitHub response did not contain data"),
            Self::RateLimited(delay) => write!(
                formatter,
                "GitHub rate limit reached; retry after {} seconds",
                delay.as_secs()
            ),
            Self::Request(error) => write!(formatter, "GitHub request error: {error}"),
            Self::UserNotFound(login) => write!(formatter, "GitHub user {login} was not found"),
        }
    }
}

impl Error for ClientError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Request(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};

    use super::{
        RawContributionDay, build_all_time_calendar, contribution_year, rate_limit_delay,
        truncate_for_error,
    };

    #[test]
    fn error_body_is_bounded() {
        let input = "x".repeat(300);
        let output = truncate_for_error(&input);
        assert!(output.chars().count() <= 257);
        assert!(output.ends_with('…'));
    }

    #[test]
    fn retry_after_header_controls_rate_limit_delay() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("75"));
        assert_eq!(rate_limit_delay(&headers).unwrap().as_secs(), 75);
    }

    #[test]
    fn all_time_calendar_sums_exact_daily_counts_and_trims_leading_zeros() {
        let historical = vec![
            RawContributionDay {
                contribution_count: 0,
                date: "2025-01-01".to_owned(),
            },
            RawContributionDay {
                contribution_count: 286,
                date: "2025-01-02".to_owned(),
            },
        ];
        let current = vec![RawContributionDay {
            contribution_count: 4_074,
            date: "2026-09-15".to_owned(),
        }];
        let calendar = build_all_time_calendar(historical, current);
        assert_eq!(calendar.total_contributions, 4_360);
        let days = &calendar.weeks[0].contribution_days;
        assert_eq!(days.first().unwrap().date, "2025-01-02");
        assert_eq!(days.last().unwrap().date, "2026-09-15");
    }

    #[test]
    fn contribution_year_reads_iso_calendar_year() {
        assert_eq!(contribution_year("2026-09-15"), Some(2026));
        assert_eq!(contribution_year("bad-date"), None);
    }
}
