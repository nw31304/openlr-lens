use std::path::Path;
use anyhow::{Context, Result};
use serde::Deserialize;

/// A single class/subclass → FRC/FOW mapping rule.
/// Rules are matched in order; first match wins.
/// `class = ""` is a catch-all; `subclass = ""` matches any (including absent) subclass.
/// `vehicular = false` marks segments that should be excluded from the routing graph.
/// Defaults to `true`; only non-vehicular classes need to set it explicitly.
#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub class: String,
    pub subclass: String,
    pub frc: u8,
    pub fow: u8,
    #[serde(default = "default_vehicular")]
    pub vehicular: bool,
}

fn default_vehicular() -> bool { true }

/// Per-flag FOW override applied after the class/subclass match.
#[derive(Debug, Clone, Deserialize)]
pub struct FlagOverride {
    pub fow: Option<u8>,
    pub frc: Option<u8>,
}

/// The complete schema mapping loaded from a TOML file.
#[derive(Debug, Clone, Deserialize)]
pub struct SchemaMapping {
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub flag_overrides: std::collections::HashMap<String, FlagOverride>,
}

impl SchemaMapping {
    /// Look up FRC/FOW for a given class and optional subclass.
    /// Returns `(frc, fow)` for the first matching rule, or `(7, 0)` if none match.
    pub fn lookup(&self, class: &str, subclass: Option<&str>) -> (u8, u8) {
        let sub = subclass.unwrap_or("");
        for rule in &self.rules {
            let class_match = rule.class.is_empty() || rule.class == class;
            let sub_match   = rule.subclass.is_empty() || rule.subclass == sub;
            if class_match && sub_match {
                return (rule.frc, rule.fow);
            }
        }
        (7, 0)
    }

    /// Returns `false` if the first matching rule marks this class/subclass as non-vehicular.
    /// Defaults to `true` when no rule matches.
    pub fn is_vehicular(&self, class: &str, subclass: Option<&str>) -> bool {
        let sub = subclass.unwrap_or("");
        for rule in &self.rules {
            let class_match = rule.class.is_empty() || rule.class == class;
            let sub_match   = rule.subclass.is_empty() || rule.subclass == sub;
            if class_match && sub_match {
                return rule.vehicular;
            }
        }
        true
    }

    /// Apply road-flag overrides to an (frc, fow) pair.
    pub fn apply_flags(&self, mut frc: u8, mut fow: u8, flags: &[&str]) -> (u8, u8) {
        for flag in flags {
            if let Some(ov) = self.flag_overrides.get(*flag) {
                if let Some(f) = ov.fow { fow = f; }
                if let Some(f) = ov.frc { frc = f; }
            }
        }
        (frc, fow)
    }
}

pub fn load(path: &Path) -> Result<SchemaMapping> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read schema file '{}'", path.display()))?;
    toml::from_str(&text)
        .with_context(|| format!("failed to parse schema file '{}'", path.display()))
}
