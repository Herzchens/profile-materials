use std::time::Duration;

use crate::github::stats::GitHubSnapshot;

#[path = "cards_legacy.rs"]
mod legacy;
#[path = "stats_card.rs"]
mod stats_card;

// Keep the proven language and streak renderers unchanged. The legacy module's
// `super::stats` import resolves through this shim.
mod stats {
    pub use crate::github::stats::*;
}

pub use legacy::GitHubCardKind;

pub struct GitHubCardDocument {
    body: String,
    etag: String,
    revision: u64,
}

impl GitHubCardDocument {
    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn etag(&self) -> &str {
        &self.etag
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

pub fn render_card(
    kind: GitHubCardKind,
    snapshot: Option<&GitHubSnapshot>,
    now_unix_ms: u64,
    stale_after: Duration,
) -> GitHubCardDocument {
    match kind {
        GitHubCardKind::Stats => {
            let rendered = stats_card::render(snapshot, now_unix_ms, stale_after);
            GitHubCardDocument {
                body: rendered.body,
                etag: rendered.etag,
                revision: rendered.revision,
            }
        }
        GitHubCardKind::Languages | GitHubCardKind::Streak => {
            let rendered = legacy::render_card(kind, snapshot, now_unix_ms, stale_after);
            GitHubCardDocument {
                body: rendered.body().to_owned(),
                etag: rendered.etag().to_owned(),
                revision: rendered.revision(),
            }
        }
    }
}
