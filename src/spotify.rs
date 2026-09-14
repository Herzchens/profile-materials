use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use arc_swap::ArcSwapOption;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use reqwest::{Client, StatusCode, header::RETRY_AFTER, redirect::Policy};
use serde::{Deserialize, Serialize};
use tokio::{fs, sync::watch, time::Instant};

use crate::{clock, config::SpotifyConfig};

const SPOTIFY_TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const SPOTIFY_CURRENTLY_PLAYING_URL: &str =
    "https://api.spotify.com/v1/me/player/currently-playing?additional_types=track";
const ACCESS_TOKEN_REFRESH_SKEW: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_CONFIRMATION_POLLS: u8 = 2;
const ACTIVE_TRACK_GRACE_MS: u64 = 15_000;
const PLAYBACK_LKG_FILE_NAME: &str = "spotify-playback.json";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpotifyPlayback {
    pub album: Option<String>,
    pub artists: String,
    pub cover_url: Option<String>,
    pub duration_ms: u64,
    pub is_playing: bool,
    pub progress_ms: u64,
    pub title: String,
    pub track_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SpotifyPlaybackState {
    Idle,
    Track(SpotifyPlayback),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpotifySnapshot {
    pub playback: SpotifyPlaybackState,
    pub revision: u64,
    pub validated_at_unix_ms: u64,
}

pub struct SpotifyStore {
    current: ArcSwapOption<SpotifySnapshot>,
    updates: watch::Sender<u64>,
}

impl SpotifyStore {
    pub fn new() -> Self {
        let (updates, _) = watch::channel(0);
        Self {
            current: ArcSwapOption::from(None),
            updates,
        }
    }

    pub fn load(&self) -> Option<Arc<SpotifySnapshot>> {
        self.current.load_full()
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.updates.subscribe()
    }

    fn restore(&self, snapshot: SpotifySnapshot) {
        let revision = snapshot.revision;
        self.current.store(Some(Arc::new(snapshot)));
        self.updates.send_replace(revision);
    }

    fn publish(&self, playback: SpotifyPlaybackState, validated_at_unix_ms: u64) -> bool {
        let current = self.current.load_full();
        let revision = match current.as_deref() {
            Some(current) if current.playback == playback => current.revision,
            Some(current) => current.revision.saturating_add(1),
            None => 1,
        };
        let changed = current
            .as_deref()
            .is_none_or(|current| current.playback != playback);

        self.current.store(Some(Arc::new(SpotifySnapshot {
            playback,
            revision,
            validated_at_unix_ms,
        })));

        if changed {
            self.updates.send_replace(revision);
        }
        changed
    }
}

struct AccessToken {
    expires_at: Instant,
    value: String,
}

#[derive(Default)]
struct PlaybackDebouncer {
    consecutive_idle: u8,
    has_active_track: bool,
}

impl PlaybackDebouncer {
    fn observe(&mut self, playback: SpotifyPlaybackState) -> Option<SpotifyPlaybackState> {
        match playback {
            SpotifyPlaybackState::Track(_) => {
                self.consecutive_idle = 0;
                self.has_active_track = true;
                Some(playback)
            }
            SpotifyPlaybackState::Idle if !self.has_active_track => {
                self.consecutive_idle = 0;
                Some(SpotifyPlaybackState::Idle)
            }
            SpotifyPlaybackState::Idle => {
                self.consecutive_idle = self.consecutive_idle.saturating_add(1);
                if self.consecutive_idle >= IDLE_CONFIRMATION_POLLS {
                    self.consecutive_idle = 0;
                    self.has_active_track = false;
                    Some(SpotifyPlaybackState::Idle)
                } else {
                    None
                }
            }
        }
    }
}

async fn publish_observation(
    store: &SpotifyStore,
    debouncer: &mut PlaybackDebouncer,
    playback: SpotifyPlaybackState,
    validated_at_unix_ms: u64,
    playback_lkg_path: &Path,
    source: &'static str,
) {
    let Some(playback) = debouncer.observe(playback) else {
        tracing::debug!(source, "ignoring transient Spotify idle observation");
        return;
    };

    let previous = store.load();
    let persist_track = should_persist_track(previous.as_deref(), &playback);
    let clear_lkg = matches!(playback, SpotifyPlaybackState::Idle)
        && previous
            .as_deref()
            .is_some_and(|snapshot| matches!(&snapshot.playback, SpotifyPlaybackState::Track(_)));

    let changed = store.publish(playback, validated_at_unix_ms);
    if changed {
        tracing::info!(source, "native Spotify playback changed");
    }

    if persist_track {
        if let Some(snapshot) = store.load()
            && let Err(error) = persist_playback_lkg(playback_lkg_path, snapshot.as_ref()).await
        {
            tracing::warn!(
                path = %playback_lkg_path.display(),
                error = %error,
                "Spotify playback LKG could not be persisted"
            );
        }
    } else if clear_lkg && let Err(error) = clear_playback_lkg(playback_lkg_path).await {
        tracing::warn!(
            path = %playback_lkg_path.display(),
            error = %error,
            "Spotify playback LKG could not be cleared"
        );
    }
}

pub async fn run_optional(
    config: Option<SpotifyConfig>,
    store: Arc<SpotifyStore>,
) -> Result<(), SpotifyError> {
    let Some(config) = config else {
        tracing::info!("native Spotify collector disabled");
        std::future::pending::<()>().await;
        unreachable!();
    };

    run(config, store).await
}

async fn run(config: SpotifyConfig, store: Arc<SpotifyStore>) -> Result<(), SpotifyError> {
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .user_agent("profile-materials/0.1 spotify-playback")
        .build()
        .map_err(SpotifyError::Http)?;
    let mut refresh_token = load_refresh_token(&config).await?;
    let playback_lkg_path = playback_lkg_path(&config);
    let mut access_token: Option<AccessToken> = None;
    let mut debouncer = PlaybackDebouncer::default();

    match load_playback_lkg(&playback_lkg_path).await {
        Ok(Some(snapshot)) => {
            let now_unix_ms = clock::unix_time_millis()
                .map_err(|error| SpotifyError::Clock(error.to_string()))?;
            if let Some(snapshot) = advance_active_snapshot(snapshot, now_unix_ms) {
                debouncer.has_active_track = true;
                let revision = snapshot.revision;
                store.restore(snapshot);
                tracing::info!(
                    revision,
                    path = %playback_lkg_path.display(),
                    "restored active native Spotify playback LKG"
                );
            } else if let Err(error) = clear_playback_lkg(&playback_lkg_path).await {
                tracing::warn!(
                    path = %playback_lkg_path.display(),
                    error = %error,
                    "expired Spotify playback LKG could not be cleared"
                );
            }
        }
        Ok(None) => {}
        Err(error) => tracing::warn!(
            path = %playback_lkg_path.display(),
            error = %error,
            "Spotify playback LKG could not be restored"
        ),
    }

    let mut interval = tokio::time::interval(config.poll_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    tracing::info!(
        poll_interval_secs = config.poll_interval.as_secs(),
        "native Spotify collector starting"
    );

    loop {
        interval.tick().await;

        if access_token.as_ref().is_none_or(|token| {
            token.expires_at.saturating_duration_since(Instant::now()) <= ACCESS_TOKEN_REFRESH_SKEW
        }) {
            match refresh_access_token(&client, &config, &mut refresh_token).await {
                Ok(token) => access_token = Some(token),
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "Spotify token refresh failed; native Spotify is temporarily unavailable"
                    );
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    continue;
                }
            }
        }

        let token = access_token
            .as_ref()
            .expect("access token exists after refresh");
        match fetch_playback(&client, &token.value).await {
            Ok(playback) => {
                let validated_at_unix_ms = clock::unix_time_millis()
                    .map_err(|error| SpotifyError::Clock(error.to_string()))?;
                publish_observation(
                    &store,
                    &mut debouncer,
                    playback,
                    validated_at_unix_ms,
                    &playback_lkg_path,
                    "poll",
                )
                .await;
            }
            Err(SpotifyError::Unauthorized) => {
                match refresh_access_token(&client, &config, &mut refresh_token).await {
                    Ok(token) => access_token = Some(token),
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "Spotify token refresh failed after 401; retaining the last successful state"
                        );
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        continue;
                    }
                }
                let token = access_token
                    .as_ref()
                    .expect("access token exists after forced refresh");
                match fetch_playback(&client, &token.value).await {
                    Ok(playback) => {
                        let validated_at_unix_ms = clock::unix_time_millis()
                            .map_err(|error| SpotifyError::Clock(error.to_string()))?;
                        publish_observation(
                            &store,
                            &mut debouncer,
                            playback,
                            validated_at_unix_ms,
                            &playback_lkg_path,
                            "post_refresh",
                        )
                        .await;
                    }
                    Err(error) => tracing::warn!(
                        error = %error,
                        "Spotify playback retry failed; retaining the last successful state"
                    ),
                }
            }
            Err(SpotifyError::RateLimited(retry_after)) => {
                tracing::warn!(
                    retry_after_secs = retry_after.as_secs(),
                    "Spotify API rate limited"
                );
                tokio::time::sleep(retry_after).await;
            }
            Err(error) => {
                tracing::warn!(error = %error, "Spotify playback poll failed; retaining last successful state");
            }
        }
    }
}

