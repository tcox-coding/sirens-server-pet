//! Slash command registration, dispatch, and the helpers commands share.

pub mod admin;
pub mod care;

use std::sync::Arc;

use anyhow::Result;
use serenity::all::{
    CommandDataOptionValue, CommandInteraction, CommandOptionType, Context, CreateCommand,
    CreateCommandOption, CreateEmbed, CreateInteractionResponse, CreateInteractionResponseMessage,
    Permissions,
};

use crate::species::Registry;
use crate::state::AppState;

/// Every command the bot registers.
///
/// Names are flat rather than subcommands of a single `/pet` group: members
/// type `/feed` far more often than anything else, and a flat list keeps the
/// common actions one keystroke away in Discord's picker.
pub fn definitions(species: &Registry) -> Vec<CreateCommand> {
    let mut species_option = CreateCommandOption::new(
        CommandOptionType::String,
        "species",
        "Which kind of pet to adopt",
    )
    .required(true);

    for sp in species.all() {
        // Discord shows the label; the bot receives the key.
        species_option = species_option.add_string_choice(sp.display(), sp.key.clone());
    }

    vec![
        CreateCommand::new("adopt")
            .description("Adopt a new pet for this server")
            .add_option(species_option)
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "name",
                    "What should the pet be called?",
                )
                .required(true)
                .min_length(1)
                .max_length(32),
            ),
        CreateCommand::new("pet").description("Check on the server pet"),
        CreateCommand::new("feed")
            .description("Feed the server pet")
            .add_option(
                CreateCommandOption::new(CommandOptionType::String, "food", "What to serve")
                    .add_string_choice("Snack", "snack")
                    .add_string_choice("Full meal", "meal")
                    .add_string_choice("Sweet treat", "treat")
                    .add_string_choice("Something healthy", "greens"),
            ),
        CreateCommand::new("play")
            .description("Play with the server pet")
            .add_option(
                CreateCommandOption::new(CommandOptionType::String, "game", "What to play")
                    .add_string_choice("Fetch", "fetch")
                    .add_string_choice("Puzzle", "puzzle")
                    .add_string_choice("Dance party", "dance"),
            ),
        CreateCommand::new("rest").description("Put the pet down for a nap, or wake it up"),
        CreateCommand::new("heal").description("Give the pet medicine when it is unwell"),
        CreateCommand::new("caretakers").description("See who has done the most for the pet"),
        CreateCommand::new("graveyard").description("Remember the pets who came before"),
        CreateCommand::new("pethelp").description("How the server pet works"),
        CreateCommand::new("petrename")
            .description("Rename the server pet")
            .default_member_permissions(Permissions::MANAGE_GUILD)
            .add_option(
                CreateCommandOption::new(CommandOptionType::String, "name", "The new name")
                    .required(true)
                    .min_length(1)
                    .max_length(32),
            ),
        CreateCommand::new("petimage")
            .description("Override the pet's picture with an image URL")
            .default_member_permissions(Permissions::MANAGE_GUILD)
            .add_option(CreateCommandOption::new(
                CommandOptionType::String,
                "url",
                "Direct link to a png, jpg, gif or webp. Leave empty to restore the default art.",
            )),
        CreateCommand::new("petalerts")
            .description("Choose where the pet asks for help")
            .default_member_permissions(Permissions::MANAGE_GUILD)
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::Channel,
                    "channel",
                    "Channel for hunger and health warnings",
                )
                .required(false),
            )
            .add_option(CreateCommandOption::new(
                CommandOptionType::Boolean,
                "enabled",
                "Turn alerts on or off",
            )),
        CreateCommand::new("petrelease")
            .description("Retire the current pet so a new one can be adopted")
            .default_member_permissions(Permissions::MANAGE_GUILD)
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "confirm",
                    "Type the pet's name to confirm",
                )
                .required(true),
            ),
    ]
}

/// Route an interaction to its handler.
pub async fn dispatch(
    state: &Arc<AppState>,
    ctx: &Context,
    cmd: &CommandInteraction,
) -> Result<()> {
    match cmd.data.name.as_str() {
        "adopt" => care::adopt(state, ctx, cmd).await,
        "pet" => care::status(state, ctx, cmd).await,
        "feed" => care::feed(state, ctx, cmd).await,
        "play" => care::play(state, ctx, cmd).await,
        "rest" => care::rest(state, ctx, cmd).await,
        "heal" => care::heal(state, ctx, cmd).await,
        "caretakers" => admin::caretakers(state, ctx, cmd).await,
        "graveyard" => admin::graveyard(state, ctx, cmd).await,
        "pethelp" => admin::help(state, ctx, cmd).await,
        "petrename" => admin::rename(state, ctx, cmd).await,
        "petimage" => admin::set_image(state, ctx, cmd).await,
        "petalerts" => admin::alerts(state, ctx, cmd).await,
        "petrelease" => admin::release(state, ctx, cmd).await,
        other => {
            tracing::warn!(command = other, "received an unknown command");
            reply_error(ctx, cmd, "That command is not available. Try `/pethelp`.").await
        }
    }
}

// -- option access ----------------------------------------------------------

