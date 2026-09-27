//! Renders the pet's status card as a PNG.
//!
//! Discord embeds can only lay out text, so the stat bars, the level bar and
//! the age badge are drawn into one image instead. The card is attached to the
//! message and shown as the embed's main image.
//!
//! ```text
//! +-----------+  Name                          [ Mood ]
//! |           |  Species  ·  Stage
//! |    art    |  HUNGER        67   HAPPINESS       80
//! |           |  [=========     ]   [===========    ]
//! |           |  HEALTH        95   ENERGY          40
//! | [AGE 2d]  |  [============= ]   [=====          ]
//! +-----------+  LEVEL 3                     45 / 200 XP
//!                [======                              ]
//! ```

use std::sync::LazyLock;

use ab_glyph::{point, Font, FontRef, GlyphId, PxScale, ScaleFont};
use anyhow::{anyhow, Result};
use tiny_skia::{
    Color, FillRule, FilterQuality, GradientStop, LinearGradient, Paint, Path, PathBuilder, Pixmap,
    PixmapPaint, Point, Shader, SpreadMode, Transform,
};

use crate::model::{Mood, Pet};
use crate::species::Species;
use crate::ui::humanize_age;

pub const WIDTH: u32 = 1000;
pub const HEIGHT: u32 = 380;

/// Name the card is attached under, referenced as `attachment://status.png`.
pub const FILENAME: &str = "status.png";

/// Below this a stat is drawn in the warning colour.
const LOW_STAT: f64 = 25.0;

struct Fonts {
    regular: FontRef<'static>,
    bold: FontRef<'static>,
}

