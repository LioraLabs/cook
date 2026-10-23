//! `cook cache serve` — the v1 artifact protocol over a local store.
//!
//! Needs no Cookfile or project root: it serves the directory `--store` names
//! and nothing else. The bearer token comes from the environment, never argv.

use std::sync::Arc;

use cook_engine::cook_cache::cloud_backend::API_KEY_ENV;
use cook_engine::cook_cache::serve::CacheServer;
use cook_engine::cook_cache::{resolve_api_key, BackendConfig, LocalBackend};

use crate::cli::CacheServeArgs;
use crate::error::CookError;

pub fn cmd_cache_serve(args: &CacheServeArgs) -> Result<(), CookError> {
    let Some(token) = resolve_api_key() else {
        return Err(CookError::Other(format!(
            "cook cache serve needs a bearer token in the {API_KEY_ENV} environment variable (unset or empty)"
        )));
    };
    // Checked before the backend exists: constructing it creates the root.
    if args.init {
        std::fs::create_dir_all(&args.store).map_err(|e| {
            CookError::Other(format!("cannot create store {}: {e}", args.store.display()))
        })?;
    }
    if !args.store.is_dir() {
        return Err(CookError::Other(format!(
            "store {} is not an existing directory; pass --init to create it",
            args.store.display()
        )));
    }
    let config = BackendConfig {
        max_artifact_bytes: args.max_artifact_mib.saturating_mul(1024 * 1024),
        ..BackendConfig::default()
    };
    let server = CacheServer::bind(args.listen.as_str(), LocalBackend::with_config(args.store.clone(), config), token)
        .map_err(|e| CookError::Other(format!("cannot listen on {}: {e}", args.listen)))?
        .with_max_connections(std::num::NonZeroUsize::new(args.max_connections as usize).expect("clap requires --max-connections >= 1"));
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ])
    .map_err(|e| CookError::Other(format!("cannot install signal handlers: {e}")))?;
    eprintln!("listening on {}", server.local_addr());
    let server = Arc::new(server);
    let stopper = Arc::clone(&server);
    std::thread::spawn(move || {
        let mut signals = signals.forever();
        if signals.next().is_some() {
            stopper.shutdown();
        }
        // A second signal must not wait on a stalled upload to drain.
        if let Some(sig) = signals.next() {
            std::process::exit(128 + sig);
        }
    });

    server.run();
    Ok(())
}
