use std::{error::Error, fmt, path::PathBuf, sync::Arc, time::Instant};

use twilight_gateway::{
    Event, EventTypeFlags, Intents, Shard, ShardId, ShardState, StreamExt as _,
};

use crate::{
    clock::{self, ClockError},
    config::DiscordConfig,
    discord::normalize::{is_target, normalize_presence},
    state::{PresenceStore, PublishError, PublishOutcome, lkg},
};

pub async fn run(
    config: DiscordConfig,
    store: Arc<PresenceStore>,
    lkg_path: PathBuf,
) -> Result<(), GatewayError> {
    let mut shard = Shard::new(
        ShardId::ONE,
        config.bot_token,
        Intents::GUILDS | Intents::GUILD_PRESENCES,
    );
    let mut previous_state = shard.state();

    tracing::info!(gateway_state = ?previous_state, "Discord Gateway collector starting");

    loop {
        let item = shard.next_event(EventTypeFlags::all()).await;
        let current_state = shard.state();

        if current_state != previous_state {
            tracing::info!(
                previous_gateway_state = ?previous_state,
                gateway_state = ?current_state,
                "Discord Gateway state changed"
            );
            previous_state = current_state;
        }

        let Some(item) = item else {
            return if current_state == ShardState::FatallyClosed {
                Err(GatewayError::FatallyClosed)
            } else {
                Err(GatewayError::StreamEnded)
            };
        };

        match item {
            Ok(event) => {
                store.mark_gateway_live()?;

                if let Event::PresenceUpdate(presence) = event {
                    let received_at = Instant::now();

                    if !is_target(&presence, config.target_user_id, config.target_guild_id) {
                        continue;
                    }

                    let data = normalize_presence(&presence);
                    let observed_at_unix_ms = clock::unix_time_millis()?;
                    let activity_count = data.activities.len();
                    let status = data.status;

                    match store.publish(data, observed_at_unix_ms)? {
                        PublishOutcome::Changed { revision } => tracing::info!(
                            revision,
                            status = ?status,
                            activity_count,
                            processing_latency_us = received_at.elapsed().as_micros(),
                            "target presence published"
                        ),
                        PublishOutcome::Unchanged { revision } => tracing::debug!(
                            revision,
                            processing_latency_us = received_at.elapsed().as_micros(),
                            "duplicate target presence refreshed"
                        ),
                    }

                    if let Some((snapshot, validated_at_unix_ms)) = store.persistence_snapshot()
                        && let Err(error) =
                            lkg::save(&lkg_path, &snapshot, validated_at_unix_ms).await
                    {
                        tracing::error!(
                            path = %lkg_path.display(),
                            error = %error,
                            "failed to persist presence LKG"
                        );
                    }
                }
            }
            Err(_) => {
                let now_unix_ms = clock::unix_time_millis()?;
                store.mark_gateway_degraded(now_unix_ms)?;
                tracing::warn!(
                    gateway_state = ?shard.state(),
                    "Discord Gateway receive error; retaining last-known-good presence"
                );
            }
        }

        if shard.state() == ShardState::FatallyClosed {
            return Err(GatewayError::FatallyClosed);
        }
    }
}

#[derive(Debug)]
pub enum GatewayError {
    Clock(ClockError),
    FatallyClosed,
    Publish(PublishError),
    StreamEnded,
}

impl fmt::Display for GatewayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clock(error) => write!(formatter, "Discord Gateway clock error: {error}"),
            Self::FatallyClosed => formatter
                .write_str("Discord Gateway closed fatally; verify token and privileged intents"),
            Self::Publish(error) => {
                write!(
                    formatter,
                    "failed to update presence runtime state: {error}"
                )
            }
            Self::StreamEnded => {
                formatter.write_str("Discord Gateway event stream ended unexpectedly")
            }
        }
    }
}

impl Error for GatewayError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Clock(error) => Some(error),
            Self::Publish(error) => Some(error),
            Self::FatallyClosed | Self::StreamEnded => None,
        }
    }
}

impl From<ClockError> for GatewayError {
    fn from(error: ClockError) -> Self {
        Self::Clock(error)
    }
}

impl From<PublishError> for GatewayError {
    fn from(error: PublishError) -> Self {
        Self::Publish(error)
    }
}
