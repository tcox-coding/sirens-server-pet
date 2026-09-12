//! Core domain types for the server pet.
//!
//! Stat convention: **every stat is "higher is better", 0.0..=100.0**.
//! `hunger` therefore measures how *fed* the pet is - 100 is a full belly and
//! 0 is starving. The UI labels it "Hunger" but draws the bar from this value,
//! so a long bar always means a well-cared-for pet.

use std::fmt;

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

pub const STAT_MAX: f64 = 100.0;
pub const STAT_MIN: f64 = 0.0;

/// Clamp a stat into the legal range.
pub fn clamp_stat(v: f64) -> f64 {
    v.clamp(STAT_MIN, STAT_MAX)
}

/// A pet as stored in SQLite. One row per guild.
///
/// Discord snowflakes are stored as `i64` because SQLite has no unsigned
/// integer type; they are converted back at the edges.
#[derive(Debug, Clone, FromRow)]
pub struct Pet {
    pub guild_id: i64,
    /// Stable id for this *incarnation*. A new pet after a death gets a new id
    /// so caretaker records stay attributed to the pet they cared for.
    pub id: String,
    pub name: String,
    pub species: String,
    pub hunger: f64,
    pub happiness: f64,
    pub health: f64,
    pub energy: f64,
    pub xp: f64,
    pub asleep: bool,
    pub alive: bool,
    /// Unix seconds.
    pub born_at: i64,
    pub died_at: Option<i64>,
    /// Unix seconds of the last simulation step. Decay is computed from here.
    pub last_tick: i64,
    /// Optional override that replaces the local asset with a remote image.
    pub custom_image_url: Option<String>,
    pub adopted_by: i64,
    /// Set when the pet dies; used for the memorial embed.
    pub cause_of_death: Option<String>,
}

impl Pet {
    pub fn new(guild_id: i64, name: String, species: String, adopted_by: i64, now: i64) -> Self {
        Self {
            guild_id,
            id: uuid::Uuid::new_v4().to_string(),
            name,
            species,
            // A new pet starts well cared for but not perfect, so there is
            // something to do in the first hour.
            hunger: 80.0,
            happiness: 80.0,
            health: 100.0,
            energy: 90.0,
            xp: 0.0,
            asleep: false,
            alive: true,
            born_at: now,
            died_at: None,
            last_tick: now,
            custom_image_url: None,
            adopted_by,
            cause_of_death: None,
        }
    }

    pub fn level(&self) -> u32 {
        level_for_xp(self.xp)
    }

    pub fn stage(&self) -> Stage {
        Stage::for_level(self.level())
    }

    /// Progress into the current level, as `(xp_into_level, xp_needed)`.
    pub fn level_progress(&self) -> (f64, f64) {
        let lvl = self.level();
        let floor = xp_for_level(lvl);
        let ceil = xp_for_level(lvl + 1);
        (self.xp - floor, ceil - floor)
    }

    /// Seconds the pet has been alive, or how long it lived if it is dead.
    pub fn age_secs(&self, now: i64) -> i64 {
        let end = self.died_at.unwrap_or(now);
        (end - self.born_at).max(0)
    }

    pub fn mood(&self) -> Mood {
        if !self.alive {
            return Mood::Dead;
        }
        if self.asleep {
            return Mood::Sleeping;
        }
        if self.health < 30.0 {
            Mood::Sick
        } else if self.hunger < 25.0 {
            Mood::Hungry
        } else if self.happiness < 25.0 {
            Mood::Sad
        } else if self.energy < 25.0 {
            Mood::Tired
        } else if self.happiness >= 75.0 && self.hunger >= 60.0 && self.health >= 75.0 {
            Mood::Happy
        } else {
            Mood::Content
        }
    }

    /// A single 0..100 summary used for alerts and the status footer.
    pub fn wellbeing(&self) -> f64 {
        if !self.alive {
            return 0.0;
        }
        // Health is weighted heaviest because it is the stat that actually
        // ends the pet's life.
        (self.health * 0.4 + self.hunger * 0.25 + self.happiness * 0.25 + self.energy * 0.1)
            .clamp(0.0, 100.0)
    }
}

/// Total xp required to *reach* a level. Level 1 is the starting level.
pub fn xp_for_level(level: u32) -> f64 {
    let n = level.max(1) as f64;
    50.0 * n * (n - 1.0)
}

/// Inverse of [`xp_for_level`], solved with the quadratic formula.
pub fn level_for_xp(xp: f64) -> u32 {
    let xp = xp.max(0.0);
    let n = (1.0 + (1.0 + 4.0 * xp / 50.0).sqrt()) / 2.0;
    // Guard against floating point landing a hair under an exact threshold.
    let n = (n + 1e-9).floor() as u32;
    n.max(1)
}

/// Life stage, derived from level. Used for flavour text and art selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Baby,
    Child,
    Teen,
    Adult,
    Elder,
}

impl Stage {
    pub fn for_level(level: u32) -> Self {
        match level {
            0..=2 => Stage::Baby,
            3..=5 => Stage::Child,
            6..=9 => Stage::Teen,
            10..=19 => Stage::Adult,
            _ => Stage::Elder,
        }
    }

