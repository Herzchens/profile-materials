mod activity;
mod artwork_embed;
mod clock;
mod config;
mod discord;
mod github;
mod http;
mod presentation;
mod state;
mod svg;

use std::{error::Error, io, sync::Arc};

use tracing_subscriber::EnvFilter;

use crate::{
    config::AppConfig,
    github::GitHubStore,
    state::{PresenceState, PresenceStore, lkg},
};

type DynError = Box<dyn Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), DynError> {
    init_tracing();
    install_crypto_provider()?;

    let AppConfig {
        bind_addr,
        discord,
        github: github_config,
        stale_after,
        state_path,
        unavailable_after,
    } = AppConfig::from_env()?;
    let now_unix_ms = clock::unix_time_millis()?;
    let restored = match lkg::load(&state_path).await {
        Ok(restored) => restored,
        Err(error) => {
            tracing::warn!(
                path = %state_path.display(),
                error = %error,
                "presence LKG could not be restored; starting without persisted state"
            );
            None
        }
    };

    if let Some(restored) = &restored {
        tracing::info!(
            path = %state_path.display(),
            revision = restored.snapshot.revision,
            validated_at_unix_ms = restored.validated_at_unix_ms,
            "restored presence LKG"
        );
    }

    let store = Arc::new(PresenceStore::new(
        stale_after,
        unavailable_after,
        restored,
        now_unix_ms,
    ));
    let initial = store.load();
    if let PresenceState::Known(snapshot) = &initial.presence {
        tracing::info!(
            revision = snapshot.revision,
            freshness = ?initial.freshness,
            "presence state initialized from LKG"
        );
    }

    let github_restored = match github::lkg::load(&github_config.state_path).await {
        Ok(restored) => restored,
        Err(error) => {
            tracing::warn!(
                path = %github_config.state_path.display(),
                error = %error,
                "GitHub LKG could not be restored; starting without persisted stats"
            );
            None
        }
    };
    if let Some(snapshot) = &github_restored {
        tracing::info!(
            path = %github_config.state_path.display(),
            revision = snapshot.revision,
            collected_at_unix_ms = snapshot.collected_at_unix_ms,
            "restored GitHub LKG"
        );
    }
    let github_stale_after = github_config.stale_after;
    let github_store = Arc::new(GitHubStore::new(github_restored));

    let gateway = discord::gateway::run(discord, Arc::clone(&store), state_path);
    let github_collector = github::run(github_config, Arc::clone(&github_store));
    let http = http::serve(
        bind_addr,
        Arc::clone(&store),
        Arc::clone(&github_store),
        github_stale_after,
    );
    let watchdog = state::run_stale_watchdog(store);

    tokio::pin!(gateway);
    tokio::pin!(github_collector);
    tokio::pin!(http);
    tokio::pin!(watchdog);

    tokio::select! {
        result = &mut gateway => match result {
            Ok(()) => Err(Box::new(io::Error::other("Discord Gateway collector ended unexpectedly")) as DynError),
            Err(error) => Err(Box::new(error) as DynError),
        },
        result = &mut github_collector => match result {
            Ok(()) => Err(Box::new(io::Error::other("GitHub collector ended unexpectedly")) as DynError),
            Err(error) => Err(Box::new(error) as DynError),
        },
        result = &mut http => match result {
            Ok(()) => Err(Box::new(io::Error::other("HTTP server ended unexpectedly")) as DynError),
            Err(error) => Err(Box::new(error) as DynError),
        },
        result = &mut watchdog => match result {
            Ok(()) => Err(Box::new(io::Error::other("stale-state watchdog ended unexpectedly")) as DynError),
            Err(error) => Err(Box::new(error) as DynError),
        },
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .init();
}

fn install_crypto_provider() -> io::Result<()> {
    if rustls::crypto::CryptoProvider::get_default().is_some() {
        return Ok(());
    }

    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| io::Error::other("failed to install the rustls crypto provider"))
}
