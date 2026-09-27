//! The background sweep.
//!
//! Stat decay itself does not need this - [`crate::state::AppState::load_pet`]
//! brings a pet up to date whenever anyone looks at it. The sweep exists so
//! that a neglected pet can *say something* before it is too late, and so a
//! death is announced when it happens rather than whenever someone next runs a
//! command.

use std::sync::Arc;

use anyhow::Result;
use serenity::all::{ChannelId, CreateMessage, Http};

use crate::db;
use crate::model::{GuildSettings, Pet};
use crate::simulation;
use crate::state::{now_secs, AppState};
use crate::ui;

/// A reason to speak up. Each kind is rate limited separately so a dying pet
/// can still report a death after having warned about hunger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alert {
    Died,
    Critical,
    Starving,
    Lonely,
}

impl Alert {
    fn key(&self) -> &'static str {
        match self {
            Alert::Died => "died",
            Alert::Critical => "critical",
            Alert::Starving => "starving",
            Alert::Lonely => "lonely",
        }
    }
}

/// Decide what, if anything, the pet should say.
fn alert_for(pet: &Pet, died: bool) -> Option<Alert> {
    if died {
        return Some(Alert::Died);
    }
    if !pet.alive {
        return None;
    }
    // Most urgent first: a pet on the edge of death should not be reported as
    // merely hungry.
    if pet.health < 25.0 {
        Some(Alert::Critical)
    } else if pet.hunger < 15.0 {
        Some(Alert::Starving)
    } else if pet.happiness < 15.0 {
        Some(Alert::Lonely)
    } else {
        None
    }
}

fn message_for(alert: Alert, pet: &Pet) -> String {
    match alert {
        Alert::Died => format!(
            "\u{1F56F}\u{FE0F} **{}** has died. Use `/adopt` when the server is ready for a new pet.",
            pet.name
        ),
        Alert::Critical => format!(
            "\u{1F6A8} **{}** is in a bad way and needs `/heal` urgently.",
            pet.name
        ),
        Alert::Starving => format!("\u{1F37D}\u{FE0F} **{}** is starving. Someone `/feed` them.", pet.name),
        Alert::Lonely => format!("\u{1F622} **{}** is lonely and would love to `/play`.", pet.name),
    }
}

/// Whether this alert is allowed to fire, given what was last said.
fn should_send(alert: Alert, settings: &GuildSettings, now: i64, cooldown_secs: i64) -> bool {
    if !settings.alerts_enabled || settings.alert_channel_id.is_none() {
        return false;
    }
    // A death is announced once and is never suppressed by the cooldown.
    if alert == Alert::Died {
        return settings.last_alert.as_deref() != Some(Alert::Died.key());
    }
    match (settings.last_alert.as_deref(), settings.last_alert_at) {
        (Some(last), Some(at)) if last == alert.key() => now - at >= cooldown_secs,
        _ => true,
    }
}

/// Run one pass over every living pet.
pub async fn sweep(state: &Arc<AppState>, http: &Http) -> Result<()> {
    let pets = db::living_pets(&state.pool).await?;
    let now = now_secs();
    let cooldown = state.config.alert_cooldown.as_secs() as i64;

    for mut pet in pets {
        let report = simulation::advance(&mut pet, now, &state.config.rates);

        if report.died {
            db::bury(&state.pool, &pet).await?;
        }
        db::save_pet(&state.pool, &pet).await?;

        let Some(alert) = alert_for(&pet, report.died) else {
            continue;
        };

        let mut settings = db::get_settings(&state.pool, pet.guild_id).await?;
        if !should_send(alert, &settings, now, cooldown) {
            continue;
        }
        let Some(channel_id) = settings.alert_channel_id else {
            continue;
        };

        let species = state.species.get_or_first(&pet.species);
        let card = ui::pet_card(&state.art, &pet, species, now, &state.config.rates).await;

        let message = CreateMessage::new()
            .content(message_for(alert, &pet))
            .embed(card.embed)
            .add_files(card.files);

        match ChannelId::new(channel_id as u64)
            .send_message(http, message)
            .await
        {
            Ok(_) => {
                settings.last_alert = Some(alert.key().to_string());
                settings.last_alert_at = Some(now);
                db::save_settings(&state.pool, &settings).await?;
            }
            Err(err) => {
                // Almost always a deleted channel or a missing permission.
                // Log it and move on; one broken guild must not stall the
                // sweep for everyone else.
                tracing::warn!(
                    guild_id = pet.guild_id,
                    channel_id,
                    %err,
                    "could not deliver a pet alert"
                );
            }
        }
    }

    Ok(())
}

