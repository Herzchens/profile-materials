use std::{
    error::Error,
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{Client, StatusCode, header};
use serde::{Deserialize, Serialize};

const GRAPHQL_ENDPOINT: &str = "https://api.github.com/graphql";
const COMMIT_SEARCH_ENDPOINT: &str = "https://api.github.com/search/commits";
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

pub struct GitHubClient {
    client: Client,
    token: String,
}

impl GitHubClient {
    pub fn new(token: String) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .timeout(Duration::from_secs(10))
            .user_agent("profile-materials/0.1 github-stats")
            .build()?;
        Ok(Self { client, token })
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
            let messages = errors
                .into_iter()
                .map(|error| error.message)
                .collect::<Vec<_>>()
                .join("; ");
            if let Some(delay) = rate_limit_delay {
                return Err(ClientError::RateLimited(delay));
            }
            if messages
                .to_ascii_lowercase()
                .contains("secondary rate limit")
            {
                return Err(ClientError::RateLimited(Duration::from_secs(60)));
            }
            return Err(ClientError::GraphQl(messages));
        }

        let data = envelope.data.ok_or(ClientError::MissingData)?;
        let user = data
            .user
            .ok_or_else(|| ClientError::UserNotFound(login.to_owned()))?;
        let total_commits_all = self.fetch_total_commits(login).await?;

        Ok(RawProfile {
            user,
            rate_limit: data.rate_limit,
            total_commits_all,
        })
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
struct GraphQlRequest<'a> {
    query: &'static str,
    variables: Variables<'a>,
}

#[derive(Serialize)]
struct Variables<'a> {
    login: &'a str,
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

    use super::{rate_limit_delay, truncate_for_error};

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
}
