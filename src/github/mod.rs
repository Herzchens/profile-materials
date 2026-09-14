pub mod cards;
mod client;
pub mod lkg;
pub mod stats;

use std::{
    error::Error,
    fmt,
    sync::{Arc, RwLock},
    time::Duration,
};

use crate::{clock, config::GitHubConfig};
use client::{ClientError, GitHubClient};
use stats::{GitHubSnapshot, build_snapshot};

const LOW_RATE_LIMIT_THRESHOLD: u64 = 100;
const LOW_RATE_LIMIT_DELAY: Duration = Duration::from_secs(60 * 60);

pub struct GitHubStore {
    snapshot: RwLock<Option<Arc<GitHubSnapshot>>>,
}

impl GitHubStore {
    pub fn new(restored: Option<GitHubSnapshot>) -> Self {
        Self {
            snapshot: RwLock::new(restored.map(Arc::new)),
        }
    }

    pub fn load(&self) -> Option<Arc<GitHubSnapshot>> {
        match self.snapshot.read() {
            Ok(snapshot) => snapshot.clone(),
            Err(poisoned) => {
                tracing::warn!("GitHub snapshot lock was poisoned; recovering stored value");
                poisoned.into_inner().clone()
            }
        }
    }

    pub fn publish(&self, mut next: GitHubSnapshot) -> Arc<GitHubSnapshot> {
        let mut slot = match self.snapshot.write() {
            Ok(slot) => slot,
            Err(poisoned) => {
                tracing::warn!("GitHub snapshot lock was poisoned; recovering stored value");
                poisoned.into_inner()
            }
        };

        next.revision = match slot.as_deref() {
            Some(previous) if previous.semantic_eq(&next) => previous.revision,
            Some(previous) => previous.revision.saturating_add(1),
            None => 1,
        };

        let next = Arc::new(next);
        *slot = Some(Arc::clone(&next));
        next
    }
}

pub async fn run(config: GitHubConfig, store: Arc<GitHubStore>) -> Result<(), RunError> {
    let Some(token) = config.token.clone() else {
        tracing::warn!(
            username = %config.username,
            "GitHub collector disabled because GITHUB_TOKEN is not configured"
        );
        return std::future::pending::<Result<(), RunError>>().await;
    };

    let client = GitHubClient::new(token).map_err(RunError::BuildClient)?;

    loop {
        let attempt_started = tokio::time::Instant::now();
        let (delay, backoff_from_completion) = match client.fetch_profile(&config.username).await {
            Ok(raw) => {
                let collected_at_unix_ms = match clock::unix_time_millis() {
                    Ok(now) => now,
                    Err(error) => {
                        tracing::warn!(error = %error, "GitHub snapshot timestamp unavailable");
                        let sleep_for = config
                            .poll_interval
                            .saturating_sub(attempt_started.elapsed());
                        tokio::time::sleep(sleep_for).await;
                        continue;
                    }
                };
                let next = build_snapshot(raw, &config.featured_repositories, collected_at_unix_ms);
                let published = store.publish(next);

                if let Err(error) = lkg::save(&config.state_path, published.as_ref()).await {
                    tracing::warn!(
                        path = %config.state_path.display(),
                        error = %error,
                        "GitHub LKG could not be persisted"
                    );
                }

                tracing::info!(
                    username = %published.login,
                    revision = published.revision,
                    contributions = published.contributions.total,
                    commits = published.overall.total_commits,
                    repositories = published.repository_count,
                    rate_limit_cost = published.rate_limit.cost,
                    rate_limit_remaining = published.rate_limit.remaining,
                    rate_limit_reset_at = %published.rate_limit.reset_at,
                    "GitHub stats published"
                );

                if published.rate_limit.remaining <= LOW_RATE_LIMIT_THRESHOLD {
                    (config.poll_interval.max(LOW_RATE_LIMIT_DELAY), true)
                } else {
                    (config.poll_interval, false)
                }
            }
            Err(ClientError::RateLimited(delay)) => {
                tracing::warn!(
                    username = %config.username,
                    retry_after_secs = delay.as_secs(),
                    "GitHub rate limit reached; keeping last known snapshot"
                );
                (config.poll_interval.max(delay), true)
            }
            Err(error) => {
                tracing::warn!(
                    username = %config.username,
                    error = %error,
                    "GitHub stats refresh failed; keeping last known snapshot"
                );
                (config.poll_interval, false)
            }
        };

        let sleep_for = if backoff_from_completion {
            delay
        } else {
            delay.saturating_sub(attempt_started.elapsed())
        };
        tokio::time::sleep(sleep_for).await;
    }
}

#[derive(Debug)]
pub enum RunError {
    BuildClient(reqwest::Error),
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BuildClient(error) => write!(formatter, "failed to build GitHub client: {error}"),
        }
    }
}

impl Error for RunError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BuildClient(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GitHubStore;
    use crate::github::stats::{
        ContributionSummary, GitHubSnapshot, OverallStats, RankSnapshot, RateLimitSnapshot,
    };

    #[test]
    fn unchanged_semantics_keep_revision_but_refresh_metadata() {
        let store = GitHubStore::new(None);
        let first = store.publish(snapshot(10, 0));
        let second = store.publish(snapshot(20, 999));
        assert_eq!(first.revision, 1);
        assert_eq!(second.revision, 1);
        assert_eq!(second.collected_at_unix_ms, 20);
        assert_eq!(second.rate_limit.remaining, 999);
    }

    #[test]
    fn semantic_change_advances_revision() {
        let store = GitHubStore::new(None);
        store.publish(snapshot(10, 100));
        let mut changed = snapshot(20, 99);
        changed.contributions.total = 11;
        assert_eq!(store.publish(changed).revision, 2);
    }

    fn snapshot(collected_at_unix_ms: u64, remaining: u64) -> GitHubSnapshot {
        GitHubSnapshot {
            revision: 0,
            collected_at_unix_ms,
            login: "Herzchens".to_owned(),
            url: "https://github.com/Herzchens".to_owned(),
            contributions: ContributionSummary {
                total: 10,
                commits: 8,
                issues: 1,
                pull_requests: 1,
                reviews: 0,
                active_days: 5,
                current_streak_days: 2,
                longest_streak_days: 3,
                calendar_start: None,
                calendar_end: None,
                today_contributions: None,
                current_streak_start: None,
                current_streak_end: None,
                longest_streak_start: None,
                longest_streak_end: None,
                restricted_contributions: 0,
                includes_restricted_contributions: false,
            },
            overall: OverallStats {
                total_commits: 20,
                total_pull_requests: 4,
                total_reviews: 2,
                total_issues: 3,
                total_stars: 5,
                contributed_to: 6,
                followers: 7,
                rank: RankSnapshot {
                    level: "B".to_owned(),
                    percentile: 55.0,
                },
            },
            languages: Vec::new(),
            projects: Vec::new(),
            repository_count: 2,
            source_truncated: false,
            rate_limit: RateLimitSnapshot {
                cost: 1,
                remaining,
                reset_at: "2026-09-12T11:00:00Z".to_owned(),
            },
        }
    }
}
