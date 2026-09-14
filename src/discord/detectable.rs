use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use reqwest::{Client, StatusCode, redirect::Policy};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::state::{ActivityAssetsSnapshot, PresenceState, RuntimeState};

const RPC_APPLICATION_BASE_URL: &str = "https://discord.com/api/v10/applications";
const ICON_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const NO_ICON_TTL: Duration = Duration::from_secs(60 * 60);
const FAILURE_RETRY: Duration = Duration::from_secs(5 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RESPONSE_BYTES: usize = 128 * 1024;

pub struct DetectableAppCatalog {
    cache: RwLock<CatalogCache>,
    client: Client,
}

#[derive(Default)]
struct CatalogCache {
    icons: HashMap<String, String>,
    retry_after: HashMap<String, Instant>,
    valid_until: HashMap<String, Instant>,
}

#[derive(Deserialize)]
struct RpcApplication {
    id: String,
    #[serde(default)]
    icon: Option<String>,
}

impl DetectableAppCatalog {
    pub fn new() -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .redirect(Policy::none())
            .timeout(HTTP_TIMEOUT)
            .user_agent("profile-materials/0.1 discord-application-icon")
            .build()?;

        Ok(Self {
            cache: RwLock::new(CatalogCache::default()),
            client,
        })
    }

    pub async fn enrich(&self, runtime: &RuntimeState) -> RuntimeState {
        let candidates = missing_application_icons(runtime);
        if candidates.is_empty() {
            return runtime.clone();
        }

        self.refresh_candidates(&candidates).await;
        let cache = self.cache.read().await;
        apply_icons(runtime, &cache.icons, &candidates)
    }

    async fn refresh_candidates(&self, candidates: &HashSet<String>) {
        let now = Instant::now();
        let due = {
            let cache = self.cache.read().await;
            candidates
                .iter()
                .filter(|application_id| {
                    !cache
                        .valid_until
                        .get(*application_id)
                        .is_some_and(|until| *until > now)
                        && !cache
                            .retry_after
                            .get(*application_id)
                            .is_some_and(|until| *until > now)
                })
                .cloned()
                .collect::<Vec<_>>()
        };

        for application_id in due {
            match self.fetch_rpc_icon(&application_id).await {
                Ok(Some(icon_hash)) => {
                    let mut cache = self.cache.write().await;
                    cache.icons.insert(application_id.clone(), icon_hash);
                    cache
                        .valid_until
                        .insert(application_id.clone(), now + ICON_TTL);
                    cache.retry_after.remove(&application_id);
                    tracing::info!(
                        %application_id,
                        "Discord application icon resolved from RPC metadata"
                    );
                }
                Ok(None) => {
                    let mut cache = self.cache.write().await;
                    cache.icons.remove(&application_id);
                    cache
                        .valid_until
                        .insert(application_id.clone(), now + NO_ICON_TTL);
                    cache.retry_after.remove(&application_id);
                    tracing::info!(
                        %application_id,
                        "Discord RPC application metadata has no icon"
                    );
                }
                Err(error) => {
                    let mut cache = self.cache.write().await;
                    cache
                        .retry_after
                        .insert(application_id.clone(), now + FAILURE_RETRY);
                    tracing::warn!(
                        %application_id,
                        %error,
                        stale_icon_available = cache.icons.contains_key(&application_id),
                        "Discord application icon lookup failed; retaining stale cache"
                    );
                }
            }
        }
    }

    async fn fetch_rpc_icon(&self, application_id: &str) -> Result<Option<String>, String> {
        if !valid_application_id(application_id) {
            return Err("invalid application id".to_owned());
        }

        let url = format!("{RPC_APPLICATION_BASE_URL}/{application_id}/rpc");
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| error.to_string())?;

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err("response exceeded size limit".to_owned());
        }

        let bytes = response.bytes().await.map_err(|error| error.to_string())?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err("response exceeded size limit".to_owned());
        }

        let application: RpcApplication =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if application.id != application_id {
            return Err("response application id mismatch".to_owned());
        }

        Ok(application
            .icon
            .as_deref()
            .map(str::trim)
            .filter(|icon_hash| valid_icon_hash(icon_hash))
            .map(str::to_owned))
    }
}

fn missing_application_icons(runtime: &RuntimeState) -> HashSet<String> {
    let PresenceState::Known(snapshot) = &runtime.presence else {
        return HashSet::new();
    };

    snapshot
        .data
        .activities
        .iter()
        .filter(|activity| {
            activity.application_id.is_some()
                && activity
                    .assets
                    .as_ref()
                    .and_then(|assets| assets.large_image.as_deref())
                    .is_none_or(|value| value.trim().is_empty())
        })
        .filter_map(|activity| activity.application_id.clone())
        .collect()
}

