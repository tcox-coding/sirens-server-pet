//! Embed construction and the small formatting helpers it needs.
//!
//! Everything here is deliberately pure except [`attachment_for`], so the
//! layout can be unit tested without a Discord connection.

use anyhow::Result;
use serenity::all::{CreateAttachment, CreateEmbed, CreateEmbedFooter};

use crate::images::{ImageIndex, PetImage};
use crate::model::{Mood, Pet};
use crate::simulation::{hours_until_death, Rates};
use crate::species::Species;

/// Number of segments in a stat bar.
const BAR_WIDTH: usize = 12;

const BAR_FULL: char = '\u{2588}';
const BAR_EMPTY: char = '\u{2591}';

/// Render a 0..100 value as a fixed-width bar.
pub fn bar(value: f64) -> String {
    let ratio = (value / 100.0).clamp(0.0, 1.0);
    // Round so that anything above zero shows at least one segment, and only a
    // true 100 shows a completely full bar.
    let mut filled = (ratio * BAR_WIDTH as f64).round() as usize;
    if filled == 0 && value > 0.0 {
        filled = 1;
    }
    if filled == BAR_WIDTH && ratio < 1.0 {
        filled = BAR_WIDTH - 1;
    }
    let mut s = String::with_capacity(BAR_WIDTH);
    for i in 0..BAR_WIDTH {
        s.push(if i < filled { BAR_FULL } else { BAR_EMPTY });
    }
    s
}

/// `"Hunger  ████████░░░░  67"`, ready to drop into an embed field.
pub fn stat_line(value: f64) -> String {
    format!("`{}` **{:.0}**", bar(value), value)
}

/// Human-readable duration, e.g. `"2d 4h"` or `"18m"`.
pub fn humanize_secs(secs: i64) -> String {
    let secs = secs.max(0);
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;

    if days > 0 {
        if hours > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{days}d")
        }
    } else if hours > 0 {
        if minutes > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{hours}h")
        }
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{secs}s")
    }
}

/// Discord relative timestamp markup, which renders in each viewer's timezone.
pub fn relative_timestamp(unix_secs: i64) -> String {
    format!("<t:{unix_secs}:R>")
}

/// Build the attachment for an image, if it is a local file.
///
/// Returns `Ok(None)` when the file has gone missing since the index was
/// built, so a deleted sprite degrades to a text-only embed rather than
/// failing the command.
pub async fn attachment_for(image: &PetImage) -> Result<Option<CreateAttachment>> {
    match image {
        PetImage::File { path, filename } => match CreateAttachment::path(path).await {
            Ok(mut attachment) => {
                attachment.filename = filename.clone();
                Ok(Some(attachment))
            }
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "pet art could not be read");
                Ok(None)
            }
        },
        _ => Ok(None),
    }
}

/// The full status card.
pub fn status_embed(
    pet: &Pet,
    species: &Species,
    image_url: Option<String>,
    now: i64,
    rates: &Rates,
) -> CreateEmbed {
    let mood = pet.mood();

    if !pet.alive {
        return memorial_embed(pet, species, image_url, now);
    }

    let (into_level, needed) = pet.level_progress();
    let mut embed = CreateEmbed::new()
        .title(format!("{} {}", mood.emoji(), pet.name))
        .description(format!(
            "{} \u{2022} **{}** \u{2022} Level {} \u{2022} {}",
            species.display(),
            pet.stage(),
            pet.level(),
            mood.label()
        ))
        .colour(mood.colour())
        .field("\u{1F356} Hunger", stat_line(pet.hunger), true)
        .field("\u{1F49B} Happiness", stat_line(pet.happiness), true)
        .field("\u{2764}\u{FE0F} Health", stat_line(pet.health), true)
        .field("\u{26A1} Energy", stat_line(pet.energy), true)
        .field(
            "\u{2728} Experience",
            format!(
                "`{}` {:.0}/{:.0}",
                bar(into_level / needed.max(1.0) * 100.0),
                into_level,
                needed
            ),
            true,
        )
        .field("\u{1F382} Age", humanize_secs(pet.age_secs(now)), true);

    if let Some(hint) = care_hint(pet, rates) {
        embed = embed.field("\u{1F4CC} Needs attention", hint, false);
    }

    if let Some(url) = image_url {
        embed = embed.image(url);
    }

    embed.footer(CreateEmbedFooter::new(format!(
        "Adopted {} \u{2022} overall wellbeing {:.0}%",
        humanize_secs(pet.age_secs(now)),
        pet.wellbeing()
    )))
}