    /// Directory-safe key used when looking up art.
    pub fn key(&self) -> &'static str {
        match self {
            Stage::Baby => "baby",
            Stage::Child => "child",
            Stage::Teen => "teen",
            Stage::Adult => "adult",
            Stage::Elder => "elder",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Stage::Baby => "Baby",
            Stage::Child => "Child",
            Stage::Teen => "Teen",
            Stage::Adult => "Adult",
            Stage::Elder => "Elder",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Emotional state, derived from stats. Drives which image is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Happy,
    Content,
    Hungry,
    Sad,
    Tired,
    Sick,
    Sleeping,
    Dead,
}

impl Mood {
    /// Every mood, in the order art packs should supply them.
    pub const ALL: [Mood; 8] = [
        Mood::Happy,
        Mood::Content,
        Mood::Hungry,
        Mood::Sad,
        Mood::Tired,
        Mood::Sick,
        Mood::Sleeping,
        Mood::Dead,
    ];

    /// Directory-safe key used when looking up art.
    pub fn key(&self) -> &'static str {
        match self {
            Mood::Happy => "happy",
            Mood::Content => "content",
            Mood::Hungry => "hungry",
            Mood::Sad => "sad",
            Mood::Tired => "tired",
            Mood::Sick => "sick",
            Mood::Sleeping => "sleeping",
            Mood::Dead => "dead",
        }
    }

    pub fn emoji(&self) -> &'static str {
        match self {
            Mood::Happy => "\u{1F60A}",
            Mood::Content => "\u{1F642}",
            Mood::Hungry => "\u{1F37D}\u{FE0F}",
            Mood::Sad => "\u{1F622}",
            Mood::Tired => "\u{1F62A}",
            Mood::Sick => "\u{1F912}",
            Mood::Sleeping => "\u{1F4A4}",
            Mood::Dead => "\u{1F480}",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Mood::Happy => "Delighted",
            Mood::Content => "Content",
            Mood::Hungry => "Hungry",
            Mood::Sad => "Lonely",
            Mood::Tired => "Exhausted",
            Mood::Sick => "Unwell",
            Mood::Sleeping => "Asleep",
            Mood::Dead => "Gone",
        }
    }

    /// Embed accent colour, as `0xRRGGBB`.
    pub fn colour(&self) -> u32 {
        match self {
            Mood::Happy => 0x57F287,
            Mood::Content => 0x5865F2,
            Mood::Hungry => 0xFEE75C,
            Mood::Sad => 0x9B84EE,
            Mood::Tired => 0x99AAB5,
            Mood::Sick => 0xED4245,
            Mood::Sleeping => 0x4E5D94,
            Mood::Dead => 0x2C2F33,
        }
    }
}

impl fmt::Display for Mood {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Per-user care record, scoped to one pet incarnation.
///
/// Mirrors the `caretakers` row in full. The key columns are not read back in
/// Rust today - queries filter on them in SQL - but keeping them makes the
/// struct self-describing in logs and debugger output.
#[allow(dead_code)]
#[derive(Debug, Clone, FromRow)]
pub struct Caretaker {
    pub guild_id: i64,
    pub pet_id: String,
    pub user_id: i64,
    pub care_points: f64,
    pub feeds: i64,
    pub plays: i64,
    pub heals: i64,
    pub last_feed_at: Option<i64>,
    pub last_play_at: Option<i64>,
    pub last_heal_at: Option<i64>,
}

/// A pet that did not make it. Kept for the graveyard command.
///
/// Mirrors the `graveyard` row in full; see the note on [`Caretaker`].
#[allow(dead_code)]
#[derive(Debug, Clone, FromRow)]
pub struct Grave {
    pub id: String,
    pub guild_id: i64,
    pub name: String,
    pub species: String,
    pub born_at: i64,
    pub died_at: i64,
    pub cause: String,
    pub level: i64,
}

/// Per-guild configuration.
#[derive(Debug, Clone, FromRow)]
pub struct GuildSettings {
    pub guild_id: i64,
    pub alert_channel_id: Option<i64>,
    pub alerts_enabled: bool,
    pub last_alert: Option<String>,
    pub last_alert_at: Option<i64>,
}

impl GuildSettings {
    pub fn default_for(guild_id: i64) -> Self {
        Self {
            guild_id,
            alert_channel_id: None,
            alerts_enabled: true,
            last_alert: None,
            last_alert_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_round_trip() {
        for lvl in 1..40u32 {
            let xp = xp_for_level(lvl);
            assert_eq!(
                level_for_xp(xp),
                lvl,
                "xp {xp} should be exactly level {lvl}"
            );
            assert_eq!(level_for_xp(xp + 1.0), lvl, "just past level {lvl}");
            assert_eq!(
                level_for_xp(xp_for_level(lvl + 1) - 1.0),
                lvl,
                "just under level {}",
                lvl + 1
            );
        }
    }

    #[test]
    fn level_one_at_zero_xp() {
        assert_eq!(level_for_xp(0.0), 1);
        assert_eq!(level_for_xp(-5.0), 1);
    }

    #[test]
    fn mood_precedence_puts_death_first() {
        let mut pet = Pet::new(1, "Test".into(), "blob".into(), 1, 0);
        pet.alive = false;
        pet.asleep = true;
        assert_eq!(pet.mood(), Mood::Dead);
    }

    #[test]
    fn sickness_outranks_hunger() {
        let mut pet = Pet::new(1, "Test".into(), "blob".into(), 1, 0);
        pet.health = 10.0;
        pet.hunger = 0.0;
        assert_eq!(pet.mood(), Mood::Sick);
    }

    #[test]
    fn stage_boundaries() {
        assert_eq!(Stage::for_level(1), Stage::Baby);
        assert_eq!(Stage::for_level(3), Stage::Child);
        assert_eq!(Stage::for_level(6), Stage::Teen);
        assert_eq!(Stage::for_level(10), Stage::Adult);
        assert_eq!(Stage::for_level(20), Stage::Elder);
    }
}
