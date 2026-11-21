//! User-facing languages. Every message shown to the user (UI, settings, issues, report)
//! is looked up by key in the catalogue of the current language.
//!
//! Catalogues live in `locales/<lang>/<domain>.toml`, one file per crate (`app`, `core`,
//! `clothing`, ...). A key `x.y` in `clothing.toml` is looked up as `clothing.x.y`. Values
//! may hold `{name}` placeholders, filled by [`tf!`]; `{{` and `}}` are literal braces.
//!
//! ```ignore
//! use optistream_i18n::{t, tf};
//! let label = t("app.analyze");                       // &'static str
//! let line = tf!("core.report.files", count = 12);    // String
//! ```

mod catalog;

use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

pub use catalog::DOMAINS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lang {
    #[default]
    En,
    Fr,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Fr];

    /// ISO 639-1 code, used on the command line and in saved preferences.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Fr => "fr",
        }
    }

    /// Name of the language in that language, for the language selector.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Fr => "Français",
        }
    }

    /// Parses a code or a locale tag (`fr`, `fr-FR`, `fr_CA.UTF-8`...).
    pub fn from_code(code: &str) -> Option<Lang> {
        let primary = code.split(['-', '_', '.']).next()?.to_ascii_lowercase();
        Lang::ALL.into_iter().find(|l| l.code() == primary)
    }

    fn index(self) -> usize {
        self as usize
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(Lang::En as u8);

/// Language currently used by [`t`] and [`tf!`].
pub fn lang() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::Fr,
        _ => Lang::En,
    }
}

pub fn set_lang(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

/// Language of the operating system, English when it is not supported.
pub fn detect() -> Lang {
    sys_locale::get_locale().as_deref().and_then(Lang::from_code).unwrap_or_default()
}

/// Message for `key` in the current language. Falls back to English, then to the key itself,
/// so a missing translation shows up on screen instead of crashing.
pub fn t(key: &'static str) -> &'static str {
    t_in(lang(), key)
}

/// Message for `key` in `lang`.
pub fn t_in(lang: Lang, key: &'static str) -> &'static str {
    catalog::get(lang.index(), key).or_else(|| catalog::get(Lang::En.index(), key)).unwrap_or(key)
}

/// Message for `key` with its `{name}` placeholders replaced. Prefer the [`tf!`] macro.
pub fn format(key: &'static str, args: &[(&str, &dyn Display)]) -> String {
    fill(t(key), args)
}

fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
        } else if let (true, Some(end)) = (tail.starts_with('{'), tail.find('}')) {
            let name = &tail[1..end];
            match args.iter().find(|(n, _)| *n == name) {
                Some((_, value)) => out.push_str(&value.to_string()),
                // Unknown placeholder: keep it visible.
                None => out.push_str(&tail[..=end]),
            }
            rest = &tail[end + 1..];
        } else {
            out.push_str(&tail[..1]);
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// `tf!("domain.key", name = value, ...)`: message with placeholders, as a `String`.
/// Values only need to implement `Display`; format them beforehand for a specific precision
/// (`size = format!("{:.1}", mib)`).
#[macro_export]
macro_rules! tf {
    ($key:literal $(,)?) => {
        $crate::format($key, &[])
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::format($key, &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+])
    };
}

#[cfg(test)]
mod tests;
