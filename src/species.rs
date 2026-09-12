//! The species registry, loaded from `assets/species.toml` at startup.
//!
//! Species are data, not code, so adding a new pet is a matter of dropping art
//! into `assets/pets/<key>/` and adding a table to the manifest. The only cost
//! is a restart, because Discord slash-command choices are fixed at
//! registration time.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// One adoptable species.
#[derive(Debug, Clone, Deserialize)]
pub struct Species {
    /// Directory-safe identifier. Must match the art folder name.
    pub key: String,
    /// Display name shown in the adopt menu.
    pub name: String,
    /// Shown next to the name when no image is available.
    #[serde(default)]
    pub emoji: String,
    #[serde(default)]
    pub description: String,
    /// Flavour only: mentioned when this food is used with the feed command.
    #[serde(default)]
    pub favourite_food: Option<String>,
    /// Flavour only: mentioned when this game is used with the play command.
    #[serde(default)]
    pub favourite_game: Option<String>,
}

impl Species {
    /// `"Blob"` or `"🫧 Blob"` depending on whether an emoji is configured.
    pub fn display(&self) -> String {
        if self.emoji.is_empty() {
            self.name.clone()
        } else {
            format!("{} {}", self.emoji, self.name)
        }
    }
}

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    species: Vec<Species>,
}

/// All known species, in manifest order.
#[derive(Debug, Clone)]
pub struct Registry {
    ordered: Vec<Species>,
    by_key: HashMap<String, usize>,
}

impl Registry {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading species manifest at {}", path.display()))?;
        let manifest: Manifest = toml::from_str(&raw)
            .with_context(|| format!("parsing species manifest at {}", path.display()))?;
        Self::from_species(manifest.species)
    }

    pub fn from_species(ordered: Vec<Species>) -> Result<Self> {
        if ordered.is_empty() {
            bail!("the species manifest defines no species; at least one is required");
        }

        // Discord caps a slash-command option at 25 choices. Failing loudly at
        // startup beats a confusing rejection from the API on first connect.
        if ordered.len() > 25 {
            bail!(
                "the species manifest defines {} species; Discord allows at most 25 choices",
                ordered.len()
            );
        }

        let mut by_key = HashMap::with_capacity(ordered.len());
        for (idx, sp) in ordered.iter().enumerate() {
            if sp.key.is_empty() {
                bail!("species {:?} has an empty key", sp.name);
            }
            if !sp
                .key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                bail!(
                    "species key {:?} must contain only letters, digits, underscore or hyphen",
                    sp.key
                );
            }
            if by_key.insert(sp.key.clone(), idx).is_some() {
                bail!("duplicate species key {:?} in the manifest", sp.key);
            }
        }

        Ok(Self { ordered, by_key })
    }

    pub fn get(&self, key: &str) -> Option<&Species> {
        self.by_key.get(key).map(|&i| &self.ordered[i])
    }

    pub fn all(&self) -> &[Species] {
        &self.ordered
    }

    pub fn len(&self) -> usize {
        self.ordered.len()
    }

    /// Falls back to the first species so a pet whose species was removed from
    /// the manifest still renders instead of crashing the status command.
    pub fn get_or_first(&self, key: &str) -> &Species {
        self.get(key).unwrap_or(&self.ordered[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(key: &str) -> Species {
        Species {
            key: key.into(),
            name: key.into(),
            emoji: String::new(),
            description: String::new(),
            favourite_food: None,
            favourite_game: None,
        }
    }

    #[test]
    fn rejects_an_empty_manifest() {
        assert!(Registry::from_species(vec![]).is_err());
    }

    #[test]
    fn rejects_duplicate_keys() {
        assert!(Registry::from_species(vec![sp("blob"), sp("blob")]).is_err());
    }

    #[test]
    fn rejects_path_traversal_in_keys() {
        assert!(Registry::from_species(vec![sp("../etc")]).is_err());
        assert!(Registry::from_species(vec![sp("a/b")]).is_err());
    }

    #[test]
    fn rejects_more_than_discord_allows() {
        let many: Vec<_> = (0..26).map(|i| sp(&format!("s{i}"))).collect();
        assert!(Registry::from_species(many).is_err());
    }

    #[test]
    fn unknown_species_falls_back_to_the_first() {
        let reg = Registry::from_species(vec![sp("blob"), sp("slime")]).unwrap();
        assert_eq!(reg.get_or_first("retired").key, "blob");
        assert_eq!(reg.get_or_first("slime").key, "slime");
    }

    #[test]
    fn parses_a_manifest() {
        let toml = r#"
            [[species]]
            key = "blob"
            name = "Blob"
            emoji = "B"
            favourite_food = "pizza"
        "#;
        let m: Manifest = toml::from_str(toml).unwrap();
        let reg = Registry::from_species(m.species).unwrap();
        assert_eq!(reg.len(), 1);
        assert_eq!(
            reg.get("blob").unwrap().favourite_food.as_deref(),
            Some("pizza")
        );
    }
}
