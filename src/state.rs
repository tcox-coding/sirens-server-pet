//! Shared application state, handed to every command and to the ticker.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::db;
use crate::images::ImageIndex;
use crate::model::Pet;
use crate::simulation::{self, TickReport};
use crate::species::Registry;

/// Current unix time in seconds.
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub struct AppState {
    pub pool: SqlitePool,
    pub config: Config,
    pub species: Registry,
    pub art: ImageIndex,
}

impl AppState {
    pub fn new(pool: SqlitePool, config: Config, species: Registry, art: ImageIndex) -> Arc<Self> {
        Arc::new(Self {
            pool,
            config,
            species,
            art,
        })
    }

    /// Load a guild's pet with its stats brought up to the present.
    ///
    /// This is the only sanctioned way to read a pet. It applies decay, writes
    /// the result back, and files a death certificate if the pet did not
    /// survive the gap, so no caller can accidentally act on stale stats.
    pub async fn load_pet(&self, guild_id: i64) -> Result<Option<(Pet, TickReport)>> {
        let Some(mut pet) = db::get_pet(&self.pool, guild_id).await? else {
            return Ok(None);
        };

        let report = simulation::advance(&mut pet, now_secs(), &self.config.rates);

        if report.died {
            db::bury(&self.pool, &pet).await?;
        }
        // Always persist: even an uneventful advance moves `last_tick`, and
        // skipping the write would replay the same decay on the next read.
        db::save_pet(&self.pool, &pet).await?;

        Ok(Some((pet, report)))
    }

    /// Persist a pet the caller has just modified.
    pub async fn save_pet(&self, pet: &Pet) -> Result<()> {
        db::save_pet(&self.pool, pet).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Cooldowns};
    use crate::images::ImageIndex;
    use crate::simulation::Rates;
    use crate::species::{Registry, Species};
    use std::time::Duration;

    /// Build a state whose clock runs fast, so a test can cover days of
    /// neglect without waiting for them.
    async fn test_state(time_scale: f64) -> Arc<AppState> {
        let pool = db::connect("sqlite::memory:").await.unwrap();
        let config = Config {
            token: String::new(),
            database_url: "sqlite::memory:".into(),
            assets_dir: "assets".into(),
            dev_guild_id: None,
            tick_interval: Duration::from_secs(60),
            rates: Rates {
                time_scale,
                ..Rates::default()
            },
            cooldowns: Cooldowns::default(),
            alert_cooldown: Duration::from_secs(60),
        };
        let species = Registry::from_species(vec![Species {
            key: "blob".into(),
            name: "Blob".into(),
            emoji: String::new(),
            description: String::new(),
            favourite_food: None,
            favourite_game: None,
        }])
        .unwrap();

        AppState::new(pool, config, species, ImageIndex::default())
    }

    #[tokio::test]
    async fn loading_a_pet_applies_the_decay_it_missed() {
        let state = test_state(1.0).await;

        let mut pet = Pet::new(1, "Mochi".into(), "blob".into(), 99, now_secs());
        // Pretend the pet was last seen eight hours ago.
        pet.last_tick = now_secs() - 8 * 3600;
        state.save_pet(&pet).await.unwrap();

        let (loaded, report) = state.load_pet(1).await.unwrap().expect("a pet");
        assert!(loaded.hunger < pet.hunger, "hunger should have fallen");
        assert!(!report.died);

        // The decay must be applied once, not replayed on the next read.
        let (again, _) = state.load_pet(1).await.unwrap().unwrap();
        assert!(
            (again.hunger - loaded.hunger).abs() < 0.5,
            "a second read replayed the decay: {} then {}",
            loaded.hunger,
            again.hunger
        );
    }

    #[tokio::test]
    async fn a_pet_that_dies_while_unwatched_is_buried_on_the_next_read() {
        let state = test_state(1.0).await;

        let mut pet = Pet::new(1, "Ghost".into(), "blob".into(), 99, now_secs());
        pet.last_tick = now_secs() - 5 * 24 * 3600; // five days of silence
        state.save_pet(&pet).await.unwrap();

        let (loaded, report) = state.load_pet(1).await.unwrap().expect("a pet");
        assert!(report.died, "five days of neglect should be fatal");
        assert!(!loaded.alive);

        let graves = db::graveyard(&state.pool, 1, 10).await.unwrap();
        assert_eq!(graves.len(), 1);
        assert_eq!(graves[0].name, "Ghost");

        // Reading again must not report the death a second time or re-bury it.
        let (_, report) = state.load_pet(1).await.unwrap().unwrap();
        assert!(!report.died);
        assert_eq!(db::graveyard(&state.pool, 1, 10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_guild_with_no_pet_reads_as_none() {
        let state = test_state(1.0).await;
        assert!(state.load_pet(404).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn the_time_scale_actually_speeds_the_world_up() {
        let slow = test_state(1.0).await;
        let fast = test_state(50.0).await;

        for state in [&slow, &fast] {
            let mut pet = Pet::new(1, "T".into(), "blob".into(), 1, now_secs());
            pet.last_tick = now_secs() - 600; // ten minutes
            state.save_pet(&pet).await.unwrap();
        }

        let (slow_pet, _) = slow.load_pet(1).await.unwrap().unwrap();
        let (fast_pet, _) = fast.load_pet(1).await.unwrap().unwrap();
        assert!(
            fast_pet.hunger < slow_pet.hunger,
            "a higher TIME_SCALE should decay faster: {} vs {}",
            fast_pet.hunger,
            slow_pet.hunger
        );
    }
}
