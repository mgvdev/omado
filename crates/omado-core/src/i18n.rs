//! Traductions de l'interface, de la CLI et des notifications.
//!
//! Les chaînes source sont en anglais. Les catalogues gettext `po/*.po` sont
//! compilés par `build.rs` et embarqués dans les binaires. La langue suit
//! l'environnement, avec les règles de gettext : `LANGUAGE` (liste par ordre
//! de préférence), puis `LC_ALL`, `LC_MESSAGES` et `LANG`.
//!
//! Dans le code : `tr!("Today")`, `tr!("“{title}” deleted", title = t)`,
//! `trn!("{n} task", "{n} tasks", n)` (accordé à `n`, disponible comme `{n}`)
//! et `trc!("month", "May")` quand un mot a plusieurs sens.
//! `po/update.sh` extrait ces chaînes vers `po/omado.pot`.

use std::collections::HashMap;
use std::fmt::{Display, Write};
#[cfg(not(test))]
use std::sync::OnceLock;

mod catalogs {
    include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));
}

/// Clé d'un message : contexte éventuel et chaîne source (au singulier).
type Key = (Option<&'static str>, &'static str);

struct Language {
    code: &'static str,
    /// Traductions ; les formes plurielles sont séparées par `\0`.
    messages: HashMap<Key, &'static str>,
}

#[cfg(not(test))]
static ACTIVE: OnceLock<Option<Language>> = OnceLock::new();

fn active() -> Option<&'static Language> {
    // En test : anglais, sauf `with_language`, quelle que soit la locale de la machine.
    #[cfg(test)]
    return FORCED.get();
    #[cfg(not(test))]
    ACTIVE.get_or_init(|| pick(requested(|name| std::env::var(name).ok()))).as_ref()
}

pub fn gettext(msgid: &'static str) -> &'static str {
    lookup(None, msgid, 0).unwrap_or(msgid)
}

pub fn pgettext(context: &'static str, msgid: &'static str) -> &'static str {
    lookup(Some(context), msgid, 0).unwrap_or(msgid)
}

pub fn ngettext(one: &'static str, many: &'static str, n: u64) -> &'static str {
    let form = active().map_or(usize::from(n != 1), |l| plural_form(l.code, n));
    lookup(None, one, form).unwrap_or(if n == 1 { one } else { many })
}

fn lookup(context: Option<&'static str>, msgid: &'static str, form: usize) -> Option<&'static str> {
    let translation = active()?.messages.get(&(context, msgid))?;
    translation.split('\0').nth(form).filter(|s| !s.is_empty())
}

/// Remplace les `{nom}` d'une chaîne traduite. Les valeurs insérées ne sont
/// pas relues : un titre de tâche contenant `{title}` reste tel quel.
pub fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let arg = after.find('}').and_then(|end| {
            let (_, value) = args.iter().find(|(name, _)| *name == &after[..end])?;
            Some((value, end))
        });
        match arg {
            Some((value, end)) => {
                let _ = write!(out, "{value}");
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Première lettre en majuscule. Les traductions donnent la forme de milieu de
/// phrase (« aujourd'hui », « jeden Montag ») ; en tête de libellé, seule la
/// première lettre change, ce qui convient à toutes les langues.
pub fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Chaîne traduite ; avec des arguments nommés, remplace leurs `{nom}`.
#[macro_export]
macro_rules! tr {
    ($msgid:literal $(,)?) => {
        $crate::i18n::gettext($msgid)
    };
    ($msgid:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::gettext($msgid),
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+],
        )
    };
}

/// Chaîne accordée au nombre `n`, disponible comme `{n}`.
#[macro_export]
macro_rules! trn {
    ($one:literal, $many:literal, $n:expr $(, $name:ident = $value:expr)* $(,)?) => {{
        let n = $n;
        $crate::i18n::fill(
            $crate::i18n::ngettext($one, $many, n as u64),
            &[("n", &n as &dyn ::std::fmt::Display) $(, (stringify!($name), &$value as &dyn ::std::fmt::Display))*],
        )
    }};
}

/// Chaîne traduite selon un contexte, pour distinguer deux sens d'un même mot.
#[macro_export]
macro_rules! trc {
    ($context:literal, $msgid:literal $(,)?) => {
        $crate::i18n::pgettext($context, $msgid)
    };
    ($context:literal, $msgid:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::pgettext($context, $msgid),
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+],
        )
    };
}

// --- Choix de la langue ---------------------------------------------------

