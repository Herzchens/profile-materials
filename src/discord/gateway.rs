use std::{
    error::Error,
    fmt,
    sync::Arc,
    time::{Instant, SystemTime, SystemTimeError, UNIX_EPOCH},
};

use twilight_gateway::{
    Event, EventTypeFlags, Intents, Shard, ShardId, ShardState, StreamExt as _,
};

use crate::{
    config::DiscordConfig,
    discord::normalize::{is_target, normalize_presence},
    state::{PresenceStore, PublishError, PublishOutcome},
};

pub async fn run(config: DiscordConfig, store: Arc<PresenceStore>) -> Result<(), GatewayError> {
    let mut shard = Shard::new(
        ShardId::ONE,
        config.bot_token,
        Intents::GUILDS | Intents::GUILD_PRESENCES,
    );
    let mut previous_state = shard.state();

    tracing::info!(gateway_state = ?previous_state, "Discord Gateway collector starting");

    loop {
        let item = shard.next_event(EventTypeFlags::PRESENCE_UPDATE).await;
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
            Ok(Event::PresenceUpdate(presence)) => {
                let received_at = Instant::now();

                if !is_target(&presence, config.target_user_id, config.target_guild_id) {
                    continue;
                }

                let data = normalize_presence(&presence);
                let observed_at_unix_ms = unix_time_millis()?;
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
                        "duplicate target presence ignored"
                    ),
                }
            }
            Ok(_) => {}
            Err(_) => {
                tracing::warn!(
                    gateway_state = ?shard.state(),
                    "Discord Gateway receive error; payload and credentials intentionally omitted"
                );
            }
        }

        if shard.state() == ShardState::FatallyClosed {
            return Err(GatewayError::FatallyClosed);
        }
    }
}

fn unix_time_millis() -> Result<u64, GatewayError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(GatewayError::Clock)?;

    u64::try_from(elapsed.as_millis()).map_err(|_| GatewayError::TimestampOverflow)
}

#[derive(Debug)]
pub enum GatewayError {
    Clock(SystemTimeError),
    FatallyClosed,
    Publish(PublishError),
    StreamEnded,
    TimestampOverflow,
}

impl fmt::Display for GatewayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clock(_) => formatter.write_str("system clock is before the Unix epoch"),
            Self::FatallyClosed => formatter
                .write_str("Discord Gateway closed fatally; verify token and privileged intents"),
            Self::Publish(error) => {
                write!(formatter, "failed to publish presence snapshot: {error}")
            }
            Self::StreamEnded => {
                formatter.write_str("Discord Gateway event stream ended unexpectedly")
            }
            Self::TimestampOverflow => {
                formatter.write_str("current Unix timestamp does not fit in u64 milliseconds")
            }
        }
    }
}

impl Error for GatewayError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Clock(error) => Some(error),
            Self::Publish(error) => Some(error),
            Self::FatallyClosed | Self::StreamEnded | Self::TimestampOverflow => None,
        }
    }
}

impl From<PublishError> for GatewayError {
    fn from(error: PublishError) -> Self {
        Self::Publish(error)
    }
}