// Bundled so the card looks the same on every host, with no system font
// lookup. Noto Sans is licensed under the SIL Open Font License; see
// assets/fonts/NOTICE.md.
static FONTS: LazyLock<Fonts> = LazyLock::new(|| Fonts {
    regular: FontRef::try_from_slice(include_bytes!("../assets/fonts/NotoSans-Regular.ttf"))
        .expect("the bundled regular font is a valid TrueType file"),
    bold: FontRef::try_from_slice(include_bytes!("../assets/fonts/NotoSans-Bold.ttf"))
        .expect("the bundled bold font is a valid TrueType file"),
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rgb(u8, u8, u8);

impl Rgb {
    fn hex(v: u32) -> Self {
        Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// Mix toward white by `t` (0..1).
    fn lighten(self, t: f32) -> Self {
        let mix = |c: u8| (c as f32 + (255.0 - c as f32) * t).round() as u8;
        Rgb(mix(self.0), mix(self.1), mix(self.2))
    }

    fn alpha(self, a: f32) -> Color {
        Color::from_rgba8(
            self.0,
            self.1,
            self.2,
            (a.clamp(0.0, 1.0) * 255.0).round() as u8,
        )
    }
}

const CARD_BG: Rgb = Rgb(0x17, 0x18, 0x1D);
const TRACK: Rgb = Rgb(0x2A, 0x2D, 0x35);
const TEXT: Rgb = Rgb(0xF2, 0xF3, 0xF5);
const MUTED: Rgb = Rgb(0x9A, 0x9F, 0xA9);
const WARNING: Rgb = Rgb(0xFF, 0x5C, 0x5C);

const HUNGER: Rgb = Rgb(0xF0, 0x9A, 0x3E);
const HAPPINESS: Rgb = Rgb(0xF5, 0xC8, 0x3B);
const HEALTH: Rgb = Rgb(0xE8, 0x4E, 0x72);
const ENERGY: Rgb = Rgb(0x4F, 0x9B, 0xFF);
const XP: Rgb = Rgb(0x9B, 0x7B, 0xF2);

// -- layout -----------------------------------------------------------------

const PAD: f32 = 40.0;
const ART_SIZE: f32 = 300.0;
const COLUMN_X: f32 = PAD + ART_SIZE + 44.0;
const COLUMN_END: f32 = WIDTH as f32 - PAD;
const COLUMN_GAP: f32 = 32.0;

/// Render the card for a living pet.
///
/// `art_png` is the pet's artwork as PNG bytes. Anything that is missing or
/// fails to decode is replaced by a large initial, so a bad file never stops
/// the card from rendering.
pub fn render(pet: &Pet, species: &Species, art_png: Option<&[u8]>, now: i64) -> Result<Vec<u8>> {
    let mut pm =
        Pixmap::new(WIDTH, HEIGHT).ok_or_else(|| anyhow!("could not allocate the card"))?;
    let fonts = &*FONTS;
    let mood = pet.mood();
    let accent = Rgb::hex(mood.colour());

    // Card background, with a wash of the mood colour from the left.
    fill_rounded(
        &mut pm,
        0.0,
        0.0,
        WIDTH as f32,
        HEIGHT as f32,
        32.0,
        &solid(CARD_BG.alpha(1.0)),
    );
    fill_rounded(
        &mut pm,
        0.0,
        0.0,
        WIDTH as f32,
        HEIGHT as f32,
        32.0,
        &horizontal_gradient(0.0, 560.0, accent.alpha(0.22), accent.alpha(0.0)),
    );

    draw_art_panel(&mut pm, fonts, pet, species, accent, art_png, now);
    draw_header(&mut pm, fonts, pet, species, mood, accent);

    let col_w = (COLUMN_END - COLUMN_X - COLUMN_GAP) / 2.0;
    let right_x = COLUMN_X + col_w + COLUMN_GAP;
    draw_stat(
        &mut pm, fonts, COLUMN_X, 188.0, col_w, "HUNGER", pet.hunger, HUNGER,
    );
    draw_stat(
        &mut pm,
        fonts,
        right_x,
        188.0,
        col_w,
        "HAPPINESS",
        pet.happiness,
        HAPPINESS,
    );
    draw_stat(
        &mut pm, fonts, COLUMN_X, 258.0, col_w, "HEALTH", pet.health, HEALTH,
    );
    draw_stat(
        &mut pm, fonts, right_x, 258.0, col_w, "ENERGY", pet.energy, ENERGY,
    );
    draw_xp(&mut pm, fonts, pet);

    pm.encode_png()
        .map_err(|e| anyhow!("encoding the card: {e}"))
}

fn draw_art_panel(
    pm: &mut Pixmap,
    fonts: &Fonts,
    pet: &Pet,
    species: &Species,
    accent: Rgb,
    art_png: Option<&[u8]>,
    now: i64,
) {
    let (x, y) = (PAD, PAD);
    fill_rounded(
        pm,
        x,
        y,
        ART_SIZE,
        ART_SIZE,
        28.0,
        &solid(accent.alpha(0.16)),
    );

    let art = art_png.and_then(|bytes| match Pixmap::decode_png(bytes) {
        Ok(art) => Some(art),
        Err(err) => {
            tracing::warn!(%err, "pet art is not a readable PNG; drawing a placeholder");
            None
        }
    });

    match art {
        Some(art) => draw_image_fit(pm, &art, x + 22.0, y + 2.0, ART_SIZE - 44.0),
        None => {
            let initial: String = species
                .name
                .chars()
                .find(|c| c.is_alphanumeric())
                .unwrap_or('?')
                .to_uppercase()
                .collect();
            let size = 150.0;
            let w = text_width(&fonts.bold, size, &initial, 0.0);
            draw_text(
                pm,
                &fonts.bold,
                size,
                x + (ART_SIZE - w) / 2.0,
                y + 190.0,
                accent.lighten(0.35),
                0.9,
                &initial,
                0.0,
            );
        }
    }

    // Age badge, pinned to the bottom of the art panel.
    let label = "AGE";
    let value = humanize_age(pet.age_secs(now));
    let label_w = text_width(&fonts.bold, 13.0, label, 1.6);
    let value_w = text_width(&fonts.bold, 19.0, &value, 0.0);
    let chip_h = 42.0;
    let chip_w = (label_w + 10.0 + value_w + 36.0).min(ART_SIZE - 24.0);
    let chip_x = x + (ART_SIZE - chip_w) / 2.0;
    let chip_y = y + ART_SIZE - chip_h - 8.0;
    fill_rounded(
        pm,
        chip_x,
        chip_y,
        chip_w,
        chip_h,
        chip_h / 2.0,
        &solid(Rgb(0x0C, 0x0D, 0x10).alpha(0.82)),
    );
    let baseline = chip_y + 28.0;
    draw_text(
        pm,
        &fonts.bold,
        13.0,
        chip_x + 18.0,
        baseline - 1.0,
        MUTED,
        1.0,
        label,
        1.6,
    );
    let value = fit(&fonts.bold, 19.0, &value, chip_w - label_w - 46.0, 0.0);
    draw_text(
        pm,
        &fonts.bold,
        19.0,
        chip_x + 18.0 + label_w + 10.0,
        baseline,
        TEXT,
        1.0,
        &value,
        0.0,
    );
}

fn draw_header(
    pm: &mut Pixmap,
    fonts: &Fonts,
    pet: &Pet,
    species: &Species,
    mood: Mood,
    accent: Rgb,
) {
    // Mood pill, right aligned.
    let pill_text = mood.label();
    let pill_w = text_width(&fonts.bold, 18.0, pill_text, 0.0) + 36.0;
    let pill_h = 36.0;
    let pill_x = COLUMN_END - pill_w;
    let pill_y = 62.0;
    fill_rounded(
        pm,
        pill_x,
        pill_y,
        pill_w,
        pill_h,
        pill_h / 2.0,
        &solid(accent.alpha(0.28)),
    );
    draw_text(
        pm,
        &fonts.bold,
        18.0,
        pill_x + 18.0,
        pill_y + 25.0,
        accent.lighten(0.55),
        1.0,
        pill_text,
        0.0,
    );

    let name_max = pill_x - COLUMN_X - 20.0;
    let name = fit(&fonts.bold, 46.0, &pet.name, name_max, 0.0);
    draw_text(pm, &fonts.bold, 46.0, COLUMN_X, 96.0, TEXT, 1.0, &name, 0.0);

    let subtitle = format!("{}   \u{00B7}   {}", species.name, pet.stage());
    let subtitle = fit(&fonts.regular, 22.0, &subtitle, COLUMN_END - COLUMN_X, 0.0);
    draw_text(
        pm,
        &fonts.regular,
        22.0,
        COLUMN_X,
        132.0,
        MUTED,
        1.0,
        &subtitle,
        0.0,
    );
}

/// One labelled stat: `LABEL ... value` above a rounded bar.
#[allow(clippy::too_many_arguments)]
fn draw_stat(
    pm: &mut Pixmap,
    fonts: &Fonts,
    x: f32,
    baseline: f32,
    w: f32,
    label: &str,
    value: f64,
    colour: Rgb,
) {
    let value = if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    };
    let low = value < LOW_STAT;
    let fill = if low { WARNING } else { colour };

    draw_text(
        pm,
        &fonts.bold,
        15.0,
        x,
        baseline,
        if low { WARNING } else { MUTED },
        1.0,
        label,
        1.8,
    );
    let number = format!("{value:.0}");
    let number_w = text_width(&fonts.bold, 22.0, &number, 0.0);
    draw_text(
        pm,
        &fonts.bold,
        22.0,
        x + w - number_w,
        baseline + 1.0,
        if low { WARNING } else { TEXT },
        1.0,
        &number,
        0.0,
    );

    draw_bar(pm, x, baseline + 14.0, w, 18.0, value / 100.0, fill);
}

fn draw_xp(pm: &mut Pixmap, fonts: &Fonts, pet: &Pet) {
    let baseline = 324.0;
    let (into, needed) = pet.level_progress();
    let label = format!("LEVEL {}", pet.level());
    draw_text(
        pm,
        &fonts.bold,
        15.0,
        COLUMN_X,
        baseline,
        XP.lighten(0.35),
        1.0,
        &label,
        1.8,
    );

    let detail = format!("{:.0} / {:.0} XP", into.max(0.0), needed);
    let detail_w = text_width(&fonts.regular, 16.0, &detail, 0.0);
    draw_text(
        pm,
        &fonts.regular,
        16.0,
        COLUMN_END - detail_w,
        baseline,
        MUTED,
        1.0,
        &detail,
        0.0,
    );

    let ratio = if needed > 0.0 { into / needed } else { 0.0 };
    draw_bar(
        pm,
        COLUMN_X,
        baseline + 8.0,
        COLUMN_END - COLUMN_X,
        10.0,
        ratio,
        XP,
    );
}

/// A rounded track with a gradient fill covering `ratio` of it.
fn draw_bar(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, ratio: f64, colour: Rgb) {
    fill_rounded(pm, x, y, w, h, h / 2.0, &solid(TRACK.alpha(1.0)));

    let ratio = if ratio.is_finite() {
        ratio.clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    if ratio <= 0.0 {
        return;
    }
    // Never narrower than the bar is tall, so a nearly empty stat still reads
    // as a small pill rather than a sliver.
    let fill_w = (w * ratio).max(h);
    fill_rounded(
        pm,
        x,
        y,
        fill_w,
        h,
        h / 2.0,
        &horizontal_gradient(
            x,
            x + fill_w,
            colour.lighten(0.3).alpha(1.0),
            colour.alpha(1.0),
        ),
    );
}

// -- drawing primitives -----------------------------------------------------

fn solid(colour: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(colour);
    paint.anti_alias = true;
    paint
}

fn horizontal_gradient(x0: f32, x1: f32, from: Color, to: Color) -> Paint<'static> {
    let shader = LinearGradient::new(
        Point::from_xy(x0, 0.0),
        Point::from_xy(x1, 0.0),
        vec![GradientStop::new(0.0, from), GradientStop::new(1.0, to)],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap_or(Shader::SolidColor(to));
    Paint {
        shader,
        anti_alias: true,
        ..Paint::default()
    }
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    // Control-point distance that makes a cubic approximate a quarter circle.
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn fill_rounded(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, paint: &Paint) {
    if let Some(path) = rounded_rect(x, y, w, h, r) {
        pm.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
    }
}

/// Draw `image` centred in a `size` square, keeping its aspect.
///
/// Art that fits is enlarged by the largest whole-number factor with
/// nearest-neighbour sampling, so pixel art stays crisp. Art bigger than the
/// square is scaled down smoothly instead.
fn draw_image_fit(pm: &mut Pixmap, image: &Pixmap, x: f32, y: f32, size: f32) {
    let longest = image.width().max(image.height()) as f32;
    if longest <= 0.0 {
        return;
    }
    let (scale, quality) = if longest <= size {
        ((size / longest).floor().max(1.0), FilterQuality::Nearest)
    } else {
        (size / longest, FilterQuality::Bicubic)
    };
    // Whole-pixel offsets, or nearest-neighbour sampling smears pixel edges.
    let tx = (x + (size - image.width() as f32 * scale) / 2.0).round();
    let ty = (y + (size - image.height() as f32 * scale) / 2.0).round();
    let paint = PixmapPaint {
        quality,
        ..PixmapPaint::default()
    };
    pm.draw_pixmap(
        0,
        0,
        image.as_ref(),
        &paint,
        Transform::from_row(scale, 0.0, 0.0, scale, tx, ty),
        None,
    );
}

/// Enlarge small PNG pixel art by a whole-number `factor`, nearest-neighbour.
///
/// Discord shows an embed image at its native size, so a 128px sprite sent
/// as-is is postage-stamp small. Images whose longest side already exceeds
/// `max_side` are returned unchanged.
pub fn upscale_png(png: &[u8], factor: u32, max_side: u32) -> Result<Vec<u8>> {
    let src = Pixmap::decode_png(png).map_err(|e| anyhow!("decoding art: {e}"))?;
    let factor = factor.max(1);
    if factor == 1 || src.width().max(src.height()) > max_side {
        return Ok(png.to_vec());
    }
    let (w, h) = src
        .width()
        .checked_mul(factor)
        .zip(src.height().checked_mul(factor))
        .ok_or_else(|| anyhow!("art is too large to upscale"))?;
    let mut dst = Pixmap::new(w, h).ok_or_else(|| anyhow!("could not allocate upscaled art"))?;
    let paint = PixmapPaint {
        quality: FilterQuality::Nearest,
        ..PixmapPaint::default()
    };
    dst.draw_pixmap(
        0,
        0,
        src.as_ref(),
        &paint,
        Transform::from_scale(factor as f32, factor as f32),
        None,
    );
    dst.encode_png().map_err(|e| anyhow!("encoding art: {e}"))
}

/// Width of `text` in pixels, with `tracking` extra pixels between glyphs.
fn text_width(font: &FontRef, size: f32, text: &str, tracking: f32) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut width = 0.0;
    let mut prev: Option<GlyphId> = None;
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if id.0 == 0 {
            continue;
        }
        if let Some(p) = prev {
            width += scaled.kern(p, id) + tracking;
        }
        width += scaled.h_advance(id);
        prev = Some(id);
    }
    width
}

/// Draw `text` with its baseline at `baseline`.
///
/// Characters the font has no glyph for, such as emoji, are skipped rather
/// than drawn as empty boxes.
#[allow(clippy::too_many_arguments)]
fn draw_text(
    pm: &mut Pixmap,
    font: &FontRef,
    size: f32,
    x: f32,
    baseline: f32,
    colour: Rgb,
    opacity: f32,
    text: &str,
    tracking: f32,
) {
    let scale = PxScale::from(size);
    let scaled = font.as_scaled(scale);
    let mut caret = x;
    let mut prev: Option<GlyphId> = None;

    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if id.0 == 0 {
            continue;
        }
        if let Some(p) = prev {
            caret += scaled.kern(p, id) + tracking;
        }
        let glyph = id.with_scale_and_position(scale, point(caret, baseline));
        caret += scaled.h_advance(id);
        prev = Some(id);

        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                blend(
                    pm,
                    bounds.min.x as i32 + gx as i32,
                    bounds.min.y as i32 + gy as i32,
                    colour,
                    coverage * opacity,
                );
            });
        }
    }
}

