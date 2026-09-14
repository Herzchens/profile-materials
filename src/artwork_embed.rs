use std::{collections::HashMap, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{StreamExt, future::join_all};
use reqwest::{Client, Url, header::CONTENT_TYPE, redirect::Policy};
use tokio::sync::Mutex;

const MAX_ARTWORK_BYTES: usize = 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 64;

pub struct ArtworkEmbedder {
    cache: Mutex<HashMap<String, Arc<str>>>,
    client: Client,
}

pub struct EmbeddedArtworkBatch {
    complete: bool,
    images: HashMap<String, Arc<str>>,
}

impl Default for EmbeddedArtworkBatch {
    fn default() -> Self {
        Self {
            complete: true,
            images: HashMap::new(),
        }
    }
}

impl EmbeddedArtworkBatch {
    pub const fn complete(&self) -> bool {
        self.complete
    }

    pub fn href(&self, url: &str) -> Option<&str> {
        self.images.get(url).map(AsRef::as_ref)
    }
}

impl ArtworkEmbedder {
    pub fn new() -> Result<Self, reqwest::Error> {
        ensure_crypto_provider();

        let client = Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(5))
            .user_agent("profile-materials/0.1 artwork-embedder")
            .build()?;

        Ok(Self {
            cache: Mutex::new(HashMap::new()),
            client,
        })
    }

    pub async fn embed_all(&self, urls: &[String]) -> EmbeddedArtworkBatch {
        if urls.is_empty() {
            return EmbeddedArtworkBatch::default();
        }

        let results = join_all(urls.iter().map(|url| self.embed(url))).await;
        let mut batch = EmbeddedArtworkBatch::default();

        for (url, embedded) in urls.iter().zip(results) {
            match embedded {
                Some(data_uri) => {
                    batch.images.insert(url.clone(), data_uri);
                }
                None => {
                    batch.complete = false;
                }
            }
        }

        batch
    }

    async fn embed(&self, raw_url: &str) -> Option<Arc<str>> {
        if let Some(cached) = self.cache.lock().await.get(raw_url).cloned() {
            return Some(cached);
        }

        let url = trusted_artwork_url(raw_url)?;
        let host = url.host_str().unwrap_or("unknown").to_owned();

        let response = match self.client.get(url).send().await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(
                    %host,
                    error = %error,
                    "artwork fetch failed; renderer will use local fallback"
                );
                return None;
            }
        };

        if !response.status().is_success() {
            tracing::warn!(
                %host,
                status = %response.status(),
                "artwork origin returned a non-success response"
            );
            return None;
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(supported_content_type)?;

        let declared_length = response.content_length();
        if declared_length.is_some_and(|length| length > MAX_ARTWORK_BYTES as u64) {
            tracing::warn!(
                %host,
                ?declared_length,
                "artwork exceeded the embedding size limit"
            );
            return None;
        }

        let capacity = declared_length
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0)
            .min(MAX_ARTWORK_BYTES);
        let mut bytes = Vec::with_capacity(capacity);
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    tracing::warn!(
                        %host,
                        error = %error,
                        "artwork body could not be read"
                    );
                    return None;
                }
            };

            if bytes.len().saturating_add(chunk.len()) > MAX_ARTWORK_BYTES {
                tracing::warn!(
                    %host,
                    "artwork exceeded the embedding size limit while streaming"
                );
                return None;
            }

            bytes.extend_from_slice(&chunk);
        }

        if bytes.is_empty() {
            return None;
        }

        let data_uri: Arc<str> = Arc::from(format!(
            "data:{content_type};base64,{}",
            STANDARD.encode(bytes)
        ));

        let mut cache = self.cache.lock().await;
        if cache.len() >= MAX_CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert(raw_url.to_owned(), Arc::clone(&data_uri));

        Some(data_uri)
    }
}

fn ensure_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

fn trusted_artwork_url(raw_url: &str) -> Option<Url> {
    let url = Url::parse(raw_url).ok()?;

    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
    {
        return None;
    }

    let host = url.host_str()?;
    let path = url.path();

    let trusted = match host {
        "cdn.discordapp.com" => {
            path.starts_with("/app-assets/")
                || path.starts_with("/app-icons/")
                || path.starts_with("/avatars/")
                || path.starts_with("/avatar-decoration-presets/")
                || path.starts_with("/guild-tag-badges/")
                || path.starts_with("/embed/avatars/")
        }
        "media.discordapp.net" => !path.is_empty(),
        "i.scdn.co" => path.starts_with("/image/"),
        _ => false,
    };

    trusted.then_some(url)
}

fn supported_content_type(raw: &str) -> Option<&'static str> {
    match raw
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/png" => Some("image/png"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/webp" => Some("image/webp"),
        "image/gif" => Some("image/gif"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{EmbeddedArtworkBatch, supported_content_type, trusted_artwork_url};

    #[test]
    fn only_known_https_artwork_origins_are_allowed() {
        assert!(trusted_artwork_url("https://cdn.discordapp.com/app-assets/123/456.png").is_some());
        assert!(
            trusted_artwork_url("https://cdn.discordapp.com/app-icons/123/abcdef.png?size=512")
                .is_some()
        );
        assert!(
            trusted_artwork_url("https://cdn.discordapp.com/avatars/123/hash.webp?size=256")
                .is_some()
        );
        assert!(
            trusted_artwork_url(
                "https://cdn.discordapp.com/avatar-decoration-presets/a_hash.png?size=256"
            )
            .is_some()
        );
        assert!(
            trusted_artwork_url("https://cdn.discordapp.com/guild-tag-badges/123/hash.png?size=64")
                .is_some()
        );
        assert!(trusted_artwork_url("https://cdn.discordapp.com/embed/avatars/2.png").is_some());
        assert!(
            trusted_artwork_url(
                "https://media.discordapp.net/external/hash/https/example.invalid/icon.png"
            )
            .is_some()
        );
        assert!(trusted_artwork_url("https://i.scdn.co/image/abc123").is_some());

        assert!(trusted_artwork_url("http://i.scdn.co/image/abc123").is_none());
        assert!(trusted_artwork_url("https://example.com/icon.png").is_none());
        assert!(
            trusted_artwork_url("https://cdn.discordapp.com@127.0.0.1/app-assets/1/2.png")
                .is_none()
        );
        assert!(
            trusted_artwork_url("https://cdn.discordapp.com:8443/app-assets/1/2.png").is_none()
        );
    }

    #[test]
    fn only_raster_image_types_are_embedded() {
        assert_eq!(supported_content_type("image/png"), Some("image/png"));
        assert_eq!(
            supported_content_type("image/jpeg; charset=binary"),
            Some("image/jpeg")
        );
        assert_eq!(supported_content_type("image/webp"), Some("image/webp"));
        assert_eq!(supported_content_type("image/svg+xml"), None);
        assert_eq!(supported_content_type("text/html"), None);
    }

    #[test]
    fn empty_batch_is_complete() {
        assert!(EmbeddedArtworkBatch::default().complete());
    }
}
