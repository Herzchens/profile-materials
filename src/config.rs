use std::{
    collections::HashSet, env, error::Error, ffi::OsString, fmt, net::SocketAddr, path::PathBuf,
    time::Duration,
};

use twilight_model::id::{
    Id,
    marker::{GuildMarker, UserMarker},
};

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:3000";
const DEFAULT_STATE_PATH: &str = "state/presence.json";
const DEFAULT_STALE_AFTER_SECS: u64 = 120;
const DEFAULT_UNAVAILABLE_AFTER_SECS: u64 = 600;
const DEFAULT_GITHUB_USERNAME: &str = "Herzchens";
const DEFAULT_GITHUB_STATE_PATH: &str = "state/github.json";
const DEFAULT_GITHUB_POLL_SECS: u64 = 15;
const DEFAULT_GITHUB_STALE_AFTER_SECS: u64 = 21_600;
const MIN_GITHUB_POLL_SECS: u64 = 15;
const DEFAULT_SPOTIFY_POLL_SECS: u64 = 5;
const DEFAULT_SPOTIFY_REFRESH_TOKEN_PATH: &str = "state/spotify-refresh-token";
const MIN_SPOTIFY_POLL_SECS: u64 = 5;

pub struct AppConfig {
    pub bind_addr: SocketAddr,
    pub discord: DiscordConfig,
    pub github: GitHubConfig,
    pub spotify: Option<SpotifyConfig>,
    pub stale_after: Duration,
    pub state_path: PathBuf,
    pub unavailable_after: Duration,
}

pub struct DiscordConfig {
    pub bot_token: String,
    pub target_guild_id: Id<GuildMarker>,
    pub target_user_id: Id<UserMarker>,
}

pub struct GitHubConfig {
    pub featured_repositories: Vec<String>,
    pub poll_interval: Duration,
    pub stale_after: Duration,
    pub state_path: PathBuf,
    pub token: Option<String>,
    pub username: String,
}

pub struct SpotifyConfig {
    pub client_id: String,
    pub client_secret: String,
    pub poll_interval: Duration,
    pub refresh_token: String,
    pub refresh_token_path: PathBuf,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let bot_token = required_utf8("DISCORD_BOT_TOKEN")?;
        if bot_token.trim().is_empty() {
            return Err(ConfigError::EmptyValue("DISCORD_BOT_TOKEN"));
        }

        let target_user_id = parse_user_id(
            "TARGET_DISCORD_USER_ID",
            required_utf8("TARGET_DISCORD_USER_ID")?,
        )?;
        let target_guild_id = parse_guild_id("TARGET_GUILD_ID", required_utf8("TARGET_GUILD_ID")?)?;
        let bind_addr = optional_utf8("PROFILE_BIND_ADDR")?
            .unwrap_or_else(|| DEFAULT_BIND_ADDR.to_owned())
            .parse()
            .map_err(|_| ConfigError::InvalidSocketAddress("PROFILE_BIND_ADDR"))?;
        let state_path = optional_path("PROFILE_STATE_PATH", DEFAULT_STATE_PATH)?;
        let stale_after_secs =
            optional_seconds("PROFILE_STALE_AFTER_SECS", DEFAULT_STALE_AFTER_SECS)?;
        let unavailable_after_secs = optional_seconds(
            "PROFILE_UNAVAILABLE_AFTER_SECS",
            DEFAULT_UNAVAILABLE_AFTER_SECS,
        )?;
        if stale_after_secs >= unavailable_after_secs {
            return Err(ConfigError::InvalidStaleWindow);
        }

        let github_username = optional_non_empty("GITHUB_USERNAME")?
            .unwrap_or_else(|| DEFAULT_GITHUB_USERNAME.to_owned());
        let github_token = optional_utf8("GITHUB_TOKEN")?
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let github_featured_repositories = optional_utf8("GITHUB_FEATURED_REPOS")?
            .map(|value| parse_csv_list(&value))
            .unwrap_or_default();
        let github_state_path = optional_path("GITHUB_STATE_PATH", DEFAULT_GITHUB_STATE_PATH)?;
        let github_poll_secs = optional_seconds("GITHUB_POLL_SECS", DEFAULT_GITHUB_POLL_SECS)?;
        if github_poll_secs < MIN_GITHUB_POLL_SECS {
            return Err(ConfigError::GitHubPollTooFrequent);
        }
        let github_stale_after_secs =
            optional_seconds("GITHUB_STALE_AFTER_SECS", DEFAULT_GITHUB_STALE_AFTER_SECS)?;
        if github_stale_after_secs <= github_poll_secs {
            return Err(ConfigError::InvalidGitHubStaleWindow);
        }

