//! Catalogue loading: the TOML files are embedded at build time and parsed on first use.

use std::collections::HashMap;
use std::sync::OnceLock;

/// One catalogue file per domain (crate), in each language folder.
pub const DOMAINS: [&str; 8] = ["app", "cli", "core", "texture", "mesh", "clothing", "mapping", "vehicle"];

macro_rules! sources {
    ($lang:literal) => {
        [
            ("app", include_str!(concat!("../locales/", $lang, "/app.toml"))),
            ("cli", include_str!(concat!("../locales/", $lang, "/cli.toml"))),
            ("core", include_str!(concat!("../locales/", $lang, "/core.toml"))),
            ("texture", include_str!(concat!("../locales/", $lang, "/texture.toml"))),
            ("mesh", include_str!(concat!("../locales/", $lang, "/mesh.toml"))),
            ("clothing", include_str!(concat!("../locales/", $lang, "/clothing.toml"))),
            ("mapping", include_str!(concat!("../locales/", $lang, "/mapping.toml"))),
            ("vehicle", include_str!(concat!("../locales/", $lang, "/vehicle.toml"))),
        ]
    };
}

/// Raw catalogue sources, indexed like [`crate::Lang`].
pub(crate) const SOURCES: [[(&str, &str); DOMAINS.len()]; 2] = [sources!("en"), sources!("fr")];

type Catalog = HashMap<String, String>;

static CATALOGS: OnceLock<Vec<Catalog>> = OnceLock::new();

pub(crate) fn get(lang: usize, key: &str) -> Option<&'static str> {
    let catalogs = CATALOGS.get_or_init(|| {
        SOURCES
            .iter()
            .map(|files| {
                let mut catalog = Catalog::new();
                for (domain, src) in files {
                    // A malformed file is caught by the tests; at runtime its keys fall back.
                    for (key, value) in parse(src).unwrap_or_default() {
                        catalog.insert(format!("{domain}.{key}"), value);
                    }
                }
                catalog
            })
            .collect()
    });
    catalogs[lang].get(key).map(String::as_str)
}

/// Parses the TOML subset used by the catalogues: comments, blank lines and
/// `dotted.key = "basic string"` lines (escapes `\"`, `\\`, `\n`, `\t`).
pub(crate) fn parse(src: &str) -> Result<Vec<(String, String)>, String> {
    let mut entries = Vec::new();
    for (n, line) in src.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let err = |what: &str| format!("line {}: {what}", n + 1);
        let (key, value) = line.split_once('=').ok_or_else(|| err("expected key = \"value\""))?;
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
            return Err(err("invalid key"));
        }
        let value = value.trim();
        let inner = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .filter(|_| value.len() >= 2)
            .ok_or_else(|| err("value must be a double-quoted string"))?;
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    _ => return Err(err("unknown escape")),
                },
                '"' => return Err(err("unescaped quote")),
                c => out.push(c),
            }
        }
        entries.push((key.to_owned(), out));
    }
    Ok(entries)
}
