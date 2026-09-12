use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::state::RuntimeState;

const HERO_RENDERER_REVISION: u8 = 3;
const HERO_WIDTH: u32 = 2048;
const HERO_HEIGHT: u32 = 682;
const HERO_ART: &[u8] = include_bytes!("../assets/hero/banner.png");

#[derive(Debug)]
pub struct HeroDocument {
    body: String,
    etag: String,
    stream_revision: u64,
}

impl HeroDocument {
    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn etag(&self) -> &str {
        &self.etag
    }

    pub const fn stream_revision(&self) -> u64 {
        self.stream_revision
    }
}

pub struct HeroRenderer {
    document: Arc<HeroDocument>,
}

impl HeroRenderer {
    pub fn new() -> Result<Self, reqwest::Error> {
        let background_href = format!("data:image/png;base64,{}", STANDARD.encode(HERO_ART));
        let body = render_hero(&background_href);

        Ok(Self {
            document: Arc::new(HeroDocument {
                body,
                etag: format!("\"profile-hero-v{HERO_RENDERER_REVISION}-static\""),
                stream_revision: 0,
            }),
        })
    }

    pub async fn render(&self, _runtime: &RuntimeState) -> Arc<HeroDocument> {
        Arc::clone(&self.document)
    }
}

fn render_hero(background_href: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{HERO_WIDTH}" height="{HERO_HEIGHT}" viewBox="0 0 {HERO_WIDTH} {HERO_HEIGHT}" role="img" aria-labelledby="hero-title hero-desc"><title id="hero-title">ItzHerzchen profile hero</title><desc id="hero-desc">Final authored ItzHerzchen profile banner.</desc><image href="{background_href}" x="0" y="0" width="{HERO_WIDTH}" height="{HERO_HEIGHT}" preserveAspectRatio="xMidYMid meet"/></svg>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hero_preserves_only_the_authored_banner() {
        let renderer = HeroRenderer::new().expect("hero renderer should initialize");
        let body = renderer.document.body();

        assert!(body.contains("data:image/png;base64,"));
        assert!(!body.contains("OFF THE RADAR"));
        assert!(!body.contains("DISCORD"));
        assert!(!body.contains("CURRENT ACTIVITY"));
        assert!(!body.contains("LISTENING ON SPOTIFY"));
    }
}
