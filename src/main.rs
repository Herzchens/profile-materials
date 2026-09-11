mod config;
mod discord;
mod http;
mod state;

use std::{error::Error, io, sync::Arc};

use tracing_subscriber::EnvFilter;

use crate::{config::AppConfig, state::PresenceStore};

type DynError = Box<dyn Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), DynError> {
    init_tracing();
    install_crypto_provider()?;

    let config = AppConfig::from_env()?;
    let store = Arc::new(PresenceStore::new());
    let gateway = discord::gateway::run(config.discord, Arc::clone(&store));
    let http = http::serve(config.bind_addr, store);

    tokio::pin!(gateway);
    tokio::pin!(http);

    tokio::select! {
        result = &mut gateway => match result {
            Ok(()) => Err(Box::new(io::Error::other("Discord Gateway collector ended unexpectedly")) as DynError),
            Err(error) => Err(Box::new(error) as DynError),
        },
        result = &mut http => match result {
            Ok(()) => Err(Box::new(io::Error::other("HTTP server ended unexpectedly")) as DynError),
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