/// Langues demandées, par ordre de préférence. Comme gettext, `LANGUAGE`
/// n'est lu que si une locale autre que « C » est réglée.
#[cfg_attr(test, allow(dead_code))]
fn requested(var: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let var = |name: &str| var(name).filter(|v| !v.is_empty());
    let locale = var("LC_ALL").or_else(|| var("LC_MESSAGES")).or_else(|| var("LANG"));
    let mut tags = Vec::new();
    if locale.as_deref().is_some_and(|l| !matches!(base(l), "C" | "POSIX"))
        && let Some(list) = var("LANGUAGE")
    {
        tags.extend(list.split(':').filter(|t| !t.is_empty()).map(String::from));
    }
    tags.extend(locale);
    tags
}

/// Premier catalogue disponible parmi les langues demandées ; `None` : anglais.
#[cfg_attr(test, allow(dead_code))]
fn pick(tags: Vec<String>) -> Option<Language> {
    for tag in &tags {
        // « pt_BR.UTF-8@euro » : essayer « pt_BR », puis « pt ».
        let tag = base(tag);
        if matches!(tag, "C" | "POSIX" | "en") || tag.starts_with("en_") {
            return None;
        }
        let language = tag.split('_').next().unwrap_or(tag);
        if let Some(found) = load(tag).or_else(|| load(language)) {
            return Some(found);
        }
    }
    None
}

fn base(tag: &str) -> &str {
    tag.split(['.', '@']).next().unwrap_or(tag)
}

fn load(code: &str) -> Option<Language> {
    let (code, data) = catalogs::CATALOGS.iter().find(|(c, _)| *c == code)?;
    Some(Language { code, messages: parse_mo(data)? })
}

/// Lit un catalogue `.mo` (format GNU) sans rien copier : les chaînes restent
/// dans les octets embarqués.
fn parse_mo(data: &'static [u8]) -> Option<HashMap<Key, &'static str>> {
    let word = |at: usize, le: bool| -> Option<usize> {
        let bytes: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        Some(if le { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) } as usize)
    };
    let le = match word(0, true)? {
        0x9504_12de => true,
        0xde12_0495 => false,
        _ => return None,
    };
    let (count, originals, translations) = (word(8, le)?, word(12, le)?, word(16, le)?);
    let string = |table: usize, i: usize| -> Option<&'static str> {
        let (len, at) = (word(table + 8 * i, le)?, word(table + 8 * i + 4, le)?);
        std::str::from_utf8(data.get(at..at + len)?).ok()
    };
    let mut messages = HashMap::with_capacity(count);
    for i in 0..count {
        // « contexte \x04 singulier \0 pluriel » : la clé est le contexte et le singulier.
        let original = string(originals, i)?.split('\0').next().unwrap_or_default();
        let key = match original.split_once('\x04') {
            Some((context, msgid)) => (Some(context), msgid),
            None => (None, original),
        };
        messages.insert(key, string(translations, i)?);
    }
    Some(messages)
}

/// Forme plurielle pour `n`, d'après la règle `Plural-Forms` de chaque catalogue.
/// Une langue ajoutée dont la règle diffère de celle de l'anglais doit figurer
/// ici ; le test `plural_rules_match_catalogs` le rappelle.
fn plural_form(code: &str, n: u64) -> usize {
    let (n10, n100) = (n % 10, n % 100);
    let few = (2..=4).contains(&n10) && !(12..=14).contains(&n100);
    match code {
        "ja" | "zh_CN" => 0,
        "fr" | "pt_BR" => usize::from(n > 1),
        "ru" if n10 == 1 && n100 != 11 => 0,
        "pl" if n == 1 => 0,
        "ru" | "pl" if few => 1,
        "ru" | "pl" => 2,
        _ => usize::from(n != 1),
    }
}

// --- Tests ----------------------------------------------------------------

#[cfg(test)]
thread_local! {
    static FORCED: std::cell::Cell<Option<&'static Language>> = const { std::cell::Cell::new(None) };
}

/// Exécute `f` dans la langue `code`, pour ce fil seulement.
#[cfg(test)]
pub(crate) fn with_language<R>(code: &str, f: impl FnOnce() -> R) -> R {
    let language: &'static Language = Box::leak(Box::new(load(code).expect("catalogue inconnu")));
    FORCED.set(Some(language));
    let result = f();
    FORCED.set(None);
    result
}

