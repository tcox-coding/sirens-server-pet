//! Leaderboard, memorial listing, help, and the management commands.

use std::sync::Arc;

use anyhow::Result;
use serenity::all::{
    CommandInteraction, Context, CreateAllowedMentions, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponseMessage,
};

use super::{
    is_manager, opt_bool, opt_channel, opt_str, reply_embed, reply_error, reply_no_pet, respond,
};
use crate::db;
use crate::state::{now_secs, AppState};
use crate::ui;

/// How many rows the leaderboard and graveyard show.
const LIST_LIMIT: i64 = 10;

/// Mentions render as names without pinging anyone.
fn silent() -> CreateAllowedMentions {
    CreateAllowedMentions::new()
        .everyone(false)
        .all_users(false)
        .all_roles(false)
}

/// Guard for the management commands. Discord's own permission gate is only a
/// default that a server owner can override, so this is the real check.
fn require_manager(cmd: &CommandInteraction) -> Result<(), &'static str> {
    if is_manager(cmd) {
        Ok(())
    } else {
        Err("You need the Manage Server permission to do that.")
    }
}

fn guild_of(cmd: &CommandInteraction) -> Option<i64> {
    cmd.guild_id.map(|g| g.get() as i64)
}

// -- caretakers -------------------------------------------------------------

