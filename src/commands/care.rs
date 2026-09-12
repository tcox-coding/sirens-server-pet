//! The commands members use day to day: adopt, check, feed, play, rest, heal.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rand::seq::SliceRandom;
use serenity::all::{CommandInteraction, Context, CreateInteractionResponseMessage};

use super::{opt_str, reply_error, reply_no_pet, respond};
use crate::db::{self, CareAction};
use crate::model::{clamp_stat, Pet};
use crate::state::{now_secs, AppState};
use crate::ui;

/// Longest pet name we accept. Discord embed titles can hold far more, but a
/// short name keeps the status card readable.
const MAX_NAME_LEN: usize = 32;

/// One care action: what it is, how often it may be used, and what it does.
#[derive(Debug, Clone, Copy)]
struct Care {
    action: CareAction,
    cooldown: Duration,
    effect: Effect,
}

/// What one care action does to the pet.
#[derive(Debug, Clone, Copy)]
struct Effect {
    hunger: f64,
    happiness: f64,
    health: f64,
    energy: f64,
    xp: f64,
    /// Base care points before the urgency multiplier.
    points: f64,
}

// -- adopt ------------------------------------------------------------------

pub async fn adopt(state: &Arc<AppState>, ctx: &Context, cmd: &CommandInteraction) -> Result<()> {
    let Some(guild_id) = cmd.guild_id else {
        return reply_error(
            ctx,
            cmd,
            "The server pet only lives in servers, not in DMs.",
        )
        .await;
    };
    let guild_id = guild_id.get() as i64;

    let species_key = opt_str(cmd, "species").unwrap_or_default();
    let Some(species) = state.species.get(species_key) else {
        return reply_error(ctx, cmd, "That species is not available on this bot.").await;
    };

    let raw_name = opt_str(cmd, "name").unwrap_or_default();
    let name = match sanitize_name(raw_name) {
        Ok(name) => name,
        Err(why) => return reply_error(ctx, cmd, why).await,
    };

    // A living pet blocks adoption; a dead one is replaced.
    if let Some((existing, _)) = state.load_pet(guild_id).await? {
        if existing.alive {
            return reply_error(
                ctx,
                cmd,
                format!(
                    "This server already has **{}**. Use `/petrelease` first if you really want to start over.",
                    existing.name
                ),
            )
            .await;
        }
    }

    let pet = Pet::new(
        guild_id,
        name,
        species.key.clone(),
        cmd.user.id.get() as i64,
        now_secs(),
    );
    state.save_pet(&pet).await?;

    let (image_url, attachment) = ui::art_for(&state.art, &pet).await;
    let embed = ui::status_embed(&pet, species, image_url, now_secs(), &state.config.rates)
        .title(format!("\u{1F389} {} has joined the server!", pet.name));

    let mut message = CreateInteractionResponseMessage::new()
        .content(format!(
            "<@{}> adopted a {}. Keep them fed and happy with `/feed` and `/play`.",
            cmd.user.id, species.name
        ))
        .embed(embed);
    if let Some(file) = attachment {
        message = message.add_file(file);
    }

    respond(ctx, cmd, message).await
}

// -- status -----------------------------------------------------------------

pub async fn status(state: &Arc<AppState>, ctx: &Context, cmd: &CommandInteraction) -> Result<()> {
    let Some(guild_id) = cmd.guild_id else {
        return reply_error(
            ctx,
            cmd,
            "The server pet only lives in servers, not in DMs.",
        )
        .await;
    };

    let Some((pet, _)) = state.load_pet(guild_id.get() as i64).await? else {
        return reply_no_pet(ctx, cmd).await;
    };

    let species = state.species.get_or_first(&pet.species);
    let (image_url, attachment) = ui::art_for(&state.art, &pet).await;
    let embed = ui::status_embed(&pet, species, image_url, now_secs(), &state.config.rates);

    let mut message = CreateInteractionResponseMessage::new().embed(embed);
    if let Some(file) = attachment {
        message = message.add_file(file);
    }
    respond(ctx, cmd, message).await
}

// -- feed -------------------------------------------------------------------