/// Codes des catalogues embarqués.
#[cfg(test)]
pub(crate) fn available() -> impl Iterator<Item = &'static str> {
    catalogs::CATALOGS.iter().map(|(code, _)| *code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
    }

    fn picked(pairs: &[(&str, &str)]) -> Option<&'static str> {
        pick(requested(env(pairs))).map(|l| l.code)
    }

    #[test]
    fn language_follows_environment() {
        assert_eq!(picked(&[("LANG", "fr_FR.UTF-8")]), Some("fr"));
        assert_eq!(picked(&[("LANG", "de_AT.UTF-8")]), Some("de"));
        assert_eq!(picked(&[("LANG", "pt_BR.UTF-8")]), Some("pt_BR"));
        assert_eq!(picked(&[("LANG", "zh_CN.UTF-8")]), Some("zh_CN"));
        assert_eq!(picked(&[("LANG", "en_US.UTF-8")]), None);
        assert_eq!(picked(&[("LANG", "C.UTF-8")]), None);
        assert_eq!(picked(&[]), None);
        // Précédence : LC_ALL, LC_MESSAGES, LANG.
        assert_eq!(picked(&[("LANG", "fr_FR.UTF-8"), ("LC_MESSAGES", "de_DE.UTF-8")]), Some("de"));
        assert_eq!(picked(&[("LC_MESSAGES", "de_DE.UTF-8"), ("LC_ALL", "it_IT.UTF-8")]), Some("it"));
        // LANGUAGE passe devant, sauf avec la locale « C ».
        assert_eq!(picked(&[("LANG", "en_US.UTF-8"), ("LANGUAGE", "ja:fr")]), Some("ja"));
        assert_eq!(picked(&[("LANG", "en_US.UTF-8"), ("LANGUAGE", "eo:fr")]), Some("fr"));
        assert_eq!(picked(&[("LANG", "C"), ("LANGUAGE", "fr")]), None);
        // Une variante sans catalogue ne prend pas celle d'un autre pays.
        assert_eq!(picked(&[("LANG", "pt_PT.UTF-8")]), None);
        assert_eq!(picked(&[("LANG", "zh_TW.UTF-8")]), None);
    }

    #[test]
    fn placeholders() {
        let title = "Pay {n} bills";
        assert_eq!(fill("“{title}” deleted", &[("title", &title)]), "“Pay {n} bills” deleted");
        assert_eq!(fill("{a}{b} {missing}", &[("a", &1), ("b", &"x")]), "1x {missing}");
        assert_eq!(fill("no braces", &[]), "no braces");
        assert_eq!(capitalize("aujourd'hui"), "Aujourd'hui");
        assert_eq!(capitalize("écrire"), "Écrire");
        assert_eq!(capitalize("明天"), "明天");
    }

    #[test]
    fn english_without_catalog() {
        assert_eq!(tr!("Today"), "Today");
        assert_eq!(trn!("every {n} day", "every {n} days", 1), "every 1 day");
        assert_eq!(trn!("every {n} day", "every {n} days", 3), "every 3 days");
    }

    #[test]
    fn catalogs_load() {
        assert!(available().count() >= 9);
        for code in available() {
            let language = load(code).unwrap_or_else(|| panic!("{code} : catalogue illisible"));
            assert!(language.messages.len() > 100, "{code} : catalogue presque vide");
        }
        with_language("fr", || assert_eq!(tr!("Today"), "Aujourd'hui"));
    }

    #[test]
    fn plural_rules_match_catalogs() {
        for code in available() {
            let language = load(code).expect("catalogue");
            let header = language.messages[&(None, "")];
            let nplurals: usize = header
                .split("nplurals=")
                .nth(1)
                .and_then(|rest| rest.split(';').next())
                .and_then(|n| n.trim().parse().ok())
                .unwrap_or_else(|| panic!("{code} : Plural-Forms absent"));
            let forms: std::collections::BTreeSet<usize> = (0..200).map(|n| plural_form(code, n)).collect();
            assert_eq!(forms, (0..nplurals).collect(), "{code} : ajoutez sa règle à plural_form");
        }
        assert_eq!([1, 2, 5, 11, 21, 22, 25].map(|n| plural_form("ru", n)), [0, 1, 2, 2, 0, 1, 2]);
        assert_eq!([1, 2, 5, 12, 21, 22, 25].map(|n| plural_form("pl", n)), [0, 1, 2, 2, 2, 1, 2]);
        assert_eq!([0, 1, 2].map(|n| plural_form("fr", n)), [0, 0, 1]);
    }
}
