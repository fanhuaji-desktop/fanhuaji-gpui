//! UI strings and error-code translation, ported from the Tauri app's `i18n.ts`.
//! The locale files are copies of the Tauri app's, plus a few GPUI-only keys.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Locale {
    #[serde(rename = "en")]
    En,
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "zh-TW")]
    ZhTw,
}

pub const LOCALES: [(Locale, &str); 3] = [
    (Locale::En, "English"),
    (Locale::ZhCn, "简体中文"),
    (Locale::ZhTw, "正體中文"),
];

type Messages = HashMap<String, String>;

static MESSAGES: LazyLock<[Messages; 3]> = LazyLock::new(|| {
    let parse = |json: &str| serde_json::from_str::<Messages>(json).expect("locale JSON is valid");
    [
        parse(include_str!("../locales/en.json")),
        parse(include_str!("../locales/zh-CN.json")),
        parse(include_str!("../locales/zh-TW.json")),
    ]
});

static CURRENT: AtomicU8 = AtomicU8::new(Locale::ZhTw as u8);

impl Locale {
    /// The default and the fallback for missing keys.
    pub const DEFAULT: Locale = Locale::ZhTw;

    fn messages(self) -> &'static Messages {
        &MESSAGES[self as usize]
    }

    /// Pick a locale from a system language tag such as `zh-Hant-TW` or `en-US`.
    pub fn detect(lang: Option<&str>) -> Locale {
        let Some(lang) = lang.filter(|l| l.starts_with("zh")) else {
            // Non-Chinese systems get English, as in the Tauri app.
            return Locale::En;
        };
        if lang.contains("TW") || lang.contains("Hant") {
            Locale::ZhTw
        } else if lang.contains("CN") || lang.contains("Hans") || lang == "zh" {
            Locale::ZhCn
        } else {
            // zh-HK and other variants write Traditional.
            Locale::ZhTw
        }
    }
}

pub fn locale() -> Locale {
    match CURRENT.load(Ordering::Relaxed) {
        0 => Locale::En,
        1 => Locale::ZhCn,
        _ => Locale::ZhTw,
    }
}

pub fn set_locale(locale: Locale) {
    CURRENT.store(locale as u8, Ordering::Relaxed);
}