pub async fn caretakers(
    state: &Arc<AppState>,
    ctx: &Context,
    cmd: &CommandInteraction,
) -> Result<()> {
    let Some(guild_id) = guild_of(cmd) else {
        return reply_error(
            ctx,
            cmd,
            "The server pet only lives in servers, not in DMs.",
        )
        .await;
    };

    let Some((pet, _)) = state.load_pet(guild_id).await? else {
        return reply_no_pet(ctx, cmd).await;
    };

    let board = db::leaderboard(&state.pool, guild_id, &pet.id, LIST_LIMIT).await?;
    let total = db::caretaker_count(&state.pool, guild_id, &pet.id).await?;

    let body = if board.is_empty() {
        format!(
            "Nobody has looked after {} yet. Be the first with `/feed`.",
            pet.name
        )
    } else {
        board
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let medal = match i {
                    0 => "\u{1F947}".to_string(),
                    1 => "\u{1F948}".to_string(),
                    2 => "\u{1F949}".to_string(),
                    n => format!("`{}.`", n + 1),
                };
                format!(
                    "{medal} <@{}> \u{2014} **{:.0}** points \u{2022} {} meals, {} games, {} treatments",
                    c.user_id, c.care_points, c.feeds, c.plays, c.heals
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let embed = CreateEmbed::new()
        .title(format!("\u{1F3C6} Who looks after {}", pet.name))
        .description(body)
        .colour(pet.mood().colour())
        .footer(CreateEmbedFooter::new(format!(
            "{total} caretaker(s) \u{2022} points count for more when the pet actually needs help"
        )));

    respond(
        ctx,
        cmd,
        CreateInteractionResponseMessage::new()
            .embed(embed)
            .allowed_mentions(silent()),
    )
    .await
}

// -- graveyard --------------------------------------------------------------

pub async fn graveyard(
    state: &Arc<AppState>,
    ctx: &Context,
    cmd: &CommandInteraction,
) -> Result<()> {
    let Some(guild_id) = guild_of(cmd) else {
        return reply_error(
            ctx,
            cmd,
            "The server pet only lives in servers, not in DMs.",
        )
        .await;
    };

    let graves = db::graveyard(&state.pool, guild_id, LIST_LIMIT).await?;

    let body = if graves.is_empty() {
        "No pets have been lost here. Keep it that way.".to_string()
    } else {
        graves
            .iter()
            .map(|g| {
                let species = state.species.get_or_first(&g.species);
                format!(
                    "\u{1FAA6} **{}** \u{2022} {} \u{2022} level {} \u{2022} lived {} \u{2022} {} ({})",
                    g.name,
                    species.name,
                    g.level,
                    ui::humanize_secs(g.died_at - g.born_at),
                    g.cause,
                    ui::relative_timestamp(g.died_at),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let embed = CreateEmbed::new()
        .title("\u{1F5FF} In memory")
        .description(body)
        .colour(0x2C2F33);

    reply_embed(ctx, cmd, embed).await
}

// -- help -------------------------------------------------------------------

pub async fn help(state: &Arc<AppState>, ctx: &Context, cmd: &CommandInteraction) -> Result<()> {
    let species_list = state
        .species
        .all()
        .iter()
        .map(|s| {
            if s.description.is_empty() {
                format!("\u{2022} {}", s.display())
            } else {
                format!("\u{2022} {} \u{2014} {}", s.display(), s.description)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let embed = CreateEmbed::new()
        .title("\u{1F43E} The server pet")
        .description(
            "One pet lives in this server and everyone shares responsibility for it. \
             Its stats fall as real time passes, whether or not anyone is watching, \
             and if health reaches zero the pet dies for good.",
        )
        .colour(0x5865F2)
        .field(
            "Looking after it",
            "`/pet` check on it\n\
             `/feed` restore hunger\n\
             `/play` restore happiness, costs energy\n\
             `/rest` nap to recover energy\n\
             `/heal` treat it when health is low",
            false,
        )
        .field(
            "Everything else",
            "`/adopt` bring home a new pet\n\
             `/caretakers` who has done the most\n\
             `/graveyard` pets who came before",
            false,
        )
        .field(
            "Server managers",
            "`/petrename` rename the pet\n\
             `/petimage` use a custom picture\n\
             `/petalerts` pick a channel for warnings\n\
             `/petrelease` retire the current pet",
            false,
        )
        .field("Species", species_list, false)
        .footer(CreateEmbedFooter::new(
            "Care is worth more when the pet actually needs it.",
        ));

    reply_embed(ctx, cmd, embed).await
}

// -- rename -----------------------------------------------------------------

pub async fn rename(state: &Arc<AppState>, ctx: &Context, cmd: &CommandInteraction) -> Result<()> {
    if let Err(why) = require_manager(cmd) {
        return reply_error(ctx, cmd, why).await;
    }
    let Some(guild_id) = guild_of(cmd) else {
        return reply_error(
            ctx,
            cmd,
            "The server pet only lives in servers, not in DMs.",
        )
        .await;
    };

    let name = match super::care::sanitize_name(opt_str(cmd, "name").unwrap_or_default()) {
        Ok(name) => name,
        Err(why) => return reply_error(ctx, cmd, why).await,
    };

    let Some((mut pet, _)) = state.load_pet(guild_id).await? else {
        return reply_no_pet(ctx, cmd).await;
    };

    let previous = std::mem::replace(&mut pet.name, name);
    state.save_pet(&pet).await?;

    super::reply_text(
        ctx,
        cmd,
        format!("**{previous}** is now known as **{}**.", pet.name),
    )
    .await
}

// -- custom image -----------------------------------------------------------

pub async fn set_image(
    state: &Arc<AppState>,
    ctx: &Context,
    cmd: &CommandInteraction,
) -> Result<()> {
    if let Err(why) = require_manager(cmd) {
        return reply_error(ctx, cmd, why).await;
    }
    let Some(guild_id) = guild_of(cmd) else {
        return reply_error(
            ctx,
            cmd,
            "The server pet only lives in servers, not in DMs.",
        )
        .await;
    };

    let Some((mut pet, _)) = state.load_pet(guild_id).await? else {
        return reply_no_pet(ctx, cmd).await;
    };

    let raw = opt_str(cmd, "url").unwrap_or("").trim();

    if raw.is_empty() {
        pet.custom_image_url = None;
        state.save_pet(&pet).await?;
        return super::reply_text(
            ctx,
            cmd,
            format!("**{}** is back to its built-in artwork.", pet.name),
        )
        .await;
    }

    if let Err(why) = validate_image_url(raw) {
        return reply_error(ctx, cmd, why).await;
    }

    pet.custom_image_url = Some(raw.to_string());
    state.save_pet(&pet).await?;

    let species = state.species.get_or_first(&pet.species);
    let (image_url, _) = ui::art_for(&state.art, &pet).await;
    let embed = ui::status_embed(&pet, species, image_url, now_secs(), &state.config.rates);

    respond(
        ctx,
        cmd,
        CreateInteractionResponseMessage::new()
            .content(format!("**{}** has a new look.", pet.name))
            .embed(embed),
    )
    .await
}

/// Reject anything Discord will not render, before it becomes a broken embed.
///
/// A custom image replaces the local art entirely, so a bad value here shows
/// up on every single status card until someone notices.
pub fn validate_image_url(url: &str) -> Result<(), String> {
    const MAX_URL_LEN: usize = 512;

    if url.len() > MAX_URL_LEN {
        return Err(format!(
            "That URL is too long; keep it under {MAX_URL_LEN} characters."
        ));
    }
    if !url.starts_with("https://") {
        return Err("The image URL has to start with `https://`.".to_string());
    }
    if url.chars().any(|c| c.is_whitespace()) {
        return Err("That URL contains spaces, so it is not a direct link.".to_string());
    }

    // Look at the path only; query strings on CDN links are normal and should
    // not disqualify an otherwise valid image.
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    let ok = [".png", ".jpg", ".jpeg", ".gif", ".webp"]
        .iter()
        .any(|ext| path.ends_with(ext));
    if !ok {
        return Err(
            "That does not look like a direct image link. It should end in .png, .jpg, .gif or .webp."
                .to_string(),
        );
    }

    Ok(())
}

// -- alerts -----------------------------------------------------------------

pub async fn alerts(state: &Arc<AppState>, ctx: &Context, cmd: &CommandInteraction) -> Result<()> {
    if let Err(why) = require_manager(cmd) {
        return reply_error(ctx, cmd, why).await;
    }
    let Some(guild_id) = guild_of(cmd) else {
        return reply_error(
            ctx,
            cmd,
            "The server pet only lives in servers, not in DMs.",
        )
        .await;
    };

    let mut settings = db::get_settings(&state.pool, guild_id).await?;
    let channel = opt_channel(cmd, "channel");
    let enabled = opt_bool(cmd, "enabled");

    if channel.is_none() && enabled.is_none() {
        // Nothing to change: report the current setup.
        let where_to = match settings.alert_channel_id {
            Some(id) => format!("<#{id}>"),
            None => "nowhere yet".to_string(),
        };
        return reply_error(
            ctx,
            cmd,
            format!(
                "Alerts are **{}** and go to {where_to}. Pass `channel` or `enabled` to change that.",
                if settings.alerts_enabled { "on" } else { "off" }
            ),
        )
        .await;
    }

    if let Some(channel) = channel {
        settings.alert_channel_id = Some(channel.get() as i64);
        // Choosing a channel implies wanting alerts, unless told otherwise in
        // the same command.
        settings.alerts_enabled = enabled.unwrap_or(true);
    }
    if let Some(enabled) = enabled {
        settings.alerts_enabled = enabled;
    }

    db::save_settings(&state.pool, &settings).await?;

    let message = match (settings.alerts_enabled, settings.alert_channel_id) {
        (true, Some(id)) => format!("The pet will ask for help in <#{id}>."),
        (true, None) => {
            "Alerts are on, but no channel is set. Run `/petalerts channel:#somewhere`.".to_string()
        }
        (false, _) => "Alerts are off. The pet will suffer in silence.".to_string(),
    };

    super::reply_text(ctx, cmd, message).await
}

// -- release ----------------------------------------------------------------

pub async fn release(state: &Arc<AppState>, ctx: &Context, cmd: &CommandInteraction) -> Result<()> {
    if let Err(why) = require_manager(cmd) {
        return reply_error(ctx, cmd, why).await;
    }
    let Some(guild_id) = guild_of(cmd) else {
        return reply_error(
            ctx,
            cmd,
            "The server pet only lives in servers, not in DMs.",
        )
        .await;
    };

    let Some((pet, _)) = state.load_pet(guild_id).await? else {
        return reply_no_pet(ctx, cmd).await;
    };

    // Releasing is irreversible, so require the pet's name rather than a
    // yes/no anyone can click through.
    let confirm = opt_str(cmd, "confirm").unwrap_or_default().trim();
    if !confirm.eq_ignore_ascii_case(&pet.name) {
        return reply_error(
            ctx,
            cmd,
            format!(
                "To retire this pet, run the command again with `confirm:{}`.",
                pet.name
            ),
        )
        .await;
    }

    let mut record = pet.clone();
    if record.alive {
        record.alive = false;
        record.died_at = Some(now_secs());
        record.cause_of_death = Some("rehomed".to_string());
    }
    db::bury(&state.pool, &record).await?;
    db::delete_pet(&state.pool, guild_id).await?;

    super::reply_text(
        ctx,
        cmd,
        format!(
            "**{}** has gone to a quiet farm upstate. Use `/adopt` when the server is ready again.",
            pet.name
        ),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_direct_https_image() {
        assert!(validate_image_url("https://cdn.example.com/pets/blob.png").is_ok());
        assert!(validate_image_url("https://cdn.example.com/a.GIF?width=100").is_ok());
        assert!(validate_image_url("https://x.dev/b.webp#frag").is_ok());
    }

    #[test]
    fn rejects_insecure_or_indirect_links() {
        assert!(validate_image_url("http://example.com/a.png").is_err());
        assert!(validate_image_url("ftp://example.com/a.png").is_err());
        assert!(validate_image_url("https://imgur.com/gallery/abc").is_err());
        assert!(validate_image_url("example.com/a.png").is_err());
    }

    #[test]
    fn rejects_urls_with_whitespace() {
        assert!(validate_image_url("https://example.com/my pet.png").is_err());
    }

    #[test]
    fn rejects_absurdly_long_urls() {
        let url = format!("https://example.com/{}.png", "a".repeat(600));
        assert!(validate_image_url(&url).is_err());
    }
}