/// Composite one pixel of `colour` at `alpha` over the premultiplied canvas.
fn blend(pm: &mut Pixmap, x: i32, y: i32, colour: Rgb, alpha: f32) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    let a = alpha.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let i = ((y * w + x) * 4) as usize;
    let data = pm.data_mut();
    let inv = 1.0 - a;
    let over = |src: u8, dst: u8| (src as f32 * a + dst as f32 * inv).round() as u8;
    data[i] = over(colour.0, data[i]);
    data[i + 1] = over(colour.1, data[i + 1]);
    data[i + 2] = over(colour.2, data[i + 2]);
    data[i + 3] = over(255, data[i + 3]);
}

/// Shorten `text` with an ellipsis until it fits in `max` pixels.
fn fit(font: &FontRef, size: f32, text: &str, max: f32, tracking: f32) -> String {
    if text_width(font, size, text, tracking) <= max {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate = format!("{}\u{2026}", chars.iter().collect::<String>().trim_end());
        if text_width(font, size, &candidate, tracking) <= max {
            return candidate;
        }
    }
    "\u{2026}".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn decode(png: &[u8]) -> Pixmap {
        Pixmap::decode_png(png).expect("the card is a valid PNG")
    }

    #[test]
    fn renders_a_png_of_the_expected_size() {
        let pet = Pet::new(1, "Mochi".into(), "cat".into(), 1, 0);
        let art = std::fs::read("assets/pets/cat/happy.png").ok();
        let card = decode(&render(&pet, &species(), art.as_deref(), 3 * 3600).unwrap());
        assert_eq!((card.width(), card.height()), (WIDTH, HEIGHT));
    }

    #[test]
    fn missing_or_corrupt_art_still_renders() {
        let pet = Pet::new(1, "Mochi".into(), "cat".into(), 1, 0);
        assert!(render(&pet, &species(), None, 0).is_ok());
        assert!(render(&pet, &species(), Some(b"not a png"), 0).is_ok());
    }

    #[test]
    fn extreme_and_invalid_stats_do_not_panic() {
        let mut pet = Pet::new(1, "\u{1F525}\u{1F525}".into(), "cat".into(), 1, 0);
        pet.hunger = f64::NAN;
        pet.happiness = -40.0;
        pet.health = 250.0;
        pet.energy = 0.0;
        pet.xp = 1e9;
        assert!(render(&pet, &species(), None, i64::MAX / 2).is_ok());
    }

    #[test]
    fn a_full_bar_is_filled_and_an_empty_bar_is_not() {
        let mut pm = Pixmap::new(200, 40).unwrap();
        draw_bar(&mut pm, 0.0, 0.0, 200.0, 20.0, 1.0, ENERGY);
        draw_bar(&mut pm, 0.0, 20.0, 200.0, 20.0, 0.0, ENERGY);
        let px = |x: u32, y: u32| pm.pixel(x, y).unwrap();
        // Middle of the full bar is the fill colour, not the track.
        assert_ne!(px(150, 10).red(), TRACK.0);
        // Middle of the empty bar is the track.
        assert_eq!((px(150, 30).red(), px(150, 30).green()), (TRACK.0, TRACK.1));
    }

    #[test]
    fn small_art_is_enlarged_crisply_by_a_whole_number() {
        // A 2x2 checkerboard drawn into an 8px square must become four solid
        // 4x4 blocks: no blending at the seams.
        let mut art = Pixmap::new(2, 2).unwrap();
        let colours = [TEXT, WARNING, ENERGY, XP];
        for (i, c) in colours.iter().enumerate() {
            let px = tiny_skia::PremultipliedColorU8::from_rgba(c.0, c.1, c.2, 255).unwrap();
            art.pixels_mut()[i] = px;
        }
        let mut pm = Pixmap::new(8, 8).unwrap();
        draw_image_fit(&mut pm, &art, 0.0, 0.0, 8.0);
        for y in 0..8 {
            for x in 0..8 {
                let expected = colours[(y / 4 * 2 + x / 4) as usize];
                let got = pm.pixel(x, y).unwrap();
                assert_eq!(
                    (got.red(), got.green(), got.blue(), got.alpha()),
                    (expected.0, expected.1, expected.2, 255),
                    "pixel ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn upscaling_repeats_each_pixel_and_leaves_big_art_alone() {
        let sprite = std::fs::read("assets/pets/cat/happy.png").unwrap();
        let small = Pixmap::decode_png(&sprite).unwrap();
        let big = Pixmap::decode_png(&upscale_png(&sprite, 3, 256).unwrap()).unwrap();
        assert_eq!(
            (big.width(), big.height()),
            (small.width() * 3, small.height() * 3)
        );
        for (x, y) in [(0, 0), (40, 60), (64, 64), (127, 127)] {
            let (a, b) = (
                small.pixel(x, y).unwrap(),
                big.pixel(x * 3 + 1, y * 3 + 2).unwrap(),
            );
            assert_eq!(a, b, "sprite pixel ({x}, {y})");
        }
        assert_eq!(
            upscale_png(&sprite, 3, 64).unwrap(),
            sprite,
            "over the size limit"
        );
    }

    #[test]
    fn long_names_are_truncated_to_fit() {
        let font = &FONTS.bold;
        let long = "An Extraordinarily Long Pet Name Indeed";
        let fitted = fit(font, 46.0, long, 300.0, 0.0);
        assert!(fitted.ends_with('\u{2026}'));
        assert!(text_width(font, 46.0, &fitted, 0.0) <= 300.0);
        assert_eq!(fit(font, 46.0, "Mochi", 300.0, 0.0), "Mochi");
    }

    #[test]
    fn glyphs_the_font_lacks_take_no_space() {
        let font = &FONTS.regular;
        assert_eq!(text_width(font, 20.0, "\u{1F43E}", 0.0), 0.0);
    }

    /// Art file, name, hunger, happiness, health, energy, xp, age, asleep.
    type PreviewCase = (
        &'static str,
        &'static str,
        f64,
        f64,
        f64,
        f64,
        f64,
        i64,
        bool,
    );

    /// Writes sample cards for eyeballing. Run with
    /// `CARD_PREVIEW_DIR=some/dir cargo test card_previews`.
    #[test]
    fn card_previews() {
        let Ok(dir) = std::env::var("CARD_PREVIEW_DIR") else {
            return;
        };
        std::fs::create_dir_all(&dir).unwrap();

        let cases: [PreviewCase; 4] = [
            (
                "happy",
                "Mochi",
                92.0,
                88.0,
                100.0,
                76.0,
                240.0,
                3 * 86_400 + 5 * 3600,
                false,
            ),
            (
                "hungry",
                "Sir Wobbles the Third",
                12.0,
                55.0,
                70.0,
                60.0,
                60.0,
                7 * 3600 + 12 * 60,
                false,
            ),
            (
                "sick",
                "Pip",
                3.0,
                8.0,
                18.0,
                30.0,
                1200.0,
                19 * 86_400,
                false,
            ),
            (
                "sleeping",
                "Nap Queen",
                60.0,
                70.0,
                90.0,
                14.0,
                20.0,
                40,
                true,
            ),
        ];
        for (mood_file, name, hunger, happiness, health, energy, xp, age, asleep) in cases {
            let mut pet = Pet::new(1, name.into(), "cat".into(), 1, 0);
            (
                pet.hunger,
                pet.happiness,
                pet.health,
                pet.energy,
                pet.xp,
                pet.asleep,
            ) = (hunger, happiness, health, energy, xp, asleep);
            let art = std::fs::read(format!("assets/pets/cat/{mood_file}.png")).ok();
            let png = render(&pet, &species(), art.as_deref(), age).unwrap();
            std::fs::write(format!("{dir}/card_{mood_file}.png"), png).unwrap();
        }
    }
}
