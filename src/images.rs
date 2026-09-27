//! Art lookup for pets.
//!
//! # How art is organised
//!
//! ```text
//! assets/pets/
//!   _default/            <- used when a species has no art of its own
//!     content.png
//!     hungry.png
//!   cat/
//!     default.png        <- covers every mood not listed below
//!     happy.png
//!     sleeping.png
//!     adult/             <- optional per-stage overrides
//!       happy.png
//! ```
//!
//! Resolution walks from most specific to least specific and takes the first
//! file that exists:
//!
//! 1. `<species>/<stage>/<mood>`
//! 2. `<species>/<stage>/default`
//! 3. `<species>/<mood>`
//! 4. `<species>/default`
//! 5. `_default/<mood>`
//! 6. `_default/default`
//!
//! So a one-file art pack works, and a fully illustrated species with per-stage
//! sprites works, with nothing in between needing configuration.
//!
//! The directory is indexed once at startup rather than stat-ed on every
//! command. Adding art therefore needs a restart, which matches the species
//! manifest.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::model::{Mood, Stage};

/// Directory that supplies art for species with none of their own.
pub const FALLBACK_SPECIES: &str = "_default";

/// Filename stem that covers any mood without a dedicated file.
const DEFAULT_STEM: &str = "default";

/// Extensions Discord will render inline in an embed, in preference order.
const EXTENSIONS: [&str; 5] = ["png", "gif", "webp", "jpg", "jpeg"];

/// What to show for a pet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PetImage {
    /// A local file to upload alongside the message.
    File {
        path: PathBuf,
        /// Flat filename to send it under, referenced as `attachment://<name>`.
        filename: String,
    },
    /// A remote URL set per-pet, used verbatim.
    Remote(String),
    /// No art available; the caller falls back to an emoji.
    None,
}

impl PetImage {
    /// The value to hand to an embed's image field, if any.
    pub fn embed_url(&self) -> Option<String> {
        match self {
            PetImage::File { filename, .. } => Some(format!("attachment://{filename}")),
            PetImage::Remote(url) => Some(url.clone()),
            PetImage::None => None,
        }
    }
}

/// An index of every art file found under the assets directory.
#[derive(Debug, Default, Clone)]
pub struct ImageIndex {
    /// Keyed by logical path, e.g. `blob/adult/happy` or `blob/default`.
    entries: HashMap<String, PathBuf>,
    root: PathBuf,
}

