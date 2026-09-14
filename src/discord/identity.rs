use std::{error::Error, fmt, sync::Arc, time::Duration};

use arc_swap::ArcSwapOption;
use reqwest::{Client, redirect::Policy};
use serde::Deserialize;
use tokio::time::MissedTickBehavior;
use twilight_model::id::{Id, marker::UserMarker};

const DISCORD_API_BASE: &str = "https://discord.com/api/v10";
const IDENTITY_REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscordIdentityData {
    pub avatar_decoration_url: Option<String>,
    pub avatar_url: String,
    pub display_name: String,
    pub primary_guild: Option<PrimaryGuildIdentity>,
    pub public_flags: u64,
    pub username: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrimaryGuildIdentity {
    pub badge_url: Option<String>,
    pub tag: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscordIdentitySnapshot {
    pub data: DiscordIdentityData,
    pub revision: u64,
}

pub struct IdentityStore {
    current: ArcSwapOption<DiscordIdentitySnapshot>,
}

impl IdentityStore {
    pub fn new() -> Self {
        Self {
            current: ArcSwapOption::from(None),
        }
    }

    pub fn load(&self) -> Option<Arc<DiscordIdentitySnapshot>> {
        self.current.load_full()
    }

    fn publish(&self, data: DiscordIdentityData) -> (bool, u64) {
        let current = self.current.load_full();
        if let Some(current) = current.as_deref()
            && current.data == data
        {
            return (false, current.revision);
        }

        let revision = current
            .as_deref()
            .map_or(1, |current| current.revision.saturating_add(1));
        self.current
            .store(Some(Arc::new(DiscordIdentitySnapshot { data, revision })));
        (true, revision)
    }
}

pub async fn run(
    bot_token: String,
    target_user_id: Id<UserMarker>,
    store: Arc<IdentityStore>,
) -> Result<(), IdentityCollectorError> {
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(10))
        .user_agent("profile-materials/0.1 discord-identity")
        .build()
        .map_err(IdentityCollectorError::Http)?;
    let mut interval = tokio::time::interval(IDENTITY_REFRESH_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        interval.tick().await;

        match fetch_identity(&client, &bot_token, target_user_id).await {
            Ok(identity) => {
                let (changed, revision) = store.publish(identity);
                if changed {
                    tracing::info!(revision, "Discord identity refreshed");
                } else {
                    tracing::debug!(revision, "Discord identity unchanged");
                }
            }
            Err(error) => tracing::warn!(
                error = %error,
                "Discord identity refresh failed; retaining the last successful identity"
            ),
        }
    }
}

async fn fetch_identity(
    client: &Client,
    bot_token: &str,
    target_user_id: Id<UserMarker>,
) -> Result<DiscordIdentityData, IdentityCollectorError> {
    let user_id = target_user_id.get().to_string();
    let response = client
        .get(format!("{DISCORD_API_BASE}/users/{user_id}"))
        .header("Authorization", format!("Bot {bot_token}"))
        .send()
        .await
        .map_err(IdentityCollectorError::Http)?;

    if !response.status().is_success() {
        return Err(IdentityCollectorError::UnexpectedStatus(
            response.status().as_u16(),
        ));
    }

    let body = response
        .text()
        .await
        .map_err(IdentityCollectorError::Http)?;
    let user: ApiUser = serde_json::from_str(&body).map_err(IdentityCollectorError::Decode)?;
    if user.id != user_id {
        return Err(IdentityCollectorError::MismatchedUser);
    }

    let display_name = user
        .global_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&user.username)
        .to_owned();
    let avatar_url = avatar_url(&user.id, user.avatar.as_deref());
    let avatar_decoration_url = user
        .avatar_decoration_data
        .as_ref()
        .map(|decoration| avatar_decoration_url(&decoration.asset));
    let primary_guild = user.primary_guild.as_ref().and_then(primary_guild);

    Ok(DiscordIdentityData {
        avatar_decoration_url,
        avatar_url,
        display_name,
        primary_guild,
        public_flags: user.public_flags.unwrap_or(0),
        username: user.username,
    })
}

fn avatar_url(user_id: &str, avatar_hash: Option<&str>) -> String {
    match avatar_hash {
        Some(hash) if !hash.trim().is_empty() => {
            format!("https://cdn.discordapp.com/avatars/{user_id}/{hash}.webp?size=256")
        }
        _ => {
            let index = user_id
                .parse::<u64>()
                .map(|value| (value >> 22) % 6)
                .unwrap_or(0);
            format!("https://cdn.discordapp.com/embed/avatars/{index}.png")
        }
    }
}

fn avatar_decoration_url(asset: &str) -> String {
    format!("https://cdn.discordapp.com/avatar-decoration-presets/{asset}.png?size=256")
}

fn primary_guild(guild: &ApiPrimaryGuild) -> Option<PrimaryGuildIdentity> {
    if guild.identity_enabled != Some(true) {
        return None;
    }

    let tag = guild
        .tag
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_owned();
    let badge_url = match (
        guild.identity_guild_id.as_deref(),
        guild
            .badge
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    ) {
        (Some(guild_id), Some(badge)) => Some(format!(
            "https://cdn.discordapp.com/guild-tag-badges/{guild_id}/{badge}.png?size=64"
        )),
        _ => None,
    };

    Some(PrimaryGuildIdentity { badge_url, tag })
}

#[derive(Debug, Deserialize)]
struct ApiUser {
    avatar: Option<String>,
    avatar_decoration_data: Option<ApiAvatarDecorationData>,
    global_name: Option<String>,
    id: String,
    primary_guild: Option<ApiPrimaryGuild>,
    public_flags: Option<u64>,
    username: String,
}

#[derive(Debug, Deserialize)]
struct ApiAvatarDecorationData {
    asset: String,
}

#[derive(Debug, Deserialize)]
struct ApiPrimaryGuild {
    badge: Option<String>,
    identity_enabled: Option<bool>,
    identity_guild_id: Option<String>,
    tag: Option<String>,
}

#[derive(Debug)]
pub enum IdentityCollectorError {
    Decode(serde_json::Error),
    Http(reqwest::Error),
    MismatchedUser,
    UnexpectedStatus(u16),
}

impl fmt::Display for IdentityCollectorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(error) => {
                write!(formatter, "Discord user response was invalid JSON: {error}")
            }
            Self::Http(error) => write!(formatter, "Discord user request failed: {error}"),
            Self::MismatchedUser => {
                formatter.write_str("Discord user response did not match the target user")
            }
            Self::UnexpectedStatus(status) => {
                write!(formatter, "Discord user request returned HTTP {status}")
            }
        }
    }
}

impl Error for IdentityCollectorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Decode(error) => Some(error),
            Self::Http(error) => Some(error),
            Self::MismatchedUser | Self::UnexpectedStatus(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiPrimaryGuild, avatar_url, primary_guild};

    #[test]
    fn default_avatar_is_derived_from_the_user_id() {
        assert!(avatar_url("984085171408080897", None).contains("/embed/avatars/"));
    }

    #[test]
    fn hidden_primary_guild_is_not_rendered() {
        assert!(
            primary_guild(&ApiPrimaryGuild {
                badge: Some("badge".to_owned()),
                identity_enabled: Some(false),
                identity_guild_id: Some("123".to_owned()),
                tag: Some("TEST".to_owned()),
            })
            .is_none()
        );
    }
}
