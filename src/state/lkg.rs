use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    process,
};

use serde::{Deserialize, Serialize};
use tokio::fs;

use super::{PresenceSnapshot, RestoredPresence};

const SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct LkgFile {
    schema_version: u8,
    snapshot: PresenceSnapshot,
    validated_at_unix_ms: u64,
}

pub async fn load(path: &Path) -> Result<Option<RestoredPresence>, LkgError> {
    let bytes = match fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(LkgError::Io(error)),
    };
    let file: LkgFile = serde_json::from_slice(&bytes).map_err(LkgError::Json)?;
    if file.schema_version != SCHEMA_VERSION {
        return Err(LkgError::UnsupportedSchema(file.schema_version));
    }

    Ok(Some(RestoredPresence {
        snapshot: file.snapshot,
        validated_at_unix_ms: file.validated_at_unix_ms,
    }))
}

pub async fn save(
    path: &Path,
    snapshot: &PresenceSnapshot,
    validated_at_unix_ms: u64,
) -> Result<(), LkgError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).await.map_err(LkgError::Io)?;
    }

    let payload = serde_json::to_vec(&LkgFile {
        schema_version: SCHEMA_VERSION,
        snapshot: snapshot.clone(),
        validated_at_unix_ms,
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
        .unwrap_or("presence.json");
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
            Self::Io(error) => write!(formatter, "LKG I/O error: {error}"),
            Self::Json(error) => write!(formatter, "invalid LKG JSON: {error}"),
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported LKG schema version {version}")
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
    use crate::state::{ClientStatusSnapshot, PresenceData, PresenceSnapshot, PresenceStatus};

    #[tokio::test]
    async fn round_trips_lkg_snapshot() {
        let path = test_path();
        let snapshot = PresenceSnapshot {
            data: PresenceData {
                activities: Vec::new(),
                client_status: ClientStatusSnapshot {
                    desktop: Some(PresenceStatus::Online),
                    mobile: None,
                    web: None,
                },
                status: PresenceStatus::Online,
            },
            observed_at_unix_ms: 123,
            revision: 9,
        };

        save(&path, &snapshot, 456).await.unwrap();
        let restored = load(&path).await.unwrap().unwrap();
        assert_eq!(restored.snapshot, snapshot);
        assert_eq!(restored.validated_at_unix_ms, 456);

        let _ = tokio::fs::remove_file(path).await;
    }

    fn test_path() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "profile-service-lkg-{}-{nonce}.json",
            std::process::id()
        ))
    }
}