/// The card shown once a pet has died.
pub fn memorial_embed(
    pet: &Pet,
    species: &Species,
    image_url: Option<String>,
    now: i64,
) -> CreateEmbed {
    let lived = humanize_secs(pet.age_secs(now));
    let cause = pet.cause_of_death.as_deref().unwrap_or("neglect");

    let mut embed = CreateEmbed::new()
        .title(format!("{} {}", Mood::Dead.emoji(), pet.name))
        .description(format!(
            "{} \u{2022} reached level {}\n\n*{} lived for {lived} before succumbing to {cause}.*\n\nUse `/adopt` to welcome a new pet to the server.",
            species.display(),
            pet.level(),
            pet.name,
        ))
        .colour(Mood::Dead.colour());

    if let Some(died_at) = pet.died_at {
        embed = embed.field("Died", relative_timestamp(died_at), true);
    }
    embed = embed.field("Lifespan", lived, true);

    if let Some(url) = image_url {
        embed = embed.image(url);
    }
    embed
}

/// The single most useful thing a member could do right now, or `None` when
/// the pet is comfortable.
fn care_hint(pet: &Pet, rates: &Rates) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();

    if pet.health < 40.0 {
        lines.push(format!("`/heal` \u{2013} {} is unwell.", pet.name));
    }
    if pet.hunger < 30.0 {
        lines.push(format!("`/feed` \u{2013} {} is going hungry.", pet.name));
    }
    if pet.happiness < 30.0 {
        lines.push(format!("`/play` \u{2013} {} is lonely.", pet.name));
    }
    if pet.asleep {
        lines.push(format!("{} is asleep and recovering energy.", pet.name));
    } else if pet.energy < 20.0 {
        lines.push(format!(
            "`/rest` \u{2013} {} is running on empty.",
            pet.name
        ));
    }

    if let Some(hours) = hours_until_death(pet, rates) {
        if hours <= 12.0 {
            lines.push(format!(
                "\u{26A0}\u{FE0F} Without care, {} has about **{}** left.",
                pet.name,
                humanize_secs((hours * 3600.0) as i64)
            ));
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Resolve art and turn it into `(embed image url, attachment)` in one step.
pub async fn art_for(index: &ImageIndex, pet: &Pet) -> (Option<String>, Option<CreateAttachment>) {
    let image = index.resolve(
        &pet.species,
        pet.stage(),
        pet.mood(),
        pet.custom_image_url.as_deref(),
    );
    let attachment = attachment_for(&image).await.unwrap_or(None);

    // If the file vanished, drop the `attachment://` reference too, otherwise
    // Discord renders a broken image.
    match (&image, &attachment) {
        (PetImage::File { .. }, None) => (None, None),
        _ => (image.embed_url(), attachment),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_span_the_full_range() {
        assert_eq!(bar(0.0).chars().filter(|c| *c == BAR_FULL).count(), 0);
        assert_eq!(
            bar(100.0).chars().filter(|c| *c == BAR_FULL).count(),
            BAR_WIDTH
        );
        assert_eq!(
            bar(50.0).chars().filter(|c| *c == BAR_FULL).count(),
            BAR_WIDTH / 2
        );
    }

    #[test]
    fn bars_are_always_the_same_width() {
        for v in [-10.0, 0.0, 0.4, 33.3, 99.6, 100.0, 250.0] {
            assert_eq!(bar(v).chars().count(), BAR_WIDTH, "value {v}");
        }
    }

    #[test]
    fn a_barely_alive_stat_still_shows_something() {
        assert!(
            bar(0.5).starts_with(BAR_FULL),
            "tiny values should not read as empty"
        );
    }

    #[test]
    fn a_nearly_full_stat_is_not_shown_as_full() {
        let filled = bar(99.9).chars().filter(|c| *c == BAR_FULL).count();
        assert_eq!(filled, BAR_WIDTH - 1);
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(humanize_secs(0), "0s");
        assert_eq!(humanize_secs(45), "45s");
        assert_eq!(humanize_secs(90), "1m");
        assert_eq!(humanize_secs(3_600), "1h");
        assert_eq!(humanize_secs(3_660), "1h 1m");
        assert_eq!(humanize_secs(86_400), "1d");
        assert_eq!(humanize_secs(100_000), "1d 3h");
        assert_eq!(humanize_secs(-5), "0s");
    }

    #[test]
    fn a_healthy_pet_gets_no_nag() {
        let mut pet = Pet::new(1, "Ok".into(), "blob".into(), 1, 0);
        pet.hunger = 100.0;
        pet.happiness = 100.0;
        pet.health = 100.0;
        pet.energy = 100.0;
        assert_eq!(care_hint(&pet, &Rates::default()), None);
    }

    #[test]
    fn a_starving_pet_is_told_to_be_fed() {
        let mut pet = Pet::new(1, "Hungry".into(), "blob".into(), 1, 0);
        pet.hunger = 5.0;
        let hint = care_hint(&pet, &Rates::default()).expect("a hint");
        assert!(hint.contains("/feed"), "got {hint}");
    }
}
