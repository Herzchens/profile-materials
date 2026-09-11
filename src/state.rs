use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex},
};

use arc_swap::ArcSwap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresenceData {
    pub activities: Vec<ActivitySnapshot>,
    pub client_status: ClientStatusSnapshot,
    pub status: PresenceStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivitySnapshot {
    pub application_id: Option<String>,
    pub assets: Option<ActivityAssetsSnapshot>,
    pub details: Option<String>,
    pub kind: ActivityKind,
    pub name: String,
    pub state: Option<String>,
    pub timestamps: Option<ActivityTimestampsSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityAssetsSnapshot {
    pub large_image: Option<String>,
    pub large_text: Option<String>,
    pub small_image: Option<String>,
    pub small_text: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityTimestampsSnapshot {
    pub end: Option<u64>,
    pub start: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityKind {
    Competing,
    Custom,
    Listening,
    Playing,
    Streaming,
    Unknown(u8),
    Watching,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientStatusSnapshot {
    pub desktop: Option<PresenceStatus>,
    pub mobile: Option<PresenceStatus>,
    pub web: Option<PresenceStatus>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresenceStatus {
    DoNotDisturb,
    Idle,
    Invisible,
    Offline,
    Online,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresenceSnapshot {
    pub data: PresenceData,
    pub observed_at_unix_ms: u64,
    pub revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PresenceState {
    Known(PresenceSnapshot),
    Unknown,
}

pub struct PresenceStore {
    current: ArcSwap<PresenceState>,
    writer: Mutex<()>,
}

impl PresenceStore {
    pub fn new() -> Self {
        Self {
            current: ArcSwap::from_pointee(PresenceState::Unknown),
            writer: Mutex::new(()),
        }
    }

    pub fn load(&self) -> Arc<PresenceState> {
        self.current.load_full()
    }

    pub fn publish(
        &self,
        data: PresenceData,
        observed_at_unix_ms: u64,
    ) -> Result<PublishOutcome, PublishError> {
        let _guard = self
            .writer
            .lock()
            .map_err(|_| PublishError::WriterPoisoned)?;
        let current = self.current.load_full();

        if let PresenceState::Known(snapshot) = current.as_ref()
            && snapshot.data == data
        {
            return Ok(PublishOutcome::Unchanged {
                revision: snapshot.revision,
            });
        }

        let revision = match current.as_ref() {
            PresenceState::Known(snapshot) => snapshot
                .revision
                .checked_add(1)
                .ok_or(PublishError::RevisionExhausted)?,
            PresenceState::Unknown => 1,
        };

        self.current
            .store(Arc::new(PresenceState::Known(PresenceSnapshot {
                data,
                observed_at_unix_ms,
                revision,
            })));

        Ok(PublishOutcome::Changed { revision })
    }
}

impl Default for PresenceStore {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishOutcome {
    Changed { revision: u64 },
    Unchanged { revision: u64 },
}

#[derive(Debug, Eq, PartialEq)]
pub enum PublishError {
    RevisionExhausted,
    WriterPoisoned,
}

impl fmt::Display for PublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RevisionExhausted => formatter.write_str("presence revision counter exhausted"),
            Self::WriterPoisoned => formatter.write_str("presence writer lock is poisoned"),
        }
    }
}

impl Error for PublishError {}

#[cfg(test)]
mod tests {
    use super::{
        ClientStatusSnapshot, PresenceData, PresenceState, PresenceStatus, PresenceStore,
        PublishOutcome,
    };

    fn presence(status: PresenceStatus) -> PresenceData {
        PresenceData {
            activities: Vec::new(),
            client_status: ClientStatusSnapshot {
                desktop: None,
                mobile: None,
                web: None,
            },
            status,
        }
    }

    #[test]
    fn starts_unknown() {
        assert_eq!(
            PresenceStore::new().load().as_ref(),
            &PresenceState::Unknown
        );
    }

    #[test]
    fn publishes_only_semantic_changes() {
        let store = PresenceStore::new();

        assert_eq!(
            store
                .publish(presence(PresenceStatus::Online), 100)
                .unwrap(),
            PublishOutcome::Changed { revision: 1 }
        );
        assert_eq!(
            store
                .publish(presence(PresenceStatus::Online), 200)
                .unwrap(),
            PublishOutcome::Unchanged { revision: 1 }
        );
        assert_eq!(
            store.publish(presence(PresenceStatus::Idle), 300).unwrap(),
            PublishOutcome::Changed { revision: 2 }
        );

        let current = store.load();
        let PresenceState::Known(snapshot) = current.as_ref() else {
            panic!("expected known presence");
        };
        assert_eq!(snapshot.observed_at_unix_ms, 300);
        assert_eq!(snapshot.revision, 2);
    }
}