fn advance_active_snapshot(
    mut snapshot: SpotifySnapshot,
    now_unix_ms: u64,
) -> Option<SpotifySnapshot> {
    let SpotifyPlaybackState::Track(track) = &mut snapshot.playback else {
        return None;
    };
    if !track.is_playing || track.duration_ms == 0 {
        return None;
    }

    let elapsed_since_validation = now_unix_ms.saturating_sub(snapshot.validated_at_unix_ms);
    let remaining_at_validation = track.duration_ms.saturating_sub(track.progress_ms);
    if elapsed_since_validation > remaining_at_validation.saturating_add(ACTIVE_TRACK_GRACE_MS) {
        return None;
    }

    track.progress_ms = track
        .progress_ms
        .saturating_add(elapsed_since_validation)
        .min(track.duration_ms);
    snapshot.validated_at_unix_ms = now_unix_ms;
    Some(snapshot)
}

fn should_persist_track(current: Option<&SpotifySnapshot>, next: &SpotifyPlaybackState) -> bool {
    let SpotifyPlaybackState::Track(next) = next else {
        return false;
    };

    match current.map(|snapshot| &snapshot.playback) {
        Some(SpotifyPlaybackState::Track(current)) => !same_track_identity(current, next),
        Some(SpotifyPlaybackState::Idle) | None => true,
    }
}

