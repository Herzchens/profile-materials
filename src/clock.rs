use std::{
    error::Error,
    fmt,
    time::{SystemTime, SystemTimeError, UNIX_EPOCH},
};

pub fn unix_time_millis() -> Result<u64, ClockError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(ClockError::BeforeUnixEpoch)?;

    u64::try_from(elapsed.as_millis()).map_err(|_| ClockError::TimestampOverflow)
}

#[derive(Debug)]
pub enum ClockError {
    BeforeUnixEpoch(SystemTimeError),
    TimestampOverflow,
}

impl fmt::Display for ClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BeforeUnixEpoch(_) => {
                formatter.write_str("system clock is before the Unix epoch")
            }
            Self::TimestampOverflow => {
                formatter.write_str("current Unix timestamp does not fit in u64 milliseconds")
            }
        }
    }
}

impl Error for ClockError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BeforeUnixEpoch(error) => Some(error),
            Self::TimestampOverflow => None,
        }
    }
}
