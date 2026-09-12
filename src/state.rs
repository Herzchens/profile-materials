pub mod lkg;

use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::clock::{self, ClockError};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PresenceData {
    pub activities: Vec<ActivitySnapshot>,
    pub client_status: ClientStatusSnapshot,
    pub status: PresenceStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActivitySnapshot {
    pub application_id: Option<String>,
    pub assets: Option<ActivityAssetsSnapshot>,
    pub details: Option<String>,
    pub kind: ActivityKind,
    pub name: String,
    pub state: Option<String>,
    pub timestamps: Option<ActivityTimestampsSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActivityAssetsSnapshot {
    pub large_image: Option<String>,
    pub large_text: Option<String>,
    pub small_image: Option<String>,
    pub small_text: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActivityTimestampsSnapshot {
    pub end: Option<u64>,
    pub start: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Competing,
    Custom,
    Listening,
    Playing,
    Streaming,
    Unknown(u8),
    Watching,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClientStatusSnapshot {
    pub desktop: Option<PresenceStatus>,
    pub mobile: Option<PresenceStatus>,
    pub web: Option<PresenceStatus>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    DoNotDisturb,
    Idle,
    Invisible,
    Offline,
    Online,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayStatus {
    Degraded,
    Live,
    Starting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresenceFreshness {
    Fresh,
    Grace,
    Stale,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeState {
    pub freshness: PresenceFreshness,
    pub gateway_status: GatewayStatus,
    pub last_target_event_unix_ms: Option<u64>,
    pub presence: PresenceState,
    pub stream_revision: u64,
    pub unvalidated_since_unix_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoredPresence {
    pub snapshot: PresenceSnapshot,
    pub validated_at_unix_ms: u64,
}

pub struct PresenceStore {
    current: ArcSwap<RuntimeState>,
    stale_after: Duration,
    unavailable_after: Duration,
    updates: watch::Sender<u64>,
    writer: Mutex<()>,
}

impl PresenceStore {
    pub fn new(
        stale_after: Duration,
        unavailable_after: Duration,
        restored: Option<RestoredPresence>,
        now_unix_ms: u64,
    ) -> Self {
        let initial = match restored {
            Some(restored) => {
                let presence = PresenceState::Known(restored.snapshot);
                let unvalidated_since_unix_ms = Some(restored.validated_at_unix_ms);
                let freshness = freshness_for(
                    &presence,
                    unvalidated_since_unix_ms,
                    now_unix_ms,
                    stale_after,
                    unavailable_after,
                );
                let stream_revision = match &presence {
                    PresenceState::Known(snapshot) => snapshot.revision,
                    PresenceState::Unknown => 0,
                };

                RuntimeState {
                    freshness,
                    gateway_status: GatewayStatus::Starting,
                    last_target_event_unix_ms: Some(restored.validated_at_unix_ms),
                    presence,
                    stream_revision,
                    unvalidated_since_unix_ms,
                }
            }
            None => RuntimeState {
                freshness: PresenceFreshness::Unavailable,
                gateway_status: GatewayStatus::Starting,
                last_target_event_unix_ms: None,
                presence: PresenceState::Unknown,
                stream_revision: 0,
                unvalidated_since_unix_ms: None,
            },
        };

        let (updates, _) = watch::channel(initial.stream_revision);

        Self {
            current: ArcSwap::from_pointee(initial),
            stale_after,
            unavailable_after,
            updates,
            writer: Mutex::new(()),
        }
    }

    pub fn load(&self) -> Arc<RuntimeState> {
        self.current.load_full()
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.updates.subscribe()
    }

    pub fn publish(
        &self,
        data: PresenceData,
        observed_at_unix_ms: u64,
    ) -> Result<PublishOutcome, PublishError> {
        let _guard = self.lock_writer()?;
        let current = self.current.load_full();
        let mut next = current.as_ref().clone();
        let previous_public_class = public_freshness_class(next.freshness);

        let outcome = match &next.presence {
            PresenceState::Known(snapshot) if snapshot.data == data => PublishOutcome::Unchanged {
                revision: snapshot.revision,
            },
            PresenceState::Known(snapshot) => {
                let revision = snapshot
                    .revision
                    .checked_add(1)
                    .ok_or(PublishError::RevisionExhausted)?;
                next.presence = PresenceState::Known(PresenceSnapshot {
                    data,
                    observed_at_unix_ms,
                    revision,
                });
                PublishOutcome::Changed { revision }
            }
            PresenceState::Unknown => {
                next.presence = PresenceState::Known(PresenceSnapshot {
                    data,
                    observed_at_unix_ms,
                    revision: 1,
                });
                PublishOutcome::Changed { revision: 1 }
            }
        };

        next.gateway_status = GatewayStatus::Live;
        next.last_target_event_unix_ms = Some(observed_at_unix_ms);
        next.unvalidated_since_unix_ms = None;
        next.freshness = PresenceFreshness::Fresh;

        let semantic_changed = matches!(outcome, PublishOutcome::Changed { .. });
        let freshness_changed = previous_public_class != public_freshness_class(next.freshness);
        self.store_next(next, semantic_changed || freshness_changed)?;

        Ok(outcome)
    }

    pub fn mark_gateway_live(&self) -> Result<bool, PublishError> {
        let _guard = self.lock_writer()?;
        let current = self.current.load_full();
        if current.gateway_status == GatewayStatus::Live {
            return Ok(false);
        }

        let mut next = current.as_ref().clone();
        next.gateway_status = GatewayStatus::Live;
        self.current.store(Arc::new(next));
        Ok(true)
    }

    pub fn mark_gateway_degraded(&self, now_unix_ms: u64) -> Result<bool, PublishError> {
        let _guard = self.lock_writer()?;
        let current = self.current.load_full();
        let mut next = current.as_ref().clone();
        let previous_public_class = public_freshness_class(next.freshness);
        let mut changed = false;

        if next.gateway_status != GatewayStatus::Degraded {
            next.gateway_status = GatewayStatus::Degraded;
            changed = true;
        }

        if next.unvalidated_since_unix_ms.is_none() {
            next.unvalidated_since_unix_ms = Some(now_unix_ms);
            next.freshness = freshness_for(
                &next.presence,
                next.unvalidated_since_unix_ms,
                now_unix_ms,
                self.stale_after,
                self.unavailable_after,
            );
            changed = true;
        }

        if !changed {
            return Ok(false);
        }

        let public_changed = previous_public_class != public_freshness_class(next.freshness);
        self.store_next(next, public_changed)?;
        Ok(true)
    }

    pub fn advance_staleness(&self, now_unix_ms: u64) -> Result<bool, PublishError> {
        let _guard = self.lock_writer()?;
        let current = self.current.load_full();
        let Some(unvalidated_since_unix_ms) = current.unvalidated_since_unix_ms else {
            return Ok(false);
        };

        let next_freshness = freshness_for(
            &current.presence,
            Some(unvalidated_since_unix_ms),
            now_unix_ms,
            self.stale_after,
            self.unavailable_after,
        );
        if next_freshness == current.freshness {
            return Ok(false);
        }

        let mut next = current.as_ref().clone();
        let public_changed =
            public_freshness_class(next.freshness) != public_freshness_class(next_freshness);
        next.freshness = next_freshness;
        self.store_next(next, public_changed)?;
        Ok(true)
    }

    pub fn persistence_snapshot(&self) -> Option<(PresenceSnapshot, u64)> {
        let current = self.load();
        let PresenceState::Known(snapshot) = &current.presence else {
            return None;
        };
        let validated_at_unix_ms = current
            .last_target_event_unix_ms
            .unwrap_or(snapshot.observed_at_unix_ms);

        Some((snapshot.clone(), validated_at_unix_ms))
    }

    fn lock_writer(&self) -> Result<std::sync::MutexGuard<'_, ()>, PublishError> {
        self.writer.lock().map_err(|_| PublishError::WriterPoisoned)
    }

    fn store_next(&self, mut next: RuntimeState, public_changed: bool) -> Result<(), PublishError> {
        if public_changed {
            next.stream_revision = next
                .stream_revision
                .checked_add(1)
                .ok_or(PublishError::StreamRevisionExhausted)?;
        }
        let stream_revision = next.stream_revision;
        self.current.store(Arc::new(next));
        if public_changed {
            self.updates.send_replace(stream_revision);
        }
        Ok(())
    }
}

fn freshness_for(
    presence: &PresenceState,
    unvalidated_since_unix_ms: Option<u64>,
    now_unix_ms: u64,
    stale_after: Duration,
    unavailable_after: Duration,
) -> PresenceFreshness {
    if matches!(presence, PresenceState::Unknown) {
        return PresenceFreshness::Unavailable;
    }

    let Some(unvalidated_since_unix_ms) = unvalidated_since_unix_ms else {
        return PresenceFreshness::Fresh;
    };

    let age_ms = now_unix_ms.saturating_sub(unvalidated_since_unix_ms);
    let stale_after_ms = stale_after.as_millis().min(u128::from(u64::MAX)) as u64;
    let unavailable_after_ms = unavailable_after.as_millis().min(u128::from(u64::MAX)) as u64;

    if age_ms >= unavailable_after_ms {
        PresenceFreshness::Unavailable
    } else if age_ms >= stale_after_ms {
        PresenceFreshness::Stale
    } else {
        PresenceFreshness::Grace
    }
}

fn public_freshness_class(freshness: PresenceFreshness) -> u8 {
    match freshness {
        PresenceFreshness::Fresh | PresenceFreshness::Grace => 0,
        PresenceFreshness::Stale => 1,
        PresenceFreshness::Unavailable => 2,
    }
}

pub async fn run_stale_watchdog(store: Arc<PresenceStore>) -> Result<(), WatchdogError> {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        let now_unix_ms = clock::unix_time_millis().map_err(WatchdogError::Clock)?;
        store
            .advance_staleness(now_unix_ms)
            .map_err(WatchdogError::Publish)?;
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
    StreamRevisionExhausted,
    WriterPoisoned,
}

impl fmt::Display for PublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RevisionExhausted => formatter.write_str("presence revision counter exhausted"),
            Self::StreamRevisionExhausted => {
                formatter.write_str("presence stream revision counter exhausted")
            }
            Self::WriterPoisoned => formatter.write_str("presence writer lock is poisoned"),
        }
    }
}

impl Error for PublishError {}

#[derive(Debug)]
pub enum WatchdogError {
    Clock(ClockError),
    Publish(PublishError),
}

impl fmt::Display for WatchdogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clock(error) => write!(formatter, "stale watchdog clock error: {error}"),
            Self::Publish(error) => write!(formatter, "stale watchdog state error: {error}"),
        }
    }
}

impl Error for WatchdogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Clock(error) => Some(error),
            Self::Publish(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        ClientStatusSnapshot, GatewayStatus, PresenceData, PresenceFreshness, PresenceState,
        PresenceStatus, PresenceStore, PublishOutcome, RestoredPresence,
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

    fn store() -> PresenceStore {
        PresenceStore::new(Duration::from_secs(120), Duration::from_secs(600), None, 0)
    }

    #[test]
    fn starts_unknown() {
        let current = store().load();
        assert_eq!(current.presence, PresenceState::Unknown);
        assert_eq!(current.gateway_status, GatewayStatus::Starting);
        assert_eq!(current.freshness, PresenceFreshness::Unavailable);
    }

    #[test]
    fn publishes_only_semantic_changes() {
        let store = store();

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
        let PresenceState::Known(snapshot) = &current.presence else {
            panic!("expected known presence");
        };
        assert_eq!(snapshot.observed_at_unix_ms, 300);
        assert_eq!(snapshot.revision, 2);
        assert_eq!(current.last_target_event_unix_ms, Some(300));
        assert_eq!(current.freshness, PresenceFreshness::Fresh);
    }

    #[test]
    fn disconnect_keeps_lkg_then_becomes_stale_and_unavailable() {
        let store = store();
        store
            .publish(presence(PresenceStatus::Online), 1_000)
            .unwrap();
        store.mark_gateway_degraded(2_000).unwrap();

        let current = store.load();
        assert_eq!(current.freshness, PresenceFreshness::Grace);
        let PresenceState::Known(snapshot) = &current.presence else {
            panic!("expected known presence");
        };
        assert_eq!(snapshot.data.status, PresenceStatus::Online);

        store.advance_staleness(122_000).unwrap();
        assert_eq!(store.load().freshness, PresenceFreshness::Stale);

        store.advance_staleness(602_000).unwrap();
        assert_eq!(store.load().freshness, PresenceFreshness::Unavailable);
    }

    #[test]
    fn restored_lkg_uses_validation_age_until_a_live_target_event_arrives() {
        let snapshot = super::PresenceSnapshot {
            data: presence(PresenceStatus::Idle),
            observed_at_unix_ms: 1_000,
            revision: 7,
        };
        let store = PresenceStore::new(
            Duration::from_secs(120),
            Duration::from_secs(600),
            Some(RestoredPresence {
                snapshot,
                validated_at_unix_ms: 1_000,
            }),
            301_000,
        );

        assert_eq!(store.load().freshness, PresenceFreshness::Stale);
        assert_eq!(
            store
                .publish(presence(PresenceStatus::Idle), 302_000)
                .unwrap(),
            PublishOutcome::Unchanged { revision: 7 }
        );
        assert_eq!(store.load().freshness, PresenceFreshness::Fresh);
    }
}