pub fn opt_str<'a>(cmd: &'a CommandInteraction, name: &str) -> Option<&'a str> {
    cmd.data
        .options
        .iter()
        .find(|o| o.name == name)
        .and_then(|o| match &o.value {
            CommandDataOptionValue::String(s) => Some(s.as_str()),
            _ => None,
        })
}

pub fn opt_bool(cmd: &CommandInteraction, name: &str) -> Option<bool> {
    cmd.data
        .options
        .iter()
        .find(|o| o.name == name)
        .and_then(|o| match &o.value {
            CommandDataOptionValue::Boolean(b) => Some(*b),
            _ => None,
        })
}

pub fn opt_channel(cmd: &CommandInteraction, name: &str) -> Option<serenity::all::ChannelId> {
    cmd.data
        .options
        .iter()
        .find(|o| o.name == name)
        .and_then(|o| match &o.value {
            CommandDataOptionValue::Channel(id) => Some(*id),
            _ => None,
        })
}

/// Whether the invoking member may run management commands.
///
/// Discord already gates these with `default_member_permissions`, but that is
/// only a default: server owners can grant the command to any role. This is
/// the real check.
pub fn is_manager(cmd: &CommandInteraction) -> bool {
    cmd.member
        .as_ref()
        .and_then(|m| m.permissions)
        .map(|p| p.contains(Permissions::MANAGE_GUILD) || p.contains(Permissions::ADMINISTRATOR))
        .unwrap_or(false)
}

// -- responses --------------------------------------------------------------

pub async fn respond(
    ctx: &Context,
    cmd: &CommandInteraction,
    message: CreateInteractionResponseMessage,
) -> Result<()> {
    cmd.create_response(&ctx.http, CreateInteractionResponse::Message(message))
        .await?;
    Ok(())
}

/// A plain, publicly visible text reply.
pub async fn reply_text(
    ctx: &Context,
    cmd: &CommandInteraction,
    text: impl Into<String>,
) -> Result<()> {
    respond(
        ctx,
        cmd,
        CreateInteractionResponseMessage::new().content(text),
    )
    .await
}

/// A reply only the invoking member can see. Used for mistakes and cooldowns,
/// so a busy channel does not fill with failed attempts.
pub async fn reply_error(
    ctx: &Context,
    cmd: &CommandInteraction,
    text: impl Into<String>,
) -> Result<()> {
    respond(
        ctx,
        cmd,
        CreateInteractionResponseMessage::new()
            .content(text)
            .ephemeral(true),
    )
    .await
}

pub async fn reply_embed(
    ctx: &Context,
    cmd: &CommandInteraction,
    embed: CreateEmbed,
) -> Result<()> {
    respond(
        ctx,
        cmd,
        CreateInteractionResponseMessage::new().embed(embed),
    )
    .await
}

/// The message shown when a command needs a pet and there is not one.
pub async fn reply_no_pet(ctx: &Context, cmd: &CommandInteraction) -> Result<()> {
    reply_error(
        ctx,
        cmd,
        "This server does not have a pet yet. Use `/adopt` to bring one home.",
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::species::{Registry, Species};

    fn species(key: &str, name: &str) -> Species {
        Species {
            key: key.into(),
            name: name.into(),
            emoji: String::new(),
            description: String::new(),
            favourite_food: None,
            favourite_game: None,
        }
    }

    fn registry(list: Vec<Species>) -> Registry {
        Registry::from_species(list).unwrap()
    }

    /// Command names that [`dispatch`] knows how to route.
    const DISPATCHED: [&str; 13] = [
        "adopt",
        "pet",
        "feed",
        "play",
        "rest",
        "heal",
        "caretakers",
        "graveyard",
        "pethelp",
        "petrename",
        "petimage",
        "petalerts",
        "petrelease",
    ];

    fn registered_names(species: &Registry) -> Vec<String> {
        definitions(species)
            .iter()
            .map(|c| {
                serde_json::to_value(c).expect("commands serialise")["name"]
                    .as_str()
                    .expect("every command has a name")
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn every_registered_command_has_a_dispatch_arm() {
        // `definitions` builds the list Discord shows members; `dispatch` must
        // know every name in it or a command silently does nothing.
        let reg = registry(vec![species("blob", "Blob")]);
        for name in registered_names(&reg) {
            assert!(
                DISPATCHED.contains(&name.as_str()),
                "command {name:?} is registered but never dispatched"
            );
        }
    }

    #[test]
    fn every_dispatched_command_is_registered() {
        let reg = registry(vec![species("blob", "Blob")]);
        let registered = registered_names(&reg);
        for name in DISPATCHED {
            assert!(
                registered.iter().any(|r| r == name),
                "command {name:?} is dispatched but never registered"
            );
        }
    }

    #[test]
    fn adopt_offers_every_species_as_a_choice() {
        let reg = registry(vec![species("blob", "Blob"), species("slime", "Slime")]);
        let adopt = definitions(&reg)
            .into_iter()
            .map(|c| serde_json::to_value(&c).unwrap())
            .find(|v| v["name"] == "adopt")
            .expect("adopt is registered");

        let choices = adopt["options"][0]["choices"].as_array().unwrap().clone();
        let keys: Vec<_> = choices
            .iter()
            .map(|c| c["value"].as_str().unwrap())
            .collect();
        assert_eq!(keys, vec!["blob", "slime"]);
    }
}
