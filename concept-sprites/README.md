# Server Pet companion sprites

32 original static sprites created with Aseprite 1.3.18.5 using its native Lua image and sprite APIs. This is the revised 9–9.5/10 self-rated collection. Each character has eight states: content, dead, happy, hungry, sad, sick, sleeping, tired.

## Files

Each of `cat/`, `dog/`, `mouse/`, and `dragon/` contains eight transparent PNGs and eight matching editable `.aseprite` files. Filenames are the state names, for example `cat/happy.png` and `cat/happy.aseprite`.

- Canvas: **128 × 128 pixels**, RGBA, sRGB.
- Pixel style: broad 2 × 2 pixel clusters, carefully placed native 1px accents, and no partial alpha.
- Editable sources: five named layers separating body, head, expression, paws, and state details.
- `contact-sheet.png`: all 32 sprites shown at their native 128px size.
- `*-review.png`: enlarged character sheets using nearest-neighbor scaling.
- `ratings.md`: final individual self-ratings and revision notes.
- `validation.json`: per-sprite dimensions, alpha values, bounds, colors, and pixel hashes.

The PNGs contain no background, labels, watermark, or ground shadow. Labels and colored backdrops appear only on the review sheets. Use nearest-neighbor interpolation when enlarging the sprites.

## Character direction

**Cat:** orange tabby, cream muzzle, teal scarf, striped curled tail.

**Dog:** golden coat, floppy brown ears, cream blaze, red bandana.

**Mouse:** lavender coat, large pink ears, pink feet and tail, purple scarf.

**Dragon:** jade scales, golden horns and belly, purple wings and crest.

Sleeping uses a curled pose; dead uses a separate belly-up pose with X eyes, curled paws, and a species-specific departing spirit. Hungry uses an empty bowl and a species-specific food thought. Sick is bundled in a shaded blanket with an ice pack and thermometer. Sad gathers its paws and lowers its tail. Tired uses drooping eyes, a lowered tail, and a paw covering a yawn. Happy lifts its paws, shifts its feet, and raises its ears or wings.

Technical checks: all 32 PNGs are distinct, exactly 128 × 128, have clean transparency and unclipped bounds. Every editable source was reopened in Aseprite and its rendered pixels compared with its PNG export.
