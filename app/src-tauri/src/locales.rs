//! The app's string tables: the JSON files of ../src/locales, the same ones
//! the webview renders from, embedded at build time (build.rs lists the
//! folder). The native side (menu, quit dialog, About window) reads its
//! strings here, so a language is one file for the whole app.

use std::collections::HashMap;
use std::sync::OnceLock;

// LOCALES: &[(&str, &str)], each language's code and its JSON text.
include!(concat!(env!("OUT_DIR"), "/locales.rs"));

/// The source language: it has every key, and a key another language lacks
/// is shown in it.
pub const FALLBACK: &str = "en";

type Table = HashMap<String, String>;

fn tables() -> &'static HashMap<&'static str, Table> {
    static TABLES: OnceLock<HashMap<&'static str, Table>> = OnceLock::new();
    TABLES.get_or_init(|| {
        LOCALES
            .iter()
            .filter_map(|(code, json)| Some((*code, serde_json::from_str(json).ok()?)))
            .collect()
    })
}

/// The shipped language with exactly this code (a stored preference).
pub fn code(code: &str) -> Option<&'static str> {
    tables().keys().copied().find(|c| *c == code)
}

/// The shipped language for a device locale: "pt-BR" (or "pt_BR.UTF-8")
/// matches pt-BR.json, else pt.json.
pub fn match_tag(tag: &str) -> Option<&'static str> {
    let want = tag
        .split(['.', '@'])
        .next()
        .unwrap_or("")
        .replace('_', "-")
        .to_lowercase();
    let primary = want.split('-').next().unwrap_or("");
    let find = |w: &str| tables().keys().copied().find(|c| c.to_lowercase() == w);
    find(&want).or_else(|| find(primary))
}

/// The message in `lang`, in English when that language lacks it, and empty
/// when nobody has it.
pub fn text(lang: &str, key: &str) -> &'static str {
    let get = |l: &str| {
        tables()
            .get(l)
            .and_then(|t| t.get(key))
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    };
    get(lang).or_else(|| get(FALLBACK)).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_parses() {
        assert_eq!(tables().len(), LOCALES.len());
        assert!(code(FALLBACK).is_some());
    }

    #[test]
    fn falls_back_to_english() {
        assert_eq!(text("xx", "cancel"), "Cancel");
        assert_eq!(text("de", "cancel"), "Abbrechen");
        assert_eq!(text("de", "noSuchKey"), "");
    }

    #[test]
    fn matches_device_locales() {
        assert_eq!(match_tag("de-AT"), Some("de"));
        assert_eq!(match_tag("de_DE.UTF-8"), Some("de"));
        assert_eq!(match_tag("en-US"), Some("en"));
        assert_eq!(match_tag("xx-YY"), None);
    }
}
