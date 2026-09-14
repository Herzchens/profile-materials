use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    process,
};

use serde::{Deserialize, Serialize};
use tokio::fs;

use super::stats::GitHubSnapshot;

const SCHEMA_VERSION: u8 = 2;

#[derive(Deserialize, Serialize)]
struct LkgFile {
    schema_version: u8,
    snapshot: GitHubSnapshot,
}

pub async fn load(path: &Path) -> Result<Option<GitHubSnapshot>, LkgError> {
    let bytes = match fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(LkgError::Io(error)),
    };
    let file: LkgFile = serde_json::from_slice(&bytes).map_err(LkgError::Json)?;
    if file.schema_version != SCHEMA_VERSION {
        return Err(LkgError::UnsupportedSchema(file.schema_version));
    }
    Ok(Some(file.snapshot))
}

pub async fn save(path: &Path, snapshot: &GitHubSnapshot) -> Result<(), LkgError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).await.map_err(LkgError::Io)?;
    }

    let payload = serde_json::to_vec(&LkgFile {
        schema_version: SCHEMA_VERSION,
        snapshot: snapshot.clone(),
    })
    .map_err(LkgError::Json)?;
    let temporary_path = temporary_path(path);

    fs::write(&temporary_path, payload)
        .await
        .map_err(LkgError::Io)?;
    if let Err(error) = fs::rename(&temporary_path, path).await {
        let _ = fs::remove_file(&temporary_path).await;
        return Err(LkgError::Io(error));
    }

    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("github.json");
    path.with_file_name(format!(".{file_name}.tmp-{}", process::id()))
}

#[derive(Debug)]
pub enum LkgError {
    Io(std::io::Error),
    Json(serde_json::Error),
    UnsupportedSchema(u8),
}

impl fmt::Display for LkgError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "GitHub LKG I/O error: {error}"),
            Self::Json(error) => write!(formatter, "invalid GitHub LKG JSON: {error}"),
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported GitHub LKG schema version {version}")
            }
        }
    }
}

impl Error for LkgError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::UnsupportedSchema(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{load, save};
    use crate::github::stats::{
        ContributionSummary, GitHubSnapshot, OverallStats, RankSnapshot, RateLimitSnapshot,
    };

    #[tokio::test]
    async fn round_trips_snapshot() {
        let path = test_path();
        let snapshot = GitHubSnapshot {
            revision: 4,
            collected_at_unix_ms: 123,
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
                remaining: 4999,
                reset_at: "2026-09-12T11:00:00Z".to_owned(),
            },
        };

        save(&path, &snapshot).await.unwrap();
        assert_eq!(load(&path).await.unwrap(), Some(snapshot));
        let _ = tokio::fs::remove_file(path).await;
    }

    fn test_path() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "profile-service-github-lkg-{}-{nonce}.json",
            std::process::id()
        ))
    }
}
