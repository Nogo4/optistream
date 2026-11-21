use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use super::catalog::{parse, SOURCES};
use super::*;

fn placeholders(value: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut rest = value.replace("{{", "").replace("}}", "");
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else { break };
        names.insert(rest[start + 1..start + len].to_owned());
        rest = rest[start + len + 1..].to_owned();
    }
    names
}

/// Every key of every language, with its placeholders.
fn catalog(lang: Lang) -> Vec<(String, BTreeSet<String>)> {
    SOURCES[lang.index()]
        .iter()
        .flat_map(|(domain, src)| {
            let entries = parse(src).unwrap_or_else(|e| panic!("{}/{domain}.toml: {e}", lang.code()));
            entries.into_iter().map(move |(k, v)| (format!("{domain}.{k}"), placeholders(&v)))
        })
        .collect()
}

#[test]
fn every_catalogue_parses_without_duplicate_keys() {
    for lang in Lang::ALL {
        let mut seen = HashSet::new();
        for (key, _) in catalog(lang) {
            assert!(seen.insert(key.clone()), "{}: duplicate key {key}", lang.code());
        }
    }
}

#[test]
fn languages_have_the_same_keys_and_placeholders() {
    let reference: Vec<_> = catalog(Lang::En);
    for lang in Lang::ALL {
        let other: std::collections::BTreeMap<_, _> = catalog(lang).into_iter().collect();
        let base: std::collections::BTreeMap<_, _> = reference.iter().cloned().collect();
        for (key, names) in &base {
            let found = other.get(key).unwrap_or_else(|| panic!("{}: missing key {key}", lang.code()));
            assert_eq!(found, names, "{}: placeholders differ for {key}", lang.code());
        }
        for key in other.keys() {
            assert!(base.contains_key(key), "{}: key {key} is not in English", lang.code());
        }
    }
}

/// Every `t("...")` / `tf!("...")` key used in the workspace sources exists in English.
#[test]
fn keys_used_in_the_sources_exist() {
    let known: HashSet<String> = catalog(Lang::En).into_iter().map(|(k, _)| k).collect();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut missing = Vec::new();
    for dir in ["apps", "crates"] {
        for entry in walkdir::WalkDir::new(root.join(dir)).into_iter().filter_map(Result::ok) {
            let path = entry.path();
            // This crate's own docs and tests use example keys.
            let skipped = path.components().any(|c| c.as_os_str() == "target" || c.as_os_str() == "optistream-i18n");
            if path.extension().is_none_or(|e| e != "rs") || skipped {
                continue;
            }
            let src = std::fs::read_to_string(path).unwrap();
            for opener in ["t(\"", "t_in(", "tf!(\""] {
                for (i, _) in src.match_indices(opener) {
                    // Skip identifiers that merely end in `t` (`set(`, `get(`...).
                    if src[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let after = &src[i + opener.len()..];
                    let after = if opener == "t_in(" {
                        match after.find('"') {
                            Some(q) => &after[q + 1..],
                            None => continue,
                        }
                    } else {
                        after
                    };
                    let Some(end) = after.find('"') else { continue };
                    let key = &after[..end];
                    let in_domain = DOMAINS.iter().any(|d| key.starts_with(&format!("{d}.")));
                    if in_domain && !known.contains(key) {
                        missing.push(format!("{}: {key}", path.display()));
                    }
                }
            }
        }
    }
    assert!(missing.is_empty(), "keys missing from the English catalogues:\n{}", missing.join("\n"));
}

#[test]
fn placeholders_are_filled() {
    let n = 3;
    assert_eq!(fill("{n} files in {dir}", &[("n", &n), ("dir", &"stream")]), "3 files in stream");
    assert_eq!(fill("{{literal}} {missing}", &[]), "{literal} {missing}");
    assert_eq!(fill("no placeholder", &[]), "no placeholder");
    assert_eq!(fill("trailing {", &[]), "trailing {");
}

#[test]
fn locale_tags_are_recognised() {
    assert_eq!(Lang::from_code("fr"), Some(Lang::Fr));
    assert_eq!(Lang::from_code("fr-FR"), Some(Lang::Fr));
    assert_eq!(Lang::from_code("fr_CA.UTF-8"), Some(Lang::Fr));
    assert_eq!(Lang::from_code("EN-us"), Some(Lang::En));
    assert_eq!(Lang::from_code("de-DE"), None);
}

#[test]
fn missing_keys_fall_back_to_the_key() {
    assert_eq!(t_in(Lang::Fr, "core.__missing__"), "core.__missing__");
}

#[test]
fn parser_rejects_malformed_lines() {
    assert!(parse("a = \"ok\"\n# comment\n\nb.c = \"x \\\"q\\\" \\n\"").is_ok());
    assert!(parse("a = ok").is_err());
    assert!(parse("a \"x\"").is_err());
    assert!(parse("bad key = \"x\"").is_err());
    assert!(parse("a = \"\\q\"").is_err());
}