        let spotify = spotify_config()?;

        Ok(Self {
            bind_addr,
            discord: DiscordConfig {
                bot_token,
                target_guild_id,
                target_user_id,
            },
            github: GitHubConfig {
                featured_repositories: github_featured_repositories,
                poll_interval: Duration::from_secs(github_poll_secs),
                stale_after: Duration::from_secs(github_stale_after_secs),
                state_path: github_state_path,
                token: github_token,
                username: github_username,
            },
            spotify,
            stale_after: Duration::from_secs(stale_after_secs),
            state_path,
            unavailable_after: Duration::from_secs(unavailable_after_secs),
        })
    }
}

fn spotify_config() -> Result<Option<SpotifyConfig>, ConfigError> {
    let client_id = optional_utf8("SPOTIFY_CLIENT_ID")?;
    let client_secret = optional_utf8("SPOTIFY_CLIENT_SECRET")?;
    let refresh_token = optional_utf8("SPOTIFY_REFRESH_TOKEN")?;

    if client_id.is_none() && client_secret.is_none() && refresh_token.is_none() {
        return Ok(None);
    }
    if client_id.is_none() || client_secret.is_none() || refresh_token.is_none() {
        return Err(ConfigError::IncompleteSpotifyConfig);
    }

    let client_id = non_empty_owned("SPOTIFY_CLIENT_ID", client_id.unwrap())?;
    let client_secret = non_empty_owned("SPOTIFY_CLIENT_SECRET", client_secret.unwrap())?;
    let refresh_token = non_empty_owned("SPOTIFY_REFRESH_TOKEN", refresh_token.unwrap())?;
    let poll_secs = optional_seconds("SPOTIFY_POLL_SECS", DEFAULT_SPOTIFY_POLL_SECS)?;
    if poll_secs < MIN_SPOTIFY_POLL_SECS {
        return Err(ConfigError::SpotifyPollTooFrequent);
    }
    let refresh_token_path = optional_path(
        "SPOTIFY_REFRESH_TOKEN_PATH",
        DEFAULT_SPOTIFY_REFRESH_TOKEN_PATH,
    )?;

    Ok(Some(SpotifyConfig {
        client_id,
        client_secret,
        poll_interval: Duration::from_secs(poll_secs),
        refresh_token,
        refresh_token_path,
    }))
}

fn non_empty_owned(name: &'static str, value: String) -> Result<String, ConfigError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        Err(ConfigError::EmptyValue(name))
    } else {
        Ok(value)
    }
}

fn required_utf8(name: &'static str) -> Result<String, ConfigError> {
    let value = env::var_os(name).ok_or(ConfigError::Missing(name))?;
    os_string_into_utf8(name, value)
}

fn optional_utf8(name: &'static str) -> Result<Option<String>, ConfigError> {
    env::var_os(name)
        .map(|value| os_string_into_utf8(name, value))
        .transpose()
}

fn optional_non_empty(name: &'static str) -> Result<Option<String>, ConfigError> {
    optional_utf8(name)?
        .map(|value| non_empty_owned(name, value))
        .transpose()
}

fn optional_path(name: &'static str, default: &str) -> Result<PathBuf, ConfigError> {
    optional_utf8(name)?
        .map(|value| {
            if value.trim().is_empty() {
                Err(ConfigError::EmptyValue(name))
            } else {
                Ok(PathBuf::from(value))
            }
        })
        .transpose()
        .map(|value| value.unwrap_or_else(|| PathBuf::from(default)))
}

fn os_string_into_utf8(name: &'static str, value: OsString) -> Result<String, ConfigError> {
    value.into_string().map_err(|_| ConfigError::NotUtf8(name))
}

fn optional_seconds(name: &'static str, default: u64) -> Result<u64, ConfigError> {
    let Some(value) = optional_utf8(name)? else {
        return Ok(default);
    };
    parse_seconds(name, &value)
}

fn parse_seconds(name: &'static str, value: &str) -> Result<u64, ConfigError> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| ConfigError::InvalidSeconds(name))?;
    if parsed == 0 {
        return Err(ConfigError::InvalidSeconds(name));
    }
    Ok(parsed)
}