pub async fn feed(state: &Arc<AppState>, ctx: &Context, cmd: &CommandInteraction) -> Result<()> {
    let food = opt_str(cmd, "food").unwrap_or("meal");
    // Kept as a compact table on purpose: the numbers are easier to balance
    // when every option lines up.
    #[rustfmt::skip]
    let (effect, description) = match food {
        "snack" => (
            Effect { hunger: 15.0, happiness: 3.0, health: 0.0, energy: 2.0, xp: 4.0, points: 3.0 },
            "a quick snack",
        ),
        "treat" => (
            Effect { hunger: 10.0, happiness: 16.0, health: -2.0, energy: 6.0, xp: 6.0, points: 4.0 },
            "a sweet treat",
        ),
        "greens" => (
            Effect { hunger: 25.0, happiness: -4.0, health: 6.0, energy: 0.0, xp: 9.0, points: 6.0 },
            "something healthy",
        ),
        // "meal" and anything unexpected.
        _ => (
            Effect { hunger: 32.0, happiness: 6.0, health: 1.0, energy: 4.0, xp: 8.0, points: 6.0 },
            "a full meal",
        ),
    };

    let flavour = pick(&[
        "Not a crumb left.",
        "Cleaned the bowl in seconds.",
        "Ate far too fast, as usual.",
        "Chewed thoughtfully, then asked for more.",
    ]);

    perform(
        state,
        ctx,
        cmd,
        Care {
            action: CareAction::Feed,
            cooldown: state.config.cooldowns.feed,
            effect,
        },
        |pet| {
            if pet.hunger >= 95.0 {
                Some(format!(
                    "{} is completely full and turns away from the bowl.",
                    pet.name
                ))
            } else {
                None
            }
        },
        move |pet, species| {
            let favourite = species
                .favourite_food
                .as_deref()
                .filter(|f| f.eq_ignore_ascii_case(food))
                .map(|f| format!(" {} loves {f}!", pet.name))
                .unwrap_or_default();
            format!(
                "\u{1F37D}\u{FE0F} You served **{}** {description}.{favourite} {flavour}",
                pet.name
            )
        },
    )
    .await
}

// -- play -------------------------------------------------------------------

pub async fn play(state: &Arc<AppState>, ctx: &Context, cmd: &CommandInteraction) -> Result<()> {
    let game = opt_str(cmd, "game").unwrap_or("fetch");
    // See the note on `feed`: this stays a table.
    #[rustfmt::skip]
    let (effect, description) = match game {
        "puzzle" => (
            Effect { hunger: -3.0, happiness: 13.0, health: 0.0, energy: -8.0, xp: 14.0, points: 5.0 },
            "worked through a puzzle",
        ),
        "dance" => (
            Effect { hunger: -8.0, happiness: 26.0, health: 0.0, energy: -20.0, xp: 12.0, points: 7.0 },
            "threw a dance party",
        ),
        // "fetch" and anything unexpected.
        _ => (
            Effect { hunger: -5.0, happiness: 20.0, health: 0.0, energy: -15.0, xp: 10.0, points: 6.0 },
            "played fetch",
        ),
    };

    let flavour = pick(&[
        "Worth every bit of the energy.",
        "Somebody is going to sleep well tonight.",
        "That was the highlight of the day.",
        "Ten more minutes, surely?",
    ]);

    perform(
        state,
        ctx,
        cmd,
        Care {
            action: CareAction::Play,
            cooldown: state.config.cooldowns.play,
            effect,
        },
        |pet| {
            if pet.energy < 15.0 {
                Some(format!(
                    "{} is far too tired to play. Try `/rest` to let them nap.",
                    pet.name
                ))
            } else {
                None
            }
        },
        move |pet, species| {
            let favourite = species
                .favourite_game
                .as_deref()
                .filter(|g| g.eq_ignore_ascii_case(game))
                .map(|g| format!(" {g} is their favourite!",))
                .unwrap_or_default();
            format!(
                "\u{1F3BE} You and **{}** {description}.{favourite} {flavour}",
                pet.name
            )
        },
    )
    .await
}

// -- heal -------------------------------------------------------------------

pub async fn heal(state: &Arc<AppState>, ctx: &Context, cmd: &CommandInteraction) -> Result<()> {
    let effect = Effect {
        hunger: 0.0,
        happiness: -6.0,
        health: 34.0,
        energy: -4.0,
        xp: 16.0,
        points: 10.0,
    };

    perform(
        state,
        ctx,
        cmd,
        Care {
            action: CareAction::Heal,
            cooldown: state.config.cooldowns.heal,
            effect,
        },
        |pet| {
            if pet.health >= 90.0 {
                Some(format!("{} is in perfectly good health already.", pet.name))
            } else {
                None
            }
        },
        |pet, _| {
            format!(
                "\u{1F489} You gave **{}** medicine. It tastes awful, but it works.",
                pet.name
            )
        },
    )
    .await
}

// -- rest -------------------------------------------------------------------