impl ImageIndex {
    /// Scan `root` (typically `assets/pets`) for art.
    ///
    /// A missing directory is not an error - the bot runs fine on emoji alone,
    /// and this keeps first-run friction low.
    pub fn scan(root: &Path) -> Result<Self> {
        let mut index = ImageIndex {
            entries: HashMap::new(),
            root: root.to_path_buf(),
        };

        if !root.is_dir() {
            tracing::warn!(
                path = %root.display(),
                "no pet art directory found; pets will be shown with emoji only"
            );
            return Ok(index);
        }

        // Depth 2 is all the layout uses: <species>/<file> and
        // <species>/<stage>/<file>.
        for species_entry in read_dir_sorted(root)? {
            if !species_entry.is_dir() {
                continue;
            }
            let Some(species) = file_name_of(&species_entry) else {
                continue;
            };

            for child in read_dir_sorted(&species_entry)? {
                if child.is_dir() {
                    let Some(stage) = file_name_of(&child) else {
                        continue;
                    };
                    for file in read_dir_sorted(&child)? {
                        if let Some(stem) = art_stem(&file) {
                            index
                                .entries
                                .entry(format!("{species}/{stage}/{stem}"))
                                .or_insert(file);
                        }
                    }
                } else if let Some(stem) = art_stem(&child) {
                    index
                        .entries
                        .entry(format!("{species}/{stem}"))
                        .or_insert(child);
                }
            }
        }

        tracing::info!(
            files = index.entries.len(),
            path = %root.display(),
            "indexed pet art"
        );
        Ok(index)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve art for a pet, honouring a per-pet remote override.
    pub fn resolve(
        &self,
        species: &str,
        stage: Stage,
        mood: Mood,
        custom_url: Option<&str>,
    ) -> PetImage {
        if let Some(url) = custom_url {
            let url = url.trim();
            if !url.is_empty() {
                return PetImage::Remote(url.to_string());
            }
        }

        for key in candidate_keys(species, stage, mood) {
            if let Some(path) = self.entries.get(&key) {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("png")
                    .to_ascii_lowercase();
                return PetImage::File {
                    path: path.clone(),
                    // A stable, flat filename. Discord rejects paths here, and
                    // the mood is in the name so clients do not reuse a cached
                    // image across mood changes.
                    filename: format!("pet_{}_{}.{}", sanitize(species), mood.key(), ext),
                };
            }
        }

        PetImage::None
    }

    /// Species keys that have at least one art file. Used by the startup
    /// coverage report.
    pub fn species_with_art(&self) -> Vec<String> {
        let mut seen: Vec<String> = self
            .entries
            .keys()
            .filter_map(|k| k.split('/').next().map(str::to_string))
            .collect();
        seen.sort();
        seen.dedup();
        seen
    }

    /// Moods that would fall through to the shared fallback art for a species.
    pub fn missing_moods(&self, species: &str, stage: Stage) -> Vec<Mood> {
        Mood::ALL
            .iter()
            .copied()
            .filter(|&mood| {
                candidate_keys(species, stage, mood)
                    .into_iter()
                    // Only the species-specific candidates count as coverage.
                    .take(4)
                    .all(|k| !self.entries.contains_key(&k))
            })
            .collect()
    }
}

/// The lookup chain, most specific first.
fn candidate_keys(species: &str, stage: Stage, mood: Mood) -> Vec<String> {
    let species = sanitize(species);
    let (stage, mood) = (stage.key(), mood.key());
    vec![
        format!("{species}/{stage}/{mood}"),
        format!("{species}/{stage}/{DEFAULT_STEM}"),
        format!("{species}/{mood}"),
        format!("{species}/{DEFAULT_STEM}"),
        format!("{FALLBACK_SPECIES}/{mood}"),
        format!("{FALLBACK_SPECIES}/{DEFAULT_STEM}"),
    ]
}

/// Strip anything that could escape the assets directory or confuse Discord.
///
/// Species keys are already validated by the registry, but pets store a plain
/// string that may predate a manifest change, so this stays defensive.
fn sanitize(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if cleaned.is_empty() {
        FALLBACK_SPECIES.to_string()
    } else {
        cleaned
    }
}

fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let iter = std::fs::read_dir(dir)
        .with_context(|| format!("reading art directory {}", dir.display()))?;
    for entry in iter {
        let entry = entry.with_context(|| format!("reading an entry in {}", dir.display()))?;
        out.push(entry.path());
    }
    // Deterministic order so a duplicate stem across extensions always
    // resolves the same way between runs.
    out.sort();
    Ok(out)
}

fn file_name_of(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_ascii_lowercase())
}

