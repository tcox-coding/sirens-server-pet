//! A persistent virtual pet that lives in a Discord server.
//!
//! One pet per guild, shared by everyone. Its stats decay in real time, so a
//! server that ignores it will lose it.

mod card;
mod commands;
mod config;
mod db;
mod handler;
mod images;
mod model;
mod simulation;
mod species;
mod state;
mod ticker;
mod ui;

use anyhow::{Context as _, Result};
use serenity::all::GatewayIntents;
use serenity::Client;

use crate::config::Config;
use crate::handler::Handler;
use crate::images::ImageIndex;
use crate::species::Registry;
use crate::state::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    // A missing .env is fine; the environment may be supplied by the host.
    let _ = dotenvy::dotenv();
    init_tracing();

    let config = Config::from_env()?;

    let species = Registry::load(&config.species_manifest()).with_context(|| {
        format!(
            "loading the species manifest. Expected it at {}",
            config.species_manifest().display()
        )
    })?;
    tracing::info!(species = species.len(), "loaded species");

    let art = ImageIndex::scan(&config.art_dir())?;
    if art.is_empty() {
        tracing::warn!("no pet art was found; status cards will be text only");
    } else {
        report_art_coverage(&species, &art);
    }

    let pool = db::connect(&config.database_url).await?;
    tracing::info!(database = %config.database_url, "database ready");

    // The pet is entirely slash-command driven, so no privileged intents and
    // no message content access are needed.
    let intents = GatewayIntents::GUILDS;
    let token = config.token.clone();
    let state = AppState::new(pool, config, species, art);

    let mut client = Client::builder(&token, intents)
        .event_handler(Handler::new(state))
        .await
        .context("building the Discord client")?;

    let shard_manager = client.shard_manager.clone();
    tokio::spawn(async move {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(%err, "could not listen for shutdown");
            return;
        }
        tracing::info!("shutting down");
        shard_manager.shutdown_all().await;
    });

    client.start().await.context("running the Discord client")?;
    Ok(())
}

/// Log which species are short of art.
///
/// Missing art is never fatal - the lookup falls back to the shared `_default`
/// set - but silently showing a placeholder for a species someone just added is
/// a confusing way to find that out.
fn report_art_coverage(species: &Registry, art: &ImageIndex) {
    let with_art = art.species_with_art();

    for sp in species.all() {
        if !with_art.iter().any(|found| found == &sp.key) {
            tracing::warn!(
                species = %sp.key,
                expected = %art.root().join(&sp.key).display(),
                "no art for this species; it will borrow the shared fallback art"
            );
            continue;
        }

        let missing = art.missing_moods(&sp.key, crate::model::Stage::Adult);
        if !missing.is_empty() {
            let moods: Vec<&str> = missing.iter().map(|m| m.key()).collect();
            tracing::info!(
                species = %sp.key,
                moods = %moods.join(", "),
                "some moods fall back to the shared art"
            );
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env()
        // Serenity is chatty at info; the bot's own logs are the interesting
        // ones.
        .unwrap_or_else(|_| EnvFilter::new("server_pet=info,serenity=warn,warn"));

    fmt().with_env_filter(filter).init();
}