pub async fn rest(state: &Arc<AppState>, ctx: &Context, cmd: &CommandInteraction) -> Result<()> {
    let Some(guild_id) = cmd.guild_id else {
        return reply_error(
            ctx,
            cmd,
            "The server pet only lives in servers, not in DMs.",
        )
        .await;
    };
    let guild_id = guild_id.get() as i64;

    let Some((mut pet, _)) = state.load_pet(guild_id).await? else {
        return reply_no_pet(ctx, cmd).await;
    };

    if !pet.alive {
        return show_memorial(state, ctx, cmd, &pet).await;
    }

    // Rest has no cooldown: it is a toggle, not a stat boost, and blocking it
    // would strand a sleeping pet nobody can wake.
    let message = if pet.asleep {
        pet.asleep = false;
        format!("\u{1F31E} **{}** stretches and wakes up.", pet.name)
    } else if pet.energy >= 95.0 {
        return reply_error(
            ctx,
            cmd,
            format!("{} is wide awake and full of energy.", pet.name),
        )
        .await;
    } else {
        pet.asleep = true;
        format!(
            "\u{1F4A4} **{}** curls up for a nap and will wake once fully rested.",
            pet.name
        )
    };

    state.save_pet(&pet).await?;

    let species = state.species.get_or_first(&pet.species);
    let (image_url, attachment) = ui::art_for(&state.art, &pet).await;
    let embed = ui::status_embed(&pet, species, image_url, now_secs(), &state.config.rates);

    let mut response = CreateInteractionResponseMessage::new()
        .content(message)
        .embed(embed);
    if let Some(file) = attachment {
        response = response.add_file(file);
    }
    respond(ctx, cmd, response).await
}

// -- shared machinery -------------------------------------------------------

/// Run a care action end to end: load, gate, apply, credit, respond.
///
/// `blocked` returns a refusal message when the action makes no sense right
/// now, and `describe` builds the success line.
async fn perform<B, D>(
    state: &Arc<AppState>,
    ctx: &Context,
    cmd: &CommandInteraction,
    care: Care,
    blocked: B,
    describe: D,
) -> Result<()>
where
    B: FnOnce(&Pet) -> Option<String>,
    D: FnOnce(&Pet, &crate::species::Species) -> String,
{
    let Some(guild_id) = cmd.guild_id else {
        return reply_error(
            ctx,
            cmd,
            "The server pet only lives in servers, not in DMs.",
        )
        .await;
    };
    let guild_id = guild_id.get() as i64;
    let user_id = cmd.user.id.get() as i64;

    let Some((mut pet, _)) = state.load_pet(guild_id).await? else {
        return reply_no_pet(ctx, cmd).await;
    };

    if !pet.alive {
        return show_memorial(state, ctx, cmd, &pet).await;
    }

    let Care {
        action,
        cooldown,
        effect,
    } = care;

    if pet.asleep && action != CareAction::Heal {
        return reply_error(
            ctx,
            cmd,
            format!(
                "{} is fast asleep. Use `/rest` to wake them first.",
                pet.name
            ),
        )
        .await;
    }

    if let Some(reason) = blocked(&pet) {
        return reply_error(ctx, cmd, reason).await;
    }

    let now = now_secs();
    if let Some(remaining) =
        cooldown_remaining(state, guild_id, &pet.id, user_id, action, cooldown, now).await?
    {
        return reply_error(
            ctx,
            cmd,
            format!(
                "You have already helped recently. You can `/{}` again in **{}**.",
                action.label(),
                ui::humanize_secs(remaining)
            ),
        )
        .await;
    }

    // Care is worth more when the pet actually needs it, so the member who
    // shows up during a crisis outranks the one topping off a full stat.
    let urgency = match action {
        CareAction::Feed => 1.0 + (100.0 - pet.hunger) / 100.0,
        CareAction::Play => 1.0 + (100.0 - pet.happiness) / 100.0,
        CareAction::Heal => 1.0 + (100.0 - pet.health) / 100.0,
    };

    let before = pet.clone();
    apply(&mut pet, effect);
    let leveled_up = pet.level() > before.level();

    state.save_pet(&pet).await?;
    db::record_care(
        &state.pool,
        guild_id,
        &pet.id,
        user_id,
        action,
        effect.points * urgency,
        now,
    )
    .await?;

    let species = state.species.get_or_first(&pet.species);
    let mut content = describe(&pet, species);
    if leveled_up {
        content.push_str(&format!(
            "\n\u{2B06}\u{FE0F} **{}** reached level **{}** \u{2014} now a {}!",
            pet.name,
            pet.level(),
            pet.stage()
        ));
    }

    let (image_url, attachment) = ui::art_for(&state.art, &pet).await;
    let embed = ui::status_embed(&pet, species, image_url, now, &state.config.rates);

    let mut response = CreateInteractionResponseMessage::new()
        .content(content)
        .embed(embed);
    if let Some(file) = attachment {
        response = response.add_file(file);
    }
    respond(ctx, cmd, response).await
}

/// Apply an effect, clamping every stat.
fn apply(pet: &mut Pet, effect: Effect) {
    pet.hunger = clamp_stat(pet.hunger + effect.hunger);
    pet.happiness = clamp_stat(pet.happiness + effect.happiness);
    pet.health = clamp_stat(pet.health + effect.health);
    pet.energy = clamp_stat(pet.energy + effect.energy);
    pet.xp = (pet.xp + effect.xp).max(0.0);
}

