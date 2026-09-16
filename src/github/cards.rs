use std::time::Duration;

use crate::github::stats::GitHubSnapshot;

#[path = "cards_legacy.rs"]
mod legacy;
#[path = "stats_card.rs"]
mod stats_card;

const STREAK_PERF_REVISION: u8 = 1;

// Keep the proven language renderer unchanged. The legacy module's
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
        GitHubCardKind::Languages => {
            let rendered = legacy::render_card(kind, snapshot, now_unix_ms, stale_after);
            GitHubCardDocument {
                body: rendered.body().to_owned(),
                etag: rendered.etag().to_owned(),
                revision: rendered.revision(),
            }
        }
        GitHubCardKind::Streak => {
            let rendered = legacy::render_card(kind, snapshot, now_unix_ms, stale_after);
            GitHubCardDocument {
                body: optimize_streak_for_mobile(rendered.body()),
                etag: append_streak_perf_etag(rendered.etag()),
                revision: rendered.revision(),
            }
        }
    }
}

fn optimize_streak_for_mobile(svg: &str) -> String {
    const PERF_STYLE: &str = r#"<style id="streak-mobile-perf">
.fade{opacity:1!important;animation:none!important}
.fire-ignite,.flame-motion,.flame-outer-motion,.flame-inner-motion,.fire-glow-motion{animation:none!important}
.fire-glow-motion{filter:none!important;opacity:.16!important}
.ember{display:none!important;animation:none!important}
.current-number-lit{filter:none!important}
#smoke-plume-main,#smoke-plume-side,#smoke-plume-main-unknown{filter:none!important}
</style>"#;

    let Some(close) = svg.rfind("</svg>") else {
        return svg.to_owned();
    };

    let mut optimized = String::with_capacity(svg.len() + PERF_STYLE.len());
    optimized.push_str(&svg[..close]);
    optimized.push_str(PERF_STYLE);
    optimized.push_str(&svg[close..]);
    optimized
}

fn append_streak_perf_etag(etag: &str) -> String {
    match etag.strip_suffix('"') {
        Some(prefix) => format!("{prefix}-perf{STREAK_PERF_REVISION}\""),
        None => format!("{etag}-perf{STREAK_PERF_REVISION}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{append_streak_perf_etag, optimize_streak_for_mobile};

    #[test]
    fn streak_mobile_override_disables_infinite_motion_and_heavy_filters() {
        let optimized = optimize_streak_for_mobile("<svg><g/></svg>");
        assert!(optimized.contains("id=\"streak-mobile-perf\""));
        assert!(optimized.contains("animation:none!important"));
        assert!(optimized.contains("current-number-lit{filter:none!important}"));
        assert!(optimized.ends_with("</svg>"));
    }

    #[test]
    fn streak_perf_revision_changes_the_etag() {
        assert_eq!(
            append_streak_perf_etag("\"profile-github-card-v10-streak-7-fresh\""),
            "\"profile-github-card-v10-streak-7-fresh-perf1\""
        );
    }
}
