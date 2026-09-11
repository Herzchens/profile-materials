use std::{env, error::Error, ffi::OsString, fmt, net::SocketAddr};

use twilight_model::id::{
    Id,
    marker::{GuildMarker, UserMarker},
};

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:3000";

pub struct AppConfig {
    pub bind_addr: SocketAddr,
    pub discord: DiscordConfig,
}

pub struct DiscordConfig {
    pub bot_token: String,
    pub target_guild_id: Id<GuildMarker>,
    pub target_user_id: Id<UserMarker>,
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

        Ok(Self {
            bind_addr,
            discord: DiscordConfig {
                bot_token,
                target_guild_id,
                target_user_id,
            },
        })
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

fn os_string_into_utf8(name: &'static str, value: OsString) -> Result<String, ConfigError> {
    value.into_string().map_err(|_| ConfigError::NotUtf8(name))
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
    InvalidSnowflake(&'static str),
    InvalidSocketAddress(&'static str),
    Missing(&'static str),
    NotUtf8(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyValue(name) => {
                write!(formatter, "environment variable {name} must not be empty")
            }
            Self::InvalidSnowflake(name) => write!(
                formatter,
                "environment variable {name} must be a non-zero Discord snowflake"
            ),
            Self::InvalidSocketAddress(name) => write!(
                formatter,
                "environment variable {name} must be a valid socket address"
            ),
            Self::Missing(name) => {
                write!(formatter, "required environment variable {name} is missing")
            }
            Self::NotUtf8(name) => {
                write!(formatter, "environment variable {name} must be valid UTF-8")
            }
        }
    }
}

impl Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::{ConfigError, parse_guild_id, parse_snowflake, parse_user_id};

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
}