fn parse_csv_list(value: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .filter(|entry| seen.insert(entry.to_ascii_lowercase()))
        .map(str::to_owned)
        .collect()
}

fn parse_user_id(name: &'static str, value: String) -> Result<Id<UserMarker>, ConfigError> {
    parse_snowflake(name, value).map(Id::new)
}

fn parse_guild_id(name: &'static str, value: String) -> Result<Id<GuildMarker>, ConfigError> {
    parse_snowflake(name, value).map(Id::new)
}

fn parse_snowflake(name: &'static str, value: String) -> Result<u64, ConfigError> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| ConfigError::InvalidSnowflake(name))?;

    if parsed == 0 {
        return Err(ConfigError::InvalidSnowflake(name));
    }

    Ok(parsed)
}

#[derive(Debug, Eq, PartialEq)]
pub enum ConfigError {
    EmptyValue(&'static str),
    GitHubPollTooFrequent,
    IncompleteSpotifyConfig,
    InvalidGitHubStaleWindow,
    InvalidSnowflake(&'static str),
    InvalidSocketAddress(&'static str),
    InvalidSeconds(&'static str),
    InvalidStaleWindow,
    Missing(&'static str),
    NotUtf8(&'static str),
    SpotifyPollTooFrequent,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyValue(name) => {
                write!(formatter, "environment variable {name} must not be empty")
            }
            Self::GitHubPollTooFrequent => {
                formatter.write_str("GITHUB_POLL_SECS must be at least 15 seconds")
            }
            Self::IncompleteSpotifyConfig => formatter.write_str(
                "SPOTIFY_CLIENT_ID, SPOTIFY_CLIENT_SECRET, and SPOTIFY_REFRESH_TOKEN must be configured together",
            ),
            Self::InvalidGitHubStaleWindow => {
                formatter.write_str("GITHUB_STALE_AFTER_SECS must be greater than GITHUB_POLL_SECS")
            }
            Self::InvalidSnowflake(name) => write!(
                formatter,
                "environment variable {name} must be a non-zero Discord snowflake"
            ),
            Self::InvalidSocketAddress(name) => write!(
                formatter,
                "environment variable {name} must be a valid socket address"
            ),
            Self::InvalidSeconds(name) => write!(
                formatter,
                "environment variable {name} must be a positive integer number of seconds"
            ),
            Self::InvalidStaleWindow => formatter.write_str(
                "PROFILE_STALE_AFTER_SECS must be lower than PROFILE_UNAVAILABLE_AFTER_SECS",
            ),
            Self::Missing(name) => {
                write!(formatter, "required environment variable {name} is missing")
            }
            Self::NotUtf8(name) => {
                write!(formatter, "environment variable {name} must be valid UTF-8")
            }
            Self::SpotifyPollTooFrequent => {
                formatter.write_str("SPOTIFY_POLL_SECS must be at least 5 seconds")
            }
        }
    }
}

impl Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::{
        ConfigError, parse_csv_list, parse_guild_id, parse_seconds, parse_snowflake, parse_user_id,
    };

    #[test]
    fn parses_non_zero_snowflakes() {
        assert_eq!(parse_snowflake("TEST", "123".to_owned()), Ok(123));
        assert_eq!(parse_user_id("TEST", "123".to_owned()).unwrap().get(), 123);
        assert_eq!(parse_guild_id("TEST", "456".to_owned()).unwrap().get(), 456);
    }

    #[test]
    fn rejects_zero_and_non_numeric_snowflakes() {
        assert_eq!(
            parse_snowflake("TEST", "0".to_owned()),
            Err(ConfigError::InvalidSnowflake("TEST"))
        );
        assert_eq!(
            parse_snowflake("TEST", "not-an-id".to_owned()),
            Err(ConfigError::InvalidSnowflake("TEST"))
        );
    }

    #[test]
    fn parses_positive_seconds_only() {
        assert_eq!(parse_seconds("TEST", "120"), Ok(120));
        assert_eq!(
            parse_seconds("TEST", "0"),
            Err(ConfigError::InvalidSeconds("TEST"))
        );
        assert_eq!(
            parse_seconds("TEST", "abc"),
            Err(ConfigError::InvalidSeconds("TEST"))
        );
    }

    #[test]
    fn parses_featured_repository_list_in_order_without_duplicates() {
        assert_eq!(
            parse_csv_list("Graphite-Bot, profile-materials, graphite-bot, , QuestUI"),
            vec!["Graphite-Bot", "profile-materials", "QuestUI"]
        );
    }
}
