//! Runtime configuration, read from the environment (and `.env` if present).

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use crate::simulation::Rates;

/// Per-member cooldowns between care actions.
#[derive(Debug, Clone, Copy)]
pub struct Cooldowns {
    pub feed: Duration,
    pub play: Duration,
    pub heal: Duration,
}

impl Default for Cooldowns {
    fn default() -> Self {
        // Short enough that a member can help whenever they check in, long
        // enough that one person cannot solo-maintain the pet and lock
        // everyone else out of contributing.
        Self {
            feed: Duration::from_secs(15 * 60),
            play: Duration::from_secs(10 * 60),
            heal: Duration::from_secs(2 * 60 * 60),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub token: String,
    pub database_url: String,
    pub assets_dir: PathBuf,
    /// When set, slash commands register to this guild only, which is instant.
    /// Global registration can take up to an hour to propagate.
    pub dev_guild_id: Option<u64>,
    pub tick_interval: Duration,
    pub rates: Rates,
    pub cooldowns: Cooldowns,
    /// Minimum gap between two alerts of the same kind in one guild.
    pub alert_cooldown: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let token = std::env::var("DISCORD_TOKEN")
            .map_err(|_| {
                anyhow!("DISCORD_TOKEN is not set; copy .env.example to .env and fill it in")
            })?
            .trim()
            .to_string();
        if token.is_empty() {
            return Err(anyhow!("DISCORD_TOKEN is empty"));
        }

        let database_url = env_or("DATABASE_URL", "sqlite://data/pet.db");
        let assets_dir = PathBuf::from(env_or("ASSETS_DIR", "assets"));

        let dev_guild_id = match std::env::var("DEV_GUILD_ID") {
            Ok(v) if !v.trim().is_empty() => Some(
                v.trim()
                    .parse::<u64>()
                    .context("DEV_GUILD_ID must be a numeric Discord guild id")?,
            ),
            _ => None,
        };

        let tick_interval = Duration::from_secs(parse_env("TICK_SECONDS", 300u64)?.max(10));

        let rates = Rates {
            time_scale: parse_env("TIME_SCALE", 1.0f64)?,
            ..Rates::default()
        };
        if rates.time_scale <= 0.0 {
            return Err(anyhow!("TIME_SCALE must be greater than zero"));
        }

        let cooldowns = Cooldowns {
            feed: Duration::from_secs(parse_env("FEED_COOLDOWN_SECS", 15 * 60u64)?),
            play: Duration::from_secs(parse_env("PLAY_COOLDOWN_SECS", 10 * 60u64)?),
            heal: Duration::from_secs(parse_env("HEAL_COOLDOWN_SECS", 2 * 60 * 60u64)?),
        };

        Ok(Self {
            token,
            database_url,
            assets_dir,
            dev_guild_id,
            tick_interval,
            rates,
            cooldowns,
            alert_cooldown: Duration::from_secs(parse_env("ALERT_COOLDOWN_SECS", 6 * 60 * 60u64)?),
        })
    }

    pub fn species_manifest(&self) -> PathBuf {
        self.assets_dir.join("species.toml")
    }

    pub fn art_dir(&self) -> PathBuf {
        self.assets_dir.join("pets")
    }
}

fn env_or(key: &str, default: &str) -> String {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => default.to_string(),
    }
}

fn parse_env<T>(key: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => v
            .trim()
            .parse::<T>()
            .map_err(|e| anyhow!("{key} is not a valid value: {e}")),
        _ => Ok(default),
    }
}