fn same_track_identity(left: &SpotifyPlayback, right: &SpotifyPlayback) -> bool {
    match (left.track_id.as_deref(), right.track_id.as_deref()) {
        (Some(left), Some(right)) => left == right,
        _ => {
            left.title == right.title
                && left.artists == right.artists
                && left.duration_ms == right.duration_ms
        }
    }
}

fn playback_lkg_path(config: &SpotifyConfig) -> PathBuf {
    config
        .refresh_token_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.join(PLAYBACK_LKG_FILE_NAME))
        .unwrap_or_else(|| PathBuf::from(PLAYBACK_LKG_FILE_NAME))
}

async fn load_playback_lkg(path: &Path) -> Result<Option<SpotifySnapshot>, SpotifyError> {
    match fs::read_to_string(path).await {
        Ok(body) => serde_json::from_str(&body)
            .map(Some)
            .map_err(SpotifyError::Decode),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(SpotifyError::Io(error)),
    }
}

async fn persist_playback_lkg(path: &Path, snapshot: &SpotifySnapshot) -> Result<(), SpotifyError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).await.map_err(SpotifyError::Io)?;
    }

    let body = serde_json::to_vec(snapshot).map_err(SpotifyError::Encode)?;
    let temp = path.with_extension("tmp");
    fs::write(&temp, body).await.map_err(SpotifyError::Io)?;
    fs::rename(&temp, path).await.map_err(SpotifyError::Io)
}

async fn clear_playback_lkg(path: &Path) -> Result<(), SpotifyError> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SpotifyError::Io(error)),
    }
}

async fn load_refresh_token(config: &SpotifyConfig) -> Result<String, SpotifyError> {
    match fs::read_to_string(&config.refresh_token_path).await {
        Ok(token) => {
            let token = token.trim();
            if token.is_empty() {
                Ok(config.refresh_token.clone())
            } else {
                Ok(token.to_owned())
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(config.refresh_token.clone())
        }
        Err(error) => Err(SpotifyError::Io(error)),
    }
}

async fn refresh_access_token(
    client: &Client,
    config: &SpotifyConfig,
    refresh_token: &mut String,
) -> Result<AccessToken, SpotifyError> {
    let basic = BASE64_STANDARD.encode(format!("{}:{}", config.client_id, config.client_secret));
    let body = format!(
        "grant_type=refresh_token&refresh_token={}",
        form_component(refresh_token)
    );
    let response = client
        .post(SPOTIFY_TOKEN_URL)
        .header("Authorization", format!("Basic {basic}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(SpotifyError::Http)?;

    let status = response.status();
    let body = response.text().await.map_err(SpotifyError::Http)?;
    if !status.is_success() {
        return Err(SpotifyError::TokenStatus(status.as_u16()));
    }

    let token: TokenResponse = serde_json::from_str(&body).map_err(SpotifyError::Decode)?;
    if token.access_token.trim().is_empty() || token.expires_in == 0 {
        return Err(SpotifyError::InvalidTokenResponse);
    }

    if let Some(rotated) = token
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && rotated != refresh_token
    {
        persist_refresh_token(&config.refresh_token_path, rotated).await?;
        *refresh_token = rotated.to_owned();
    }

    Ok(AccessToken {
        expires_at: Instant::now() + Duration::from_secs(token.expires_in),
        value: token.access_token,
    })
}

async fn persist_refresh_token(path: &Path, token: &str) -> Result<(), SpotifyError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).await.map_err(SpotifyError::Io)?;
    }

    let temp = path.with_extension("tmp");
    fs::write(&temp, format!("{token}\n"))
        .await
        .map_err(SpotifyError::Io)?;
    fs::rename(&temp, path).await.map_err(SpotifyError::Io)
}

