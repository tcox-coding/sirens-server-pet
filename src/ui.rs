//! Embed construction and the formatting helpers it needs.
//!
//! Stat bars, the level bar and the age badge are drawn into an image by
//! [`crate::card`]. This module wraps that image in an embed together with the
//! text Discord renders natively: the title, what the pet needs, and dates.

use anyhow::{anyhow, Result};
use serenity::all::{CreateAttachment, CreateEmbed, CreateEmbedFooter};

use crate::card;
use crate::images::{ImageIndex, PetImage};
use crate::model::{Mood, Pet};
use crate::simulation::{hours_until_death, Rates};
use crate::species::Species;

/// An embed plus every file it references. Attach all of `files` alongside
/// `embed`, or the embed's images will not load.
pub struct PetCard {
    pub embed: CreateEmbed,
    pub files: Vec<CreateAttachment>,
}

/// Compact duration for inline text, e.g. `"2d 4h"` or `"18m"`.
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

/// Friendly age with at most two units, e.g. `"3 days, 4 hours"`.
///
/// Past two weeks it counts in weeks, because "45 days" is harder to picture
/// than "6 weeks, 3 days".
pub fn humanize_age(secs: i64) -> String {
    let secs = secs.max(0);
    let minutes = secs / 60;
    let hours = secs / 3_600;
    let days = secs / 86_400;
    let weeks = days / 7;

    let unit = |n: i64, name: &str| {
        if n == 1 {
            format!("1 {name}")
        } else {
            format!("{n} {name}s")
        }
    };
    let pair = |big: String, small: i64, name: &str| {
        if small == 0 {
            big
        } else {
            format!("{big}, {}", unit(small, name))
        }
    };

    if secs < 60 {
        "Just born".to_string()
    } else if hours < 1 {
        unit(minutes, "minute")
    } else if days < 1 {
        pair(unit(hours, "hour"), minutes % 60, "min")
    } else if weeks < 2 {
        pair(unit(days, "day"), hours % 24, "hour")
    } else {
        pair(unit(weeks, "week"), days % 7, "day")
    }
}

/// Discord relative timestamp markup, which renders in each viewer's timezone.
pub fn relative_timestamp(unix_secs: i64) -> String {
    format!("<t:{unix_secs}:R>")
}

/// Discord long-date markup, e.g. "12 September 2026" in the viewer's locale.
fn long_date(unix_secs: i64) -> String {
    format!("<t:{unix_secs}:D>")
}

/// Build the embed and attachments that show a pet.
///
/// A living pet gets the rendered status card as its main image. A dead pet
/// gets the memorial with its artwork. If the card cannot be rendered, the
/// stats fall back to plain embed fields so the command still answers.
pub async fn pet_card(
    index: &ImageIndex,
    pet: &Pet,
    species: &Species,
    now: i64,
    rates: &Rates,
) -> PetCard {
    let image = index.resolve(
        &pet.species,
        pet.stage(),
        pet.mood(),
        pet.custom_image_url.as_deref(),
    );

    if !pet.alive {
        let (image_url, files) = memorial_image(&image).await;
        return PetCard {
            embed: memorial_embed(pet, species, image_url, now),
            files,
        };
    }

    let mut files = Vec::new();
    let mut thumbnail = None;
    let mut art_png = None;

    match &image {
        PetImage::File { path, .. } if is_png(path) => match tokio::fs::read(path).await {
            Ok(bytes) => art_png = Some(bytes),
            Err(err) => tracing::warn!(path = %path.display(), %err, "pet art could not be read"),
        },
        PetImage::File { .. } => {
            // Only PNG art is drawn into the card. Other formats, including
            // animated GIFs, are shown untouched as the embed thumbnail.
            let (url, extra) = image_as_attachment(&image).await;
            thumbnail = url;
            files.extend(extra);
        }
        // Remote images are never fetched by the bot; Discord loads them.
        PetImage::Remote(url) => thumbnail = Some(url.clone()),
        PetImage::None => {}
    }

    let rendered = {
        let (pet, species) = (pet.clone(), species.clone());
        // Rasterising takes a few milliseconds of CPU; keep it off the
        // async workers that drive the gateway connection.
        tokio::task::spawn_blocking(move || card::render(&pet, &species, art_png.as_deref(), now))
            .await
            .unwrap_or_else(|join| Err(anyhow!("the card renderer panicked: {join}")))
    };

    let mut embed = status_embed(pet, rates);
    match rendered {
        Ok(png) => {
            files.push(CreateAttachment::bytes(png, card::FILENAME));
            embed = embed.image(format!("attachment://{}", card::FILENAME));
        }
        Err(err) => {
            tracing::error!(%err, "could not render the status card; using text stats");
            embed = with_text_stats(embed, pet, now);
        }
    }
    if let Some(url) = thumbnail {
        embed = embed.thumbnail(url);
    }

    PetCard { embed, files }
}

