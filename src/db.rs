//! SQLite persistence.
//!
//! Queries are written at runtime rather than with `sqlx::query!`, so building
//! the project never needs a live database or a checked-in query cache.

use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

use crate::model::{Caretaker, Grave, GuildSettings, Pet};

const SCHEMA: &str = include_str!("../migrations/0001_init.sql");

/// Open the pool and apply the schema.
pub async fn connect(url: &str) -> Result<SqlitePool> {
    // Make sure the parent directory exists before SQLite tries to create the
    // file, otherwise `create_if_missing` still fails on a fresh checkout.
    if let Some(path) = url.strip_prefix("sqlite://").map(Path::new) {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating database directory {}", parent.display()))?;
            }
        }
    }

    let options = SqliteConnectOptions::from_str(url)
        .with_context(|| format!("parsing DATABASE_URL {url:?}"))?
        .create_if_missing(true)
        // WAL plus NORMAL is the usual durability trade for a bot: a crash can
        // lose the last fraction of a second, which for stat decay is noise.
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(10));

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .with_context(|| format!("opening the database at {url}"))?;

    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .context("applying the database schema")?;

    Ok(pool)
}

// -- pets -------------------------------------------------------------------

pub async fn get_pet(pool: &SqlitePool, guild_id: i64) -> Result<Option<Pet>> {
    let pet = sqlx::query_as::<_, Pet>("SELECT * FROM pets WHERE guild_id = ?")
        .bind(guild_id)
        .fetch_optional(pool)
        .await
        .context("loading the guild pet")?;
    Ok(pet)
}

/// Insert or replace the guild's pet.
pub async fn save_pet(pool: &SqlitePool, pet: &Pet) -> Result<()> {
    sqlx::query(
        "INSERT INTO pets (
            guild_id, id, name, species, hunger, happiness, health, energy, xp,
            asleep, alive, born_at, died_at, last_tick, custom_image_url,
            adopted_by, cause_of_death
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(guild_id) DO UPDATE SET
            id = excluded.id,
            name = excluded.name,
            species = excluded.species,
            hunger = excluded.hunger,
            happiness = excluded.happiness,
            health = excluded.health,
            energy = excluded.energy,
            xp = excluded.xp,
            asleep = excluded.asleep,
            alive = excluded.alive,
            born_at = excluded.born_at,
            died_at = excluded.died_at,
            last_tick = excluded.last_tick,
            custom_image_url = excluded.custom_image_url,
            adopted_by = excluded.adopted_by,
            cause_of_death = excluded.cause_of_death",
    )
    .bind(pet.guild_id)
    .bind(&pet.id)
    .bind(&pet.name)
    .bind(&pet.species)
    .bind(pet.hunger)
    .bind(pet.happiness)
    .bind(pet.health)
    .bind(pet.energy)
    .bind(pet.xp)
    .bind(pet.asleep)
    .bind(pet.alive)
    .bind(pet.born_at)
    .bind(pet.died_at)
    .bind(pet.last_tick)
    .bind(&pet.custom_image_url)
    .bind(pet.adopted_by)
    .bind(&pet.cause_of_death)
    .execute(pool)
    .await
    .context("saving the pet")?;
    Ok(())
}

pub async fn delete_pet(pool: &SqlitePool, guild_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM pets WHERE guild_id = ?")
        .bind(guild_id)
        .execute(pool)
        .await
        .context("deleting the pet")?;
    Ok(())
}

/// Every pet still alive, for the background sweep.
pub async fn living_pets(pool: &SqlitePool) -> Result<Vec<Pet>> {
    let pets = sqlx::query_as::<_, Pet>("SELECT * FROM pets WHERE alive = 1")
        .fetch_all(pool)
        .await
        .context("loading living pets")?;
    Ok(pets)
}

// -- caretakers -------------------------------------------------------------

pub async fn get_caretaker(
    pool: &SqlitePool,
    guild_id: i64,
    pet_id: &str,
    user_id: i64,
) -> Result<Option<Caretaker>> {
    let row = sqlx::query_as::<_, Caretaker>(
        "SELECT * FROM caretakers WHERE guild_id = ? AND pet_id = ? AND user_id = ?",
    )
    .bind(guild_id)
    .bind(pet_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .context("loading a caretaker record")?;
    Ok(row)
}

/// The action a member just performed, used to bump the right counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CareAction {
    Feed,
    Play,
    Heal,
}

