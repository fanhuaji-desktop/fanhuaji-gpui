//! Persisted user settings: what the Tauri app keeps under `fanhuaji-*` localStorage keys,
//! stored here as one JSON file.

use crate::i18n::Locale;
use fanhuaji_core::ConvertParams;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Naming {
    /// `name.Taiwan.txt`
    #[default]
    Auto,
    /// `name.<custom_suffix>.txt`
    Suffix,
    /// Write over the input, keeping a backup for Undo.
    Overwrite,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum ModuleChoice {
    #[default]
    Auto,
    Enable,
    Disable,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Settings {
    pub converter: String,
    pub naming: Naming,
    pub custom_suffix: String,
    /// None: next to the input file.
    pub save_folder: Option<PathBuf>,
    pub pre_replace: String,
    pub post_replace: String,
    pub protect_replace: String,
    /// Only modules the user changed from Auto.
    pub modules: BTreeMap<String, ModuleChoice>,
    pub auto_convert: bool,
    /// None: follow the system language.
    pub locale: Option<Locale>,
    pub theme: ThemePref,
}

/// The settings that change a conversion's output, to tell when a saved result is stale.
#[derive(Clone, PartialEq, Debug)]
pub struct OutputKey {
    pub converter: String,
    replace: [String; 3],
    modules: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            converter: "Taiwan".into(),
            naming: Naming::Auto,
            custom_suffix: String::new(),
            save_folder: None,
            pre_replace: String::new(),
            post_replace: String::new(),
            protect_replace: String::new(),
            modules: BTreeMap::new(),
            auto_convert: true,
            locale: None,
            theme: ThemePref::System,
        }
    }
}

impl Settings {
    /// Missing or unreadable files give the defaults, so a bad file never blocks launch.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Written to a temp file and renamed, so a crash mid-write can't corrupt the settings.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }

    /// The `modules` API parameter: `{"Name":1}` to force on, `0` to force off.
    pub fn module_overrides(&self) -> String {
        let overrides: BTreeMap<&str, u8> = self
            .modules
            .iter()
            .filter_map(|(k, v)| match v {
                ModuleChoice::Enable => Some((k.as_str(), 1)),
                ModuleChoice::Disable => Some((k.as_str(), 0)),
                ModuleChoice::Auto => None,
            })
            .collect();
        serde_json::to_string(&overrides).expect("a string map always serializes")
    }

    pub fn params(&self, input: &Path, file_id: String) -> ConvertParams {
        ConvertParams {
            file_id,
            input_path: input.to_string_lossy().into_owned(),
            converter: self.converter.clone(),
            save_folder: self
                .save_folder
                .as_ref()
                .map_or("same".into(), |p| p.to_string_lossy().into_owned()),
            naming: match self.naming {
                Naming::Auto => "auto",
                Naming::Suffix => "suffix",
                Naming::Overwrite => "overwrite",
            }
            .into(),
            custom_suffix: self.custom_suffix.clone(),
            pre_replace: self.pre_replace.clone(),
            post_replace: self.post_replace.clone(),
            protect_replace: self.protect_replace.clone(),
            modules: self.module_overrides(),
        }
    }

    pub fn output_key(&self) -> OutputKey {
        OutputKey {
            converter: self.converter.clone(),
            replace: [
                self.pre_replace.clone(),
                self.post_replace.clone(),
                self.protect_replace.clone(),
            ],
            modules: self.module_overrides(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("fanhuaji-gpui-settings-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn defaults_match_the_tauri_app() {
        let s = Settings::default();
        assert_eq!(s.converter, "Taiwan");
        assert_eq!(s.naming, Naming::Auto);
        assert_eq!(s.save_folder, None);
        assert!(s.auto_convert, "auto-convert is on unless turned off");
        assert_eq!(s.locale, None, "follows the system language until chosen");
        assert_eq!(s.theme, ThemePref::System);
    }

    #[test]
    fn load_falls_back_to_defaults_for_missing_or_corrupt_files() {
        assert_eq!(Settings::load(&tmp("missing.json")), Settings::default());
        let bad = tmp("bad.json");
        std::fs::write(&bad, "{not json").unwrap();
        assert_eq!(Settings::load(&bad), Settings::default());
    }

    #[test]
    fn load_fills_missing_fields_with_defaults() {
        let p = tmp("partial.json");
        std::fs::write(&p, r#"{"converter":"Hongkong"}"#).unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.converter, "Hongkong");
        assert!(s.auto_convert);
    }

    #[test]
    fn save_then_load_round_trips() {
        let p = tmp("nested/dir/settings.json");
        let s = Settings {
            naming: Naming::Suffix,
            custom_suffix: "tw".into(),
            save_folder: Some("/tmp/out".into()),
            pre_replace: "a=b\nc=d".into(),
            modules: [("Smooth".into(), ModuleChoice::Disable)].into(),
            auto_convert: false,
            locale: Some(Locale::En),
            theme: ThemePref::Dark,
            ..Default::default()
        };
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);
    }

    #[test]
    fn module_overrides_map_enable_and_disable_and_skip_auto() {
        let mut s = Settings::default();
        assert_eq!(s.module_overrides(), "{}");
        s.modules.insert("A".into(), ModuleChoice::Enable);
        s.modules.insert("B".into(), ModuleChoice::Disable);
        s.modules.insert("C".into(), ModuleChoice::Auto);
        assert_eq!(s.module_overrides(), r#"{"A":1,"B":0}"#);
    }

    #[test]
    fn params_carry_the_settings() {
        let mut s = Settings::default();
        let p = s.params(Path::new("/in/a.srt"), "7".into());
        assert_eq!(
            (
                p.file_id.as_str(),
                p.input_path.as_str(),
                p.converter.as_str()
            ),
            ("7", "/in/a.srt", "Taiwan")
        );
        assert_eq!(
            (p.save_folder.as_str(), p.naming.as_str()),
            ("same", "auto")
        );

        s.save_folder = Some("/out".into());
        s.naming = Naming::Overwrite;
        s.protect_replace = "台灣".into();
        let p = s.params(Path::new("/in/a.srt"), "7".into());
        assert_eq!(
            (p.save_folder.as_str(), p.naming.as_str()),
            ("/out", "overwrite")
        );
        assert_eq!(p.protect_replace, "台灣");
        assert_eq!(p.modules, "{}");
    }

    #[test]
    fn output_key_tracks_only_what_changes_the_output() {
        let base = Settings::default();
        let mut s = base.clone();
        s.naming = Naming::Suffix;
        s.save_folder = Some("/x".into());
        s.auto_convert = false;
        assert_eq!(s.output_key(), base.output_key());

        for change in [
            |s: &mut Settings| s.converter = "China".into(),
            |s: &mut Settings| s.post_replace = "a=b".into(),
            |s: &mut Settings| {
                s.modules.insert("A".into(), ModuleChoice::Enable);
            },
        ] {
            let mut s = base.clone();
            change(&mut s);
            assert_ne!(s.output_key(), base.output_key());
        }
    }
}