/// The text around the status card. Stats themselves live in the image.
fn status_embed(pet: &Pet, rates: &Rates) -> CreateEmbed {
    let mood = pet.mood();
    let mut embed = CreateEmbed::new()
        .title(format!("{} {}", mood.emoji(), pet.name))
        .description(format!(
            "Born {} \u{2022} {}",
            long_date(pet.born_at),
            relative_timestamp(pet.born_at)
        ))
        .colour(mood.colour());

    if let Some(hint) = care_hint(pet, rates) {
        embed = embed.field("\u{1F4CC} Needs attention", hint, false);
    }

    embed.footer(CreateEmbedFooter::new(format!(
        "Overall wellbeing {:.0}%",
        pet.wellbeing()
    )))
}

/// Plain-text stats, used only when the card image could not be produced.
fn with_text_stats(embed: CreateEmbed, pet: &Pet, now: i64) -> CreateEmbed {
    let (into, needed) = pet.level_progress();
    embed
        .field("Hunger", format!("**{:.0}** / 100", pet.hunger), true)
        .field("Happiness", format!("**{:.0}** / 100", pet.happiness), true)
        .field("Health", format!("**{:.0}** / 100", pet.health), true)
        .field("Energy", format!("**{:.0}** / 100", pet.energy), true)
        .field(
            "Level",
            format!("**{}** ({:.0} / {:.0} XP)", pet.level(), into, needed),
            true,
        )
        .field("Age", humanize_age(pet.age_secs(now)), true)
}

