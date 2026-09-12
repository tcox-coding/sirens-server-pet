//! Gateway event handling.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serenity::all::{Command, Context, EventHandler, GuildId, Interaction, Ready};
use serenity::async_trait;

use crate::commands;
use crate::state::AppState;
use crate::ticker;

pub struct Handler {
    state: Arc<AppState>,
    /// `ready` fires again after every reconnect; one-time setup is guarded.
    initialised: AtomicBool,
}

impl Handler {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            initialised: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!(
            user = %ready.user.name,
            guilds = ready.guilds.len(),
            "connected to Discord"
        );

        if self.initialised.swap(true, Ordering::SeqCst) {
            tracing::debug!("reconnected; skipping one-time setup");
            return;
        }

        let definitions = commands::definitions(&self.state.species);
        let count = definitions.len();

        let registration = match self.state.config.dev_guild_id {
            // Guild commands appear instantly, which makes development
            // bearable. Global commands can take up to an hour to propagate.
            Some(guild_id) => GuildId::new(guild_id)
                .set_commands(&ctx.http, definitions)
                .await
                .map(|_| format!("registered {count} commands to guild {guild_id}")),
            None => Command::set_global_commands(&ctx.http, definitions)
                .await
                .map(|_| format!("registered {count} commands globally")),
        };

        match registration {
            Ok(msg) => tracing::info!("{msg}"),
            Err(err) => tracing::error!(%err, "failed to register slash commands"),
        }

        tokio::spawn(ticker::run(Arc::clone(&self.state), Arc::clone(&ctx.http)));
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Interaction::Command(cmd) = interaction else {
            return;
        };

        let name = cmd.data.name.clone();
        if let Err(err) = commands::dispatch(&self.state, &ctx, &cmd).await {
            tracing::error!(command = %name, %err, "command failed");

            // The command may or may not have already responded. Try the
            // initial response, then fall back to a follow-up, and give up
            // quietly if both fail - the error is already logged.
            let notice = "Something went wrong handling that. It has been logged.";
            if commands::reply_error(&ctx, &cmd, notice).await.is_err() {
                let followup = serenity::all::CreateInteractionResponseFollowup::new()
                    .content(notice)
                    .ephemeral(true);
                if let Err(err) = cmd.create_followup(&ctx.http, followup).await {
                    tracing::warn!(%err, "could not tell the user the command failed");
                }
            }
        }
    }
}