/// Loop forever, sweeping on the configured interval.
pub async fn run(state: Arc<AppState>, http: Arc<Http>) {
    let mut interval = tokio::time::interval(state.config.tick_interval);
    // A sweep that runs long must not cause a burst of catch-up sweeps.
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    tracing::info!(
        seconds = state.config.tick_interval.as_secs(),
        "pet ticker started"
    );

    loop {
        interval.tick().await;
        if let Err(err) = sweep(&state, &http).await {
            tracing::error!(%err, "pet sweep failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(last: Option<&str>, at: Option<i64>) -> GuildSettings {
        GuildSettings {
            guild_id: 1,
            alert_channel_id: Some(42),
            alerts_enabled: true,
            last_alert: last.map(str::to_string),
            last_alert_at: at,
        }
    }

    fn pet_with(hunger: f64, happiness: f64, health: f64) -> Pet {
        let mut pet = Pet::new(1, "T".into(), "blob".into(), 1, 0);
        pet.hunger = hunger;
        pet.happiness = happiness;
        pet.health = health;
        pet
    }

    #[test]
    fn a_comfortable_pet_says_nothing() {
        assert_eq!(alert_for(&pet_with(80.0, 80.0, 80.0), false), None);
    }

    #[test]
    fn urgency_wins_over_hunger() {
        let alert = alert_for(&pet_with(1.0, 1.0, 10.0), false);
        assert_eq!(alert, Some(Alert::Critical));
    }

    #[test]
    fn hunger_wins_over_loneliness() {
        assert_eq!(
            alert_for(&pet_with(5.0, 5.0, 90.0), false),
            Some(Alert::Starving)
        );
    }

    #[test]
    fn death_overrides_everything() {
        assert_eq!(
            alert_for(&pet_with(100.0, 100.0, 100.0), true),
            Some(Alert::Died)
        );
    }

    #[test]
    fn alerts_are_suppressed_when_turned_off() {
        let mut s = settings(None, None);
        s.alerts_enabled = false;
        assert!(!should_send(Alert::Starving, &s, 1000, 100));
    }

    #[test]
    fn alerts_are_suppressed_without_a_channel() {
        let mut s = settings(None, None);
        s.alert_channel_id = None;
        assert!(!should_send(Alert::Starving, &s, 1000, 100));
    }

    #[test]
    fn the_same_alert_is_rate_limited() {
        let s = settings(Some("starving"), Some(1000));
        assert!(!should_send(Alert::Starving, &s, 1050, 100), "too soon");
        assert!(
            should_send(Alert::Starving, &s, 1100, 100),
            "cooldown elapsed"
        );
    }

    #[test]
    fn a_different_alert_is_not_rate_limited() {
        let s = settings(Some("starving"), Some(1000));
        assert!(should_send(Alert::Critical, &s, 1001, 100));
    }

    #[test]
    fn a_death_is_announced_once_and_immediately() {
        let fresh = settings(Some("starving"), Some(1000));
        assert!(should_send(Alert::Died, &fresh, 1001, 10_000));

        let already = settings(Some("died"), Some(1000));
        assert!(!should_send(Alert::Died, &already, 999_999, 10_000));
    }
}