/// The card shown once a pet has died.
fn memorial_embed(
    pet: &Pet,
    species: &Species,
    image_url: Option<String>,
    now: i64,
) -> CreateEmbed {
    let lived = humanize_age(pet.age_secs(now));
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

/// How much the memorial enlarges sprites. Discord shows an embed image at
/// its native size, and a 128px sprite is postage-stamp small.
const MEMORIAL_UPSCALE: u32 = 3;

/// Sprites bigger than this are sent as they are.
const MEMORIAL_UPSCALE_MAX_SIDE: u32 = 256;

/// The memorial's image: local PNG sprites enlarged crisply, anything else
/// sent untouched.
async fn memorial_image(image: &PetImage) -> (Option<String>, Vec<CreateAttachment>) {
    if let PetImage::File { path, filename } = image {
        if is_png(path) {
            if let Ok(bytes) = tokio::fs::read(path).await {
                let enlarged = tokio::task::spawn_blocking(move || {
                    card::upscale_png(&bytes, MEMORIAL_UPSCALE, MEMORIAL_UPSCALE_MAX_SIDE)
                })
                .await;
                match enlarged {
                    Ok(Ok(png)) => {
                        return (
                            image.embed_url(),
                            vec![CreateAttachment::bytes(png, filename.clone())],
                        )
                    }
                    Ok(Err(err)) => tracing::warn!(%err, "could not enlarge the memorial sprite"),
                    Err(err) => tracing::warn!(%err, "the sprite upscaler panicked"),
                }
            }
        }
    }
    image_as_attachment(image).await
}

/// The image as an embed URL plus the upload it needs, if any.
async fn image_as_attachment(image: &PetImage) -> (Option<String>, Vec<CreateAttachment>) {
    let attachment = attachment_for(image).await.unwrap_or(None);
    match (image, attachment) {
        // The file vanished: drop the `attachment://` reference too, or
        // Discord renders a broken image.
        (PetImage::File { .. }, None) => (None, Vec::new()),
        (_, attachment) => (image.embed_url(), attachment.into_iter().collect()),
    }
}

fn is_png(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn species() -> Species {
        Species {
            key: "cat".into(),
            name: "Cat".into(),
            emoji: String::new(),
            description: String::new(),
            favourite_food: None,
            favourite_game: None,
        }
    }

    fn json(embed: &CreateEmbed) -> serde_json::Value {
        serde_json::to_value(embed).unwrap()
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
    fn ages_read_like_a_person_would_say_them() {
        assert_eq!(humanize_age(-1), "Just born");
        assert_eq!(humanize_age(30), "Just born");
        assert_eq!(humanize_age(60), "1 minute");
        assert_eq!(humanize_age(59 * 60), "59 minutes");
        assert_eq!(humanize_age(3_600), "1 hour");
        assert_eq!(humanize_age(5 * 3_600 + 12 * 60), "5 hours, 12 mins");
        assert_eq!(humanize_age(86_400), "1 day");
        assert_eq!(humanize_age(3 * 86_400 + 4 * 3_600), "3 days, 4 hours");
        assert_eq!(humanize_age(13 * 86_400), "13 days");
        assert_eq!(humanize_age(45 * 86_400), "6 weeks, 3 days");
        assert_eq!(humanize_age(14 * 86_400), "2 weeks");
    }

    #[test]
    fn a_healthy_pet_gets_no_nag() {
        let mut pet = Pet::new(1, "Ok".into(), "cat".into(), 1, 0);
        pet.hunger = 100.0;
        pet.happiness = 100.0;
        pet.health = 100.0;
        pet.energy = 100.0;
        assert_eq!(care_hint(&pet, &Rates::default()), None);
    }

    #[test]
    fn a_starving_pet_is_told_to_be_fed() {
        let mut pet = Pet::new(1, "Hungry".into(), "cat".into(), 1, 0);
        pet.hunger = 5.0;
        let hint = care_hint(&pet, &Rates::default()).expect("a hint");
        assert!(hint.contains("/feed"), "got {hint}");
    }

    #[tokio::test]
    async fn a_living_pet_gets_the_rendered_card() {
        let index = ImageIndex::scan(Path::new("assets/pets")).unwrap();
        let pet = Pet::new(1, "Mochi".into(), "cat".into(), 1, 0);
        let card = pet_card(&index, &pet, &species(), 3_600, &Rates::default()).await;

        let names: Vec<_> = card.files.iter().map(|f| f.filename.as_str()).collect();
        assert_eq!(names, vec![card::FILENAME]);
        assert_eq!(
            json(&card.embed)["image"]["url"],
            format!("attachment://{}", card::FILENAME)
        );
        // The stats are in the image, so no text fields duplicate them.
        assert!(json(&card.embed)["fields"]
            .as_array()
            .map_or(true, |f| f.iter().all(|f| f["name"] != "Hunger")));
    }

    #[tokio::test]
    async fn a_remote_image_becomes_the_thumbnail() {
        let index = ImageIndex::default();
        let mut pet = Pet::new(1, "Mochi".into(), "cat".into(), 1, 0);
        pet.custom_image_url = Some("https://example.com/mochi.png".into());
        let card = pet_card(&index, &pet, &species(), 0, &Rates::default()).await;

        assert_eq!(
            json(&card.embed)["thumbnail"]["url"],
            "https://example.com/mochi.png"
        );
        assert_eq!(card.files.len(), 1, "only the rendered card is uploaded");
    }

    #[tokio::test]
    async fn a_dead_pet_gets_the_memorial_not_the_card() {
        let index = ImageIndex::scan(Path::new("assets/pets")).unwrap();
        let mut pet = Pet::new(1, "Rest".into(), "cat".into(), 1, 0);
        pet.alive = false;
        pet.died_at = Some(100);
        let card = pet_card(&index, &pet, &species(), 200, &Rates::default()).await;

        assert!(card.files.iter().all(|f| f.filename != card::FILENAME));

        let sprite = card
            .files
            .iter()
            .find(|f| f.filename.starts_with("pet_cat_dead"))
            .expect("the memorial carries the dead sprite");
        let enlarged = tiny_skia::Pixmap::decode_png(&sprite.data).unwrap();
        assert_eq!(enlarged.width(), 128 * MEMORIAL_UPSCALE);
    }
}