async fn fetch_playback(
    client: &Client,
    access_token: &str,
) -> Result<SpotifyPlaybackState, SpotifyError> {
    let response = client
        .get(SPOTIFY_CURRENTLY_PLAYING_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(SpotifyError::Http)?;

    match response.status() {
        StatusCode::NO_CONTENT => Ok(SpotifyPlaybackState::Idle),
        StatusCode::UNAUTHORIZED => Err(SpotifyError::Unauthorized),
        StatusCode::TOO_MANY_REQUESTS => {
            let retry_after = retry_after(&response);
            Err(SpotifyError::RateLimited(retry_after))
        }
        status if status.is_success() => {
            let body = response.text().await.map_err(SpotifyError::Http)?;
            let payload: CurrentlyPlayingResponse =
                serde_json::from_str(&body).map_err(SpotifyError::Decode)?;
            Ok(payload.into_playback())
        }
        status => Err(SpotifyError::PlaybackStatus(status.as_u16())),
    }
}

fn retry_after(response: &reqwest::Response) -> Duration {
    let retry_after = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(5);
    Duration::from_secs(retry_after.max(1))
}

impl CurrentlyPlayingResponse {
    fn into_playback(self) -> SpotifyPlaybackState {
        if !self.is_playing {
            return SpotifyPlaybackState::Idle;
        }
        let Some(item) = self.item else {
            return SpotifyPlaybackState::Idle;
        };

        SpotifyPlaybackState::Track(track_item_into_playback(
            item,
            true,
            self.progress_ms.unwrap_or(0),
        ))
    }
}

fn track_item_into_playback(
    item: TrackItem,
    is_playing: bool,
    progress_ms: u64,
) -> SpotifyPlayback {
    let artists = item
        .artists
        .into_iter()
        .map(|artist| artist.name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let cover_url = item.album.as_ref().and_then(|album| {
        album
            .images
            .iter()
            .max_by_key(|image| {
                image
                    .width
                    .unwrap_or(0)
                    .saturating_mul(image.height.unwrap_or(0))
            })
            .map(|image| image.url.clone())
    });
    let album = item
        .album
        .as_ref()
        .map(|album| album.name.trim().to_owned())
        .filter(|name| !name.is_empty());

    SpotifyPlayback {
        album,
        artists,
        cover_url,
        duration_ms: item.duration_ms,
        is_playing,
        progress_ms: progress_ms.min(item.duration_ms),
        title: item.name.trim().to_owned(),
        track_id: item.id,
    }
}

fn form_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CurrentlyPlayingResponse {
    is_playing: bool,
    item: Option<TrackItem>,
    progress_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TrackItem {
    album: Option<Album>,
    #[serde(default)]
    artists: Vec<Artist>,
    duration_ms: u64,
    id: Option<String>,
    name: String,
}

#[derive(Debug, Deserialize)]
struct Album {
    #[serde(default)]
    images: Vec<Image>,
    name: String,
}

#[derive(Debug, Deserialize)]
struct Artist {
    name: String,
}

#[derive(Debug, Deserialize)]
struct Image {
    height: Option<u64>,
    url: String,
    width: Option<u64>,
}

#[derive(Debug)]
pub enum SpotifyError {
    Clock(String),
    Decode(serde_json::Error),
    Encode(serde_json::Error),
    Http(reqwest::Error),
    InvalidTokenResponse,
    Io(std::io::Error),
    PlaybackStatus(u16),
    RateLimited(Duration),
    TokenStatus(u16),
    Unauthorized,
}

impl fmt::Display for SpotifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clock(error) => write!(formatter, "Spotify clock error: {error}"),
            Self::Decode(error) => write!(formatter, "Spotify response was invalid JSON: {error}"),
            Self::Encode(error) => write!(
                formatter,
                "Spotify playback state could not be encoded: {error}"
            ),
            Self::Http(error) => write!(formatter, "Spotify HTTP request failed: {error}"),
            Self::InvalidTokenResponse => {
                formatter.write_str("Spotify token response was incomplete")
            }
            Self::Io(error) => write!(formatter, "Spotify state I/O failed: {error}"),
            Self::PlaybackStatus(status) => {
                write!(
                    formatter,
                    "Spotify playback endpoint returned HTTP {status}"
                )
            }
            Self::RateLimited(duration) => write!(
                formatter,
                "Spotify API rate limited requests for {} seconds",
                duration.as_secs()
            ),
            Self::TokenStatus(status) => {
                write!(formatter, "Spotify token endpoint returned HTTP {status}")
            }
            Self::Unauthorized => formatter.write_str("Spotify access token was unauthorized"),
        }
    }
}