impl CareAction {
    /// Column holding the timestamp of the last use, for cooldowns.
    pub fn timestamp_column(&self) -> &'static str {
        match self {
            CareAction::Feed => "last_feed_at",
            CareAction::Play => "last_play_at",
            CareAction::Heal => "last_heal_at",
        }
    }

    fn counter_column(&self) -> &'static str {
        match self {
            CareAction::Feed => "feeds",
            CareAction::Play => "plays",
            CareAction::Heal => "heals",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            CareAction::Feed => "feed",
            CareAction::Play => "play",
            CareAction::Heal => "heal",
        }
    }
}

/// Credit a member for caring for the pet.
pub async fn record_care(
    pool: &SqlitePool,
    guild_id: i64,
    pet_id: &str,
    user_id: i64,
    action: CareAction,
    points: f64,
    now: i64,
) -> Result<()> {
    // Column names come from the `CareAction` enum, never from user input, so
    // interpolating them is safe. Values stay bound.
    let counter = action.counter_column();
    let stamp = action.timestamp_column();
    let sql = format!(
        "INSERT INTO caretakers (guild_id, pet_id, user_id, care_points, {counter}, {stamp})
         VALUES (?, ?, ?, ?, 1, ?)
         ON CONFLICT(guild_id, pet_id, user_id) DO UPDATE SET
            care_points = care_points + excluded.care_points,
            {counter} = {counter} + 1,
            {stamp} = excluded.{stamp}"
    );

    sqlx::query(&sql)
        .bind(guild_id)
        .bind(pet_id)
        .bind(user_id)
        .bind(points)
        .bind(now)
        .execute(pool)
        .await
        .context("recording care")?;
    Ok(())
}

pub async fn leaderboard(
    pool: &SqlitePool,
    guild_id: i64,
    pet_id: &str,
    limit: i64,
) -> Result<Vec<Caretaker>> {
    let rows = sqlx::query_as::<_, Caretaker>(
        "SELECT * FROM caretakers
         WHERE guild_id = ? AND pet_id = ?
         ORDER BY care_points DESC, feeds + plays + heals DESC
         LIMIT ?",
    )
    .bind(guild_id)
    .bind(pet_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("loading the leaderboard")?;
    Ok(rows)
}

/// Number of distinct members who have ever cared for this pet.
pub async fn caretaker_count(pool: &SqlitePool, guild_id: i64, pet_id: &str) -> Result<i64> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM caretakers WHERE guild_id = ? AND pet_id = ?")
            .bind(guild_id)
            .bind(pet_id)
            .fetch_one(pool)
            .await
            .context("counting caretakers")?;
    Ok(count)
}

// -- graveyard --------------------------------------------------------------

pub async fn bury(pool: &SqlitePool, pet: &Pet) -> Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO graveyard (id, guild_id, name, species, born_at, died_at, cause, level)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&pet.id)
    .bind(pet.guild_id)
    .bind(&pet.name)
    .bind(&pet.species)
    .bind(pet.born_at)
    .bind(pet.died_at.unwrap_or(pet.born_at))
    .bind(pet.cause_of_death.as_deref().unwrap_or("unknown"))
    .bind(pet.level() as i64)
    .execute(pool)
    .await
    .context("recording a death")?;
    Ok(())
}