/// Seconds left on this member's cooldown, or `None` if they may act.
async fn cooldown_remaining(
    state: &Arc<AppState>,
    guild_id: i64,
    pet_id: &str,
    user_id: i64,
    action: CareAction,
    cooldown: Duration,
    now: i64,
) -> Result<Option<i64>> {
    if cooldown.is_zero() {
        return Ok(None);
    }

    let Some(record) = db::get_caretaker(&state.pool, guild_id, pet_id, user_id).await? else {
        return Ok(None);
    };

    let last = match action {
        CareAction::Feed => record.last_feed_at,
        CareAction::Play => record.last_play_at,
        CareAction::Heal => record.last_heal_at,
    };

    let Some(last) = last else {
        return Ok(None);
    };

    let ready_at = last + cooldown.as_secs() as i64;
    // A clock that moved backwards should not lock a member out forever.
    if ready_at > now && last <= now {
        Ok(Some(ready_at - now))
    } else {
        Ok(None)
    }
}

async fn show_memorial(
    state: &Arc<AppState>,
    ctx: &Context,
    cmd: &CommandInteraction,
    pet: &Pet,
) -> Result<()> {
    let species = state.species.get_or_first(&pet.species);
    let (image_url, attachment) = ui::art_for(&state.art, pet).await;
    let embed = ui::memorial_embed(pet, species, image_url, now_secs());

    let mut response = CreateInteractionResponseMessage::new()
        .content(format!("{} is no longer with us.", pet.name))
        .embed(embed);
    if let Some(file) = attachment {
        response = response.add_file(file);
    }
    respond(ctx, cmd, response).await
}

/// Validate and normalise a name supplied by a member.
pub fn sanitize_name(raw: &str) -> Result<String, String> {
    let name = raw.replace(['\n', '\r', '\t'], " ").trim().to_string();

    if name.is_empty() {
        return Err("A pet needs a name.".to_string());
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(format!(
            "That name is too long; keep it under {MAX_NAME_LEN} characters."
        ));
    }
    let lowered = name.to_lowercase();
    if lowered.contains("@everyone") || lowered.contains("@here") {
        return Err("Nice try. Pick a name without a mass mention in it.".to_string());
    }
    Ok(name)
}

/// Pick one of several flavour strings at random.
///
/// The generator is created and dropped inside this function so it never has
/// to be held across an await point.
fn pick(options: &[&str]) -> String {
    let mut rng = rand::thread_rng();
    options
        .choose(&mut rng)
        .copied()
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_cannot_push_stats_out_of_range() {
        let mut pet = Pet::new(1, "T".into(), "blob".into(), 1, 0);
        pet.hunger = 98.0;
        apply(
            &mut pet,
            Effect {
                hunger: 50.0,
                happiness: -500.0,
                health: 0.0,
                energy: 0.0,
                xp: 5.0,
                points: 1.0,
            },
        );
        assert_eq!(pet.hunger, 100.0);
        assert_eq!(pet.happiness, 0.0);
    }

    #[test]
    fn xp_never_goes_negative() {
        let mut pet = Pet::new(1, "T".into(), "blob".into(), 1, 0);
        apply(
            &mut pet,
            Effect {
                hunger: 0.0,
                happiness: 0.0,
                health: 0.0,
                energy: 0.0,
                xp: -100.0,
                points: 0.0,
            },
        );
        assert_eq!(pet.xp, 0.0);
    }

    #[test]
    fn names_are_trimmed_and_flattened() {
        assert_eq!(sanitize_name("  Mochi \n").unwrap(), "Mochi");
        assert_eq!(sanitize_name("Big\nDog").unwrap(), "Big Dog");
    }

    #[test]
    fn empty_names_are_rejected() {
        assert!(sanitize_name("").is_err());
        assert!(sanitize_name("    ").is_err());
    }

    #[test]
    fn overlong_names_are_rejected() {
        assert!(sanitize_name(&"a".repeat(MAX_NAME_LEN + 1)).is_err());
        assert!(sanitize_name(&"a".repeat(MAX_NAME_LEN)).is_ok());
    }

    #[test]
    fn mass_mentions_are_rejected() {
        assert!(sanitize_name("@everyone").is_err());
        assert!(sanitize_name("hi @HERE friends").is_err());
    }

    #[test]
    fn unicode_names_are_measured_in_characters_not_bytes() {
        // 32 multi-byte characters is a legal name even though it is 128 bytes.
        let name = "\u{1F600}".repeat(MAX_NAME_LEN);
        assert!(sanitize_name(&name).is_ok());
    }

    #[test]
    fn flavour_picker_always_returns_something() {
        assert!(!pick(&["only"]).is_empty());
        assert_eq!(pick(&[]), "");
    }
}