fn apply_icons(
    runtime: &RuntimeState,
    icons: &HashMap<String, String>,
    candidates: &HashSet<String>,
) -> RuntimeState {
    let mut enriched = runtime.clone();
    let PresenceState::Known(snapshot) = &mut enriched.presence else {
        return enriched;
    };

    for activity in &mut snapshot.data.activities {
        let Some(application_id) = activity.application_id.as_deref() else {
            continue;
        };
        if !candidates.contains(application_id) {
            continue;
        }
        let Some(icon_hash) = icons.get(application_id) else {
            continue;
        };

        let assets = activity.assets.get_or_insert(ActivityAssetsSnapshot {
            large_image: None,
            large_text: None,
            small_image: None,
            small_text: None,
        });
        if assets
            .large_image
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            assets.large_image = Some(format!("appicon:{icon_hash}"));
        }
    }

    enriched
}

fn valid_application_id(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_icon_hash(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use crate::state::{
        ActivityKind, ActivitySnapshot, ClientStatusSnapshot, GatewayStatus, PresenceData,
        PresenceFreshness, PresenceSnapshot, PresenceState, PresenceStatus, RuntimeState,
    };

    use super::{RpcApplication, apply_icons, valid_application_id, valid_icon_hash};

    fn runtime(large_image: Option<&str>) -> RuntimeState {
        RuntimeState {
            freshness: PresenceFreshness::Fresh,
            gateway_status: GatewayStatus::Live,
            last_target_event_unix_ms: Some(1),
            presence: PresenceState::Known(PresenceSnapshot {
                data: PresenceData {
                    activities: vec![ActivitySnapshot {
                        application_id: Some("1247227126416146462".to_owned()),
                        assets: large_image.map(|value| crate::state::ActivityAssetsSnapshot {
                            large_image: Some(value.to_owned()),
                            large_text: None,
                            small_image: None,
                            small_text: None,
                        }),
                        details: None,
                        kind: ActivityKind::Playing,
                        name: "Wuthering Waves".to_owned(),
                        party: None,
                        state: None,
                        timestamps: None,
                    }],
                    client_status: ClientStatusSnapshot {
                        desktop: Some(PresenceStatus::Online),
                        mobile: None,
                        web: None,
                    },
                    status: PresenceStatus::Online,
                },
                observed_at_unix_ms: 1,
                revision: 1,
            }),
            stream_revision: 1,
            unvalidated_since_unix_ms: None,
        }
    }

    #[test]
    fn enriches_missing_large_artwork_without_overwriting_rpc_artwork() {
        let mut icons = HashMap::new();
        icons.insert(
            "1247227126416146462".to_owned(),
            "1e7d7e9ca69ea0951467994c581f70f5".to_owned(),
        );
        let candidates = HashSet::from(["1247227126416146462".to_owned()]);

        let enriched = apply_icons(&runtime(None), &icons, &candidates);
        let PresenceState::Known(snapshot) = enriched.presence else {
            panic!("known presence");
        };
        assert_eq!(
            snapshot.data.activities[0]
                .assets
                .as_ref()
                .and_then(|assets| assets.large_image.as_deref()),
            Some("appicon:1e7d7e9ca69ea0951467994c581f70f5")
        );

        let existing = apply_icons(&runtime(Some("rpc-artwork")), &icons, &candidates);
        let PresenceState::Known(snapshot) = existing.presence else {
            panic!("known presence");
        };
        assert_eq!(
            snapshot.data.activities[0]
                .assets
                .as_ref()
                .and_then(|assets| assets.large_image.as_deref()),
            Some("rpc-artwork")
        );
    }

    #[test]
    fn parses_rpc_application_icon_shape() {
        let application: RpcApplication = serde_json::from_str(
            r#"{"id":"1247227126416146462","icon":"1e7d7e9ca69ea0951467994c581f70f5"}"#,
        )
        .expect("valid RPC application payload");

        assert_eq!(application.id, "1247227126416146462");
        assert_eq!(
            application.icon.as_deref(),
            Some("1e7d7e9ca69ea0951467994c581f70f5")
        );
        assert!(valid_application_id(&application.id));
        assert!(valid_icon_hash(application.icon.as_deref().expect("icon")));
    }
}