pub async fn graveyard(pool: &SqlitePool, guild_id: i64, limit: i64) -> Result<Vec<Grave>> {
    let rows = sqlx::query_as::<_, Grave>(
        "SELECT * FROM graveyard WHERE guild_id = ? ORDER BY died_at DESC LIMIT ?",
    )
    .bind(guild_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("loading the graveyard")?;
    Ok(rows)
}

// -- settings ---------------------------------------------------------------

pub async fn get_settings(pool: &SqlitePool, guild_id: i64) -> Result<GuildSettings> {
    let row = sqlx::query_as::<_, GuildSettings>("SELECT * FROM guild_settings WHERE guild_id = ?")
        .bind(guild_id)
        .fetch_optional(pool)
        .await
        .context("loading guild settings")?;
    Ok(row.unwrap_or_else(|| GuildSettings::default_for(guild_id)))
}

pub async fn save_settings(pool: &SqlitePool, settings: &GuildSettings) -> Result<()> {
    sqlx::query(
        "INSERT INTO guild_settings (guild_id, alert_channel_id, alerts_enabled, last_alert, last_alert_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(guild_id) DO UPDATE SET
            alert_channel_id = excluded.alert_channel_id,
            alerts_enabled = excluded.alerts_enabled,
            last_alert = excluded.last_alert,
            last_alert_at = excluded.last_alert_at",
    )
    .bind(settings.guild_id)
    .bind(settings.alert_channel_id)
    .bind(settings.alerts_enabled)
    .bind(&settings.last_alert)
    .bind(settings.last_alert_at)
    .execute(pool)
    .await
    .context("saving guild settings")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Pet;

    async fn memory_pool() -> SqlitePool {
        connect("sqlite::memory:")
            .await
            .expect("in-memory database")
    }

    #[tokio::test]
    async fn schema_applies_cleanly_and_is_idempotent() {
        let pool = memory_pool().await;
        // Re-applying must not fail; the bot runs this on every start.
        sqlx::raw_sql(SCHEMA).execute(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn pets_round_trip() {
        let pool = memory_pool().await;
        assert!(get_pet(&pool, 7).await.unwrap().is_none());

        let mut pet = Pet::new(7, "Mochi".into(), "blob".into(), 42, 1000);
        save_pet(&pool, &pet).await.unwrap();

        let loaded = get_pet(&pool, 7).await.unwrap().expect("saved pet");
        assert_eq!(loaded.name, "Mochi");
        assert_eq!(loaded.id, pet.id);
        assert!(loaded.alive);
        assert!(!loaded.asleep);

        pet.name = "Mochi II".into();
        pet.asleep = true;
        save_pet(&pool, &pet).await.unwrap();

        let loaded = get_pet(&pool, 7).await.unwrap().unwrap();
        assert_eq!(loaded.name, "Mochi II");
        assert!(loaded.asleep);
    }

    #[tokio::test]
    async fn only_living_pets_are_swept() {
        let pool = memory_pool().await;
        let alive = Pet::new(1, "A".into(), "blob".into(), 1, 0);
        let mut dead = Pet::new(2, "B".into(), "blob".into(), 1, 0);
        dead.alive = false;
        save_pet(&pool, &alive).await.unwrap();
        save_pet(&pool, &dead).await.unwrap();

        let living = living_pets(&pool).await.unwrap();
        assert_eq!(living.len(), 1);
        assert_eq!(living[0].name, "A");
    }

    #[tokio::test]
    async fn care_accumulates_per_member() {
        let pool = memory_pool().await;
        record_care(&pool, 1, "pet-a", 100, CareAction::Feed, 5.0, 10)
            .await
            .unwrap();
        record_care(&pool, 1, "pet-a", 100, CareAction::Feed, 5.0, 20)
            .await
            .unwrap();
        record_care(&pool, 1, "pet-a", 200, CareAction::Play, 3.0, 30)
            .await
            .unwrap();

        let board = leaderboard(&pool, 1, "pet-a", 10).await.unwrap();
        assert_eq!(board.len(), 2);
        assert_eq!(board[0].user_id, 100);
        assert_eq!(board[0].care_points, 10.0);
        assert_eq!(board[0].feeds, 2);
        assert_eq!(board[0].last_feed_at, Some(20));
        assert_eq!(board[1].plays, 1);

        assert_eq!(caretaker_count(&pool, 1, "pet-a").await.unwrap(), 2);
    }

    #[tokio::test]
    async fn care_is_scoped_to_one_incarnation() {
        let pool = memory_pool().await;
        record_care(&pool, 1, "pet-a", 100, CareAction::Feed, 5.0, 10)
            .await
            .unwrap();
        assert!(leaderboard(&pool, 1, "pet-b", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn burial_is_idempotent() {
        let pool = memory_pool().await;
        let mut pet = Pet::new(1, "Rest".into(), "blob".into(), 1, 0);
        pet.alive = false;
        pet.died_at = Some(500);
        pet.cause_of_death = Some("starvation".into());

        bury(&pool, &pet).await.unwrap();
        bury(&pool, &pet).await.unwrap();

        let graves = graveyard(&pool, 1, 10).await.unwrap();
        assert_eq!(graves.len(), 1);
        assert_eq!(graves[0].cause, "starvation");
    }

    #[tokio::test]
    async fn settings_default_then_persist() {
        let pool = memory_pool().await;
        let s = get_settings(&pool, 5).await.unwrap();
        assert_eq!(s.alert_channel_id, None);
        assert!(s.alerts_enabled);

        let mut s = s;
        s.alert_channel_id = Some(999);
        s.alerts_enabled = false;
        save_settings(&pool, &s).await.unwrap();

        let s = get_settings(&pool, 5).await.unwrap();
        assert_eq!(s.alert_channel_id, Some(999));
        assert!(!s.alerts_enabled);
    }
}
