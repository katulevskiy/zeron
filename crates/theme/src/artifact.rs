//! Browser export: upstream families and accent derivations are serialized
//! directly. Browser-only tokens are authored separately, never added to the
//! upstream domain model. See web/packages/theme/README.md for source policy.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AccentPreset, AccentRoles, AccentSelection, Appearance, Color, ThemeFamily, ThemeRegistry,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserTokens {
    pub provenance: String,
    pub layout: Value,
    pub motion: Value,
    pub fonts: Value,
    pub colors: BTreeMap<String, BrowserColorRoles>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserColorRoles {
    pub raised_hover: Color,
    pub text_dim: Color,
    pub danger_strong: Color,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeArtifact {
    pub schema_version: u32,
    pub generator: &'static str,
    pub families: Vec<ThemeFamily>,
    pub accent_presets: Vec<AccentPresetTokens>,
    pub accents: BTreeMap<String, BTreeMap<String, AccentRoles>>,
    pub browser_provenance: String,
    pub browser_colors: BTreeMap<String, BrowserColorRoles>,
    pub layout: Value,
    pub motion: Value,
    pub fonts: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccentPresetTokens {
    pub id: String,
    pub label: &'static str,
    pub dark: Color,
    pub light: Color,
}

impl ThemeArtifact {
    pub fn build(registry: &ThemeRegistry, browser: BrowserTokens) -> Self {
        let preset_id = |preset: AccentPreset| {
            serde_json::to_value(preset)
                .expect("accent preset serializes")
                .as_str()
                .expect("accent preset is a string")
                .to_owned()
        };
        let accents = registry
            .families
            .iter()
            .flat_map(|family| &family.variants)
            .map(|variant| {
                let roles = AccentPreset::ALL
                    .into_iter()
                    .map(|preset| {
                        (
                            preset_id(preset),
                            variant.accent_for(AccentSelection::Preset(preset)),
                        )
                    })
                    .collect();
                (variant.id.clone(), roles)
            })
            .collect();
        Self {
            schema_version: 3,
            generator: "zeron-theme-export",
            families: registry.families.clone(),
            accent_presets: AccentPreset::ALL
                .into_iter()
                .map(|preset| AccentPresetTokens {
                    id: preset_id(preset),
                    label: preset.label(),
                    dark: preset.color(Appearance::Dark),
                    light: preset.color(Appearance::Light),
                })
                .collect(),
            accents,
            browser_provenance: browser.provenance,
            browser_colors: browser.colors,
            layout: browser.layout,
            motion: browser.motion,
            fonts: browser.fonts,
        }
    }

    pub fn json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self).map(|json| json + "\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_registry;

    fn browser() -> BrowserTokens {
        serde_json::from_str(include_str!(
            "../../../web/packages/theme/src/browser-tokens.json"
        ))
        .unwrap()
    }

    #[test]
    fn exports_upstream_variants_and_accent_math_verbatim() {
        let registry = builtin_registry();
        let artifact = ThemeArtifact::build(registry, browser());
        assert_eq!(
            serde_json::to_value(&artifact.families).unwrap(),
            serde_json::to_value(&registry.families).unwrap()
        );
        for variant in registry.families.iter().flat_map(|family| &family.variants) {
            for preset in AccentPreset::ALL {
                let id = serde_json::to_value(preset).unwrap();
                assert_eq!(
                    artifact.accents[&variant.id][id.as_str().unwrap()],
                    variant.accent_for(AccentSelection::Preset(preset))
                );
            }
        }
    }

    #[test]
    fn source_changes_change_output_and_generation_is_deterministic() {
        let registry = builtin_registry();
        let original = ThemeArtifact::build(registry, browser()).json().unwrap();
        assert_eq!(
            original,
            ThemeArtifact::build(registry, browser()).json().unwrap()
        );
        let mut changed = registry.clone();
        changed.families[0].variants[0].colors.background = Color::rgb(1, 2, 3);
        assert_ne!(
            original,
            ThemeArtifact::build(&changed, browser()).json().unwrap()
        );
        let mut changed_browser = browser();
        changed_browser.layout["space"]["xs"] = serde_json::json!(9);
        assert_ne!(
            original,
            ThemeArtifact::build(registry, changed_browser)
                .json()
                .unwrap()
        );
    }
}