/// Lowercased stem of a supported image file, or `None` for anything else.
fn art_stem(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    Some(path.file_stem()?.to_str()?.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_with(keys: &[&str]) -> ImageIndex {
        ImageIndex {
            entries: keys
                .iter()
                .map(|k| ((*k).to_string(), PathBuf::from(format!("{k}.png"))))
                .collect(),
            root: PathBuf::from("assets/pets"),
        }
    }

    #[test]
    fn prefers_the_most_specific_art() {
        let idx = index_with(&["blob/adult/happy", "blob/happy", "blob/default"]);
        let img = idx.resolve("blob", Stage::Adult, Mood::Happy, None);
        assert_eq!(
            img,
            PetImage::File {
                path: PathBuf::from("blob/adult/happy.png"),
                filename: "pet_blob_happy.png".into(),
            }
        );
    }

    #[test]
    fn falls_back_through_the_chain() {
        let idx = index_with(&["blob/default"]);
        let img = idx.resolve("blob", Stage::Adult, Mood::Sick, None);
        let PetImage::File { path, .. } = img else {
            panic!("expected a file");
        };
        assert_eq!(path, PathBuf::from("blob/default.png"));
    }

    #[test]
    fn falls_back_to_the_shared_species() {
        let idx = index_with(&["_default/default"]);
        let img = idx.resolve("unknown", Stage::Baby, Mood::Sad, None);
        let PetImage::File { path, .. } = img else {
            panic!("expected a file");
        };
        assert_eq!(path, PathBuf::from("_default/default.png"));
    }

    #[test]
    fn reports_no_image_when_nothing_matches() {
        let idx = index_with(&[]);
        assert_eq!(
            idx.resolve("blob", Stage::Baby, Mood::Happy, None),
            PetImage::None
        );
    }

    #[test]
    fn a_custom_url_wins_over_local_art() {
        let idx = index_with(&["blob/happy"]);
        let img = idx.resolve("blob", Stage::Baby, Mood::Happy, Some("https://x/y.png"));
        assert_eq!(img, PetImage::Remote("https://x/y.png".into()));
    }

    #[test]
    fn a_blank_custom_url_is_ignored() {
        let idx = index_with(&["blob/happy"]);
        let img = idx.resolve("blob", Stage::Baby, Mood::Happy, Some("   "));
        assert!(matches!(img, PetImage::File { .. }));
    }

    #[test]
    fn species_names_cannot_escape_the_assets_directory() {
        assert_eq!(sanitize("../../etc/passwd"), "etcpasswd");
        assert_eq!(sanitize("blob/../.."), "blob");
        assert_eq!(sanitize(""), FALLBACK_SPECIES);
        assert_eq!(sanitize("!!!"), FALLBACK_SPECIES);
    }

    #[test]
    fn attachment_urls_are_well_formed() {
        let idx = index_with(&["blob/happy"]);
        let img = idx.resolve("blob", Stage::Baby, Mood::Happy, None);
        assert_eq!(img.embed_url().unwrap(), "attachment://pet_blob_happy.png");
    }

    #[test]
    fn the_filename_changes_with_mood_so_clients_do_not_cache_stale_art() {
        let idx = index_with(&["blob/default"]);
        let happy = idx.resolve("blob", Stage::Baby, Mood::Happy, None);
        let sad = idx.resolve("blob", Stage::Baby, Mood::Sad, None);
        assert_ne!(happy.embed_url(), sad.embed_url());
    }

    /// Scans the art that actually ships with the repo, so a rename or a
    /// deleted file is caught here rather than as a blank status card.
    #[test]
    fn the_shipped_art_covers_every_species_mood_and_stage() {
        let root = Path::new("assets/pets");
        let index = ImageIndex::scan(root).expect("scanning the art directory");
        if index.is_empty() {
            // Art has not been generated in this checkout; nothing to verify.
            // Copy the sprites from `concept-sprites/` into `assets/pets/`.
            return;
        }

        let registry = crate::species::Registry::load(Path::new("assets/species.toml"))
            .expect("loading the species manifest");

        let stages = [
            Stage::Baby,
            Stage::Child,
            Stage::Teen,
            Stage::Adult,
            Stage::Elder,
        ];

        for sp in registry.all() {
            for mood in Mood::ALL {
                for stage in stages {
                    let resolved = index.resolve(&sp.key, stage, mood, None);
                    let PetImage::File { path, .. } = &resolved else {
                        panic!(
                            "no art resolves for species {:?}, stage {:?}, mood {:?}",
                            sp.key,
                            stage.key(),
                            mood.key()
                        );
                    };
                    assert!(
                        path.is_file(),
                        "indexed art is missing on disk: {}",
                        path.display()
                    );
                }
            }
        }
    }

    #[test]
    fn missing_moods_ignores_shared_fallback_art() {
        let idx = index_with(&["blob/happy", "_default/default"]);
        let missing = idx.missing_moods("blob", Stage::Baby);
        assert!(!missing.contains(&Mood::Happy));
        assert!(missing.contains(&Mood::Sick));
    }
}