/// Look up `key` in `locale`, falling back to the default locale and then to the key,
/// and fill each `{name}` placeholder literally.
pub fn t_in(locale: Locale, key: &str, params: &[(&str, &str)]) -> String {
    let mut text = locale
        .messages()
        .get(key)
        .or_else(|| Locale::DEFAULT.messages().get(key))
        .map_or_else(|| key.to_string(), String::clone);
    for (name, value) in params {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

pub fn t(key: &str) -> String {
    t_in(locale(), key, &[])
}

pub fn tp(key: &str, params: &[(&str, &str)]) -> String {
    t_in(locale(), key, params)
}

/// Translate a core error string `CODE` or `CODE:detail` via `errors.<CODE>`; anything
/// that isn't a known code comes back unchanged.
pub fn translate_error_in(locale: Locale, raw: &str) -> String {
    let (code, detail) = raw.split_once(':').unwrap_or((raw, ""));
    let is_code = code.starts_with(|c: char| c.is_ascii_uppercase())
        && code
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !is_code {
        return raw.to_string();
    }
    let key = format!("errors.{code}");
    let translated = t_in(locale, &key, &[("detail", detail)]);
    if translated == key {
        raw.to_string()
    } else {
        translated
    }
}

pub fn translate_error(raw: &str) -> String {
    translate_error_in(locale(), raw)
}

#[cfg(test)]
mod tests {
    //! Ported from the Tauri app's `i18n.test.ts` (the DOM-only cases have no equivalent).
    use super::*;
    use Locale::*;

    #[test]
    fn returns_each_locales_string() {
        assert_eq!(t_in(ZhTw, "app.title", &[]), "繁化姬");
        assert_eq!(t_in(En, "app.title", &[]), "Fanhuaji");
        assert_eq!(t_in(ZhCn, "drop.title", &[]), "拖放文件到此处");
    }

    #[test]
    fn returns_the_key_when_no_locale_has_it() {
        assert_eq!(t_in(En, "nonexistent.key", &[]), "nonexistent.key");
    }

    #[test]
    fn interpolates_params() {
        assert_eq!(t_in(ZhTw, "update.found", &[("version", "3.0.0")]), "發現新版本 3.0.0");
        assert_eq!(
            t_in(
                ZhTw,
                "file.convertingChapter",
                &[("current", "2"), ("total", "10"), ("name", " Chapter 2")]
            ),
            "轉換中… (2/10 Chapter 2)"
        );
        assert_eq!(t_in(En, "update.found", &[("version", "2.0.0")]), "New version 2.0.0 available");
        assert!(t_in(ZhTw, "update.failed", &[("error", "$100 error")]).contains("$100 error"));
        assert_eq!(
            t_in(ZhTw, "dom.missingElement", &[("selector", "#test")]),
            "必要元素不存在：#test"
        );
    }

    #[test]
    fn detects_the_locale_from_the_system_language() {
        assert_eq!(Locale::detect(Some("zh-TW")), ZhTw);
        assert_eq!(Locale::detect(Some("zh-CN")), ZhCn);
        assert_eq!(Locale::detect(Some("zh-Hans")), ZhCn);
        assert_eq!(Locale::detect(Some("zh-Hans-CN")), ZhCn);
        assert_eq!(Locale::detect(Some("zh-Hant")), ZhTw);
        assert_eq!(Locale::detect(Some("zh-Hant-HK")), ZhTw);
        assert_eq!(Locale::detect(Some("zh-HK")), ZhTw, "other zh variants stay Traditional");
        assert_eq!(Locale::detect(Some("zh")), ZhCn);
        assert_eq!(Locale::detect(Some("en-US")), En);
        assert_eq!(Locale::detect(Some("ja")), En);
        assert_eq!(Locale::detect(None), En);
    }

    #[test]
    fn translates_error_codes() {
        assert_eq!(translate_error_in(ZhTw, "FILE_TOO_LARGE"), "檔案過大（上限 50 MB）");
        assert_eq!(translate_error_in(En, "FILE_TOO_LARGE"), "File too large (limit 50 MB)");
        assert_eq!(translate_error_in(En, "API_ERROR:timeout"), "API error: timeout");
        assert_eq!(
            translate_error_in(ZhTw, "EPUB_PARTIAL_FAILED:3/10"),
            "部分章節失敗（3/10）"
        );
        assert_eq!(
            translate_error_in(En, "API_ERROR:$100 credit failed"),
            "API error: $100 credit failed"
        );
        assert!(
            translate_error_in(ZhTw, "NET_REQUEST_FAILED:connection to api.example.com:443 refused")
                .ends_with("connection to api.example.com:443 refused"),
            "detail keeps its own colons"
        );
    }

    #[test]
    fn leaves_unknown_or_non_code_errors_unchanged() {
        assert_eq!(
            translate_error_in(ZhTw, "TOTALLY_UNKNOWN_CODE:detail"),
            "TOTALLY_UNKNOWN_CODE:detail"
        );
        assert_eq!(translate_error_in(ZhTw, "some plain error message"), "some plain error message");
        assert_eq!(translate_error_in(ZhTw, "lower_case:x"), "lower_case:x");
        assert_eq!(translate_error_in(ZhTw, ""), "");
    }

    #[test]
    fn every_locale_has_the_same_keys() {
        let keys = |l: Locale| {
            let mut k: Vec<&String> = l.messages().keys().collect();
            k.sort();
            k
        };
        assert_eq!(keys(En), keys(ZhTw));
        assert_eq!(keys(ZhCn), keys(ZhTw));
    }

    #[test]
    fn global_locale_switches() {
        set_locale(En);
        assert_eq!(locale(), En);
        set_locale(ZhTw);
        assert_eq!(locale(), ZhTw);
    }
}