impl Error for SpotifyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Decode(error) | Self::Encode(error) => Some(error),
            Self::Http(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Clock(_)
            | Self::InvalidTokenResponse
            | Self::PlaybackStatus(_)
            | Self::RateLimited(_)
            | Self::TokenStatus(_)
            | Self::Unauthorized => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CurrentlyPlayingResponse, SpotifyPlayback, SpotifyPlaybackState, SpotifySnapshot,
        advance_active_snapshot, form_component,
    };

    fn active_track(progress_ms: u64) -> SpotifyPlaybackState {
        SpotifyPlaybackState::Track(SpotifyPlayback {
            album: Some("Album".to_owned()),
            artists: "Artist".to_owned(),
            cover_url: None,
            duration_ms: 180_000,
            is_playing: true,
            progress_ms,
            title: "Track".to_owned(),
            track_id: Some("track-1".to_owned()),
        })
    }

    #[test]
    fn form_encoding_is_safe_for_refresh_tokens() {
        assert_eq!(form_component("abc+def/ghi="), "abc%2Bdef%2Fghi%3D");
    }

    #[test]
    fn converts_current_track_without_exposing_provider_payload() {
        let payload: CurrentlyPlayingResponse = serde_json::from_str(
            r#"{
                "is_playing": true,
                "progress_ms": 42000,
                "item": {
                    "id": "track-1",
                    "name": "Half Asleep",
                    "duration_ms": 180000,
                    "artists": [{"name": "Meyo"}],
                    "album": {
                        "name": "Half Asleep",
                        "images": [
                            {"url":"https://i.scdn.co/image/small","width":64,"height":64},
                            {"url":"https://i.scdn.co/image/large","width":640,"height":640}
                        ]
                    }
                }
            }"#,
        )
        .unwrap();

        let SpotifyPlaybackState::Track(track) = payload.into_playback() else {
            panic!("expected track");
        };
        assert_eq!(track.title, "Half Asleep");
        assert_eq!(track.artists, "Meyo");
        assert_eq!(track.album.as_deref(), Some("Half Asleep"));
        assert_eq!(
            track.cover_url.as_deref(),
            Some("https://i.scdn.co/image/large")
        );
        assert_eq!(track.progress_ms, 42_000);
        assert!(track.is_playing);
    }

    #[test]
    fn paused_track_is_idle_even_when_spotify_returns_track_metadata() {
        let payload: CurrentlyPlayingResponse = serde_json::from_str(
            r#"{
                "is_playing": false,
                "progress_ms": 42000,
                "item": {
                    "id": "track-1",
                    "name": "Paused Track",
                    "duration_ms": 180000,
                    "artists": [{"name": "Artist"}],
                    "album": {"name": "Album", "images": []}
                }
            }"#,
        )
        .unwrap();

        assert_eq!(payload.into_playback(), SpotifyPlaybackState::Idle);
    }

    #[test]
    fn missing_item_is_idle() {
        let payload: CurrentlyPlayingResponse =
            serde_json::from_str(r#"{"is_playing":false,"progress_ms":null,"item":null}"#).unwrap();
        assert_eq!(payload.into_playback(), SpotifyPlaybackState::Idle);
    }

    #[test]
    fn active_track_lkg_advances_across_a_short_restart() {
        let snapshot = SpotifySnapshot {
            playback: active_track(42_000),
            revision: 7,
            validated_at_unix_ms: 1_000,
        };
        let restored = advance_active_snapshot(snapshot, 11_000).expect("active LKG");
        let SpotifyPlaybackState::Track(track) = restored.playback else {
            panic!("expected track");
        };
        assert_eq!(track.progress_ms, 52_000);
        assert_eq!(restored.validated_at_unix_ms, 11_000);
    }

    #[test]
    fn active_track_lkg_expires_after_predicted_end_and_grace() {
        let snapshot = SpotifySnapshot {
            playback: active_track(170_000),
            revision: 7,
            validated_at_unix_ms: 1_000,
        };
        assert!(advance_active_snapshot(snapshot, 27_000).is_none());
    }
}
