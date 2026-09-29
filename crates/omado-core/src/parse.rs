//! Saisie rapide façon Todoist : `Appeler Paul demain 9h #perso @tel !1`.
//!
//! Le texte est découpé en mots ; les expressions reconnues (dates, heures,
//! récurrences, `#liste`, `@étiquette`, `!priorité`) sont retirées du titre et
//! renvoyées avec leur position, pour que l'interface puisse les surligner.
//! Français et anglais sont acceptés indifféremment.

use std::ops::Range;

use chrono::{Datelike, Days, Duration, Months, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Weekday};

use crate::model::{Due, NewTask, Priority};
use crate::recurrence::{Freq, Recurrence};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpanKind {
    Date,
    Time,
    Recurrence,
    List,
    Tag,
    Priority,
}

/// Portion reconnue du texte saisi (plage en octets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub kind: SpanKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    pub title: String,
    pub due: Option<Due>,
    /// Nom de liste tel que saisi après `#`, à résoudre par le stockage.
    pub list: Option<String>,
    pub tags: Vec<String>,
    pub priority: Priority,
    pub recurrence: Option<Recurrence>,
    pub spans: Vec<Span>,
}

impl Parsed {
    /// Tâche prête à créer ; le rappel tombe à l'heure d'échéance quand il y en a une.
    pub fn into_new_task(self) -> NewTask {
        NewTask {
            title: self.title,
            priority: self.priority,
            remind_at: self.due.and_then(|d| d.time.map(|t| d.date.and_time(t))),
            due: self.due,
            recurrence: self.recurrence,
            tags: self.tags,
            ..NewTask::default()
        }
    }
}

pub fn parse(input: &str, now: NaiveDateTime) -> Parsed {
    Parser { toks: tokenize(input), now, today: now.date() }.run()
}

/// Minuscules sans accents, pour comparer des mots saisis à la main.
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' => 'i',
            'ô' | 'ö' | 'ó' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ç' => 'c',
            '’' | '‘' => '\'',
            c => c,
        })
        .collect()
}

/// `#courses-maison` désigne-t-il la liste « Courses maison » ?
pub fn list_matches(list_name: &str, typed: &str) -> bool {
    let squash = |s: &str| fold(s).chars().filter(|c| !matches!(c, ' ' | '-' | '_')).collect::<String>();
    squash(list_name) == squash(typed)
}

struct Token<'a> {
    raw: &'a str,
    norm: String,
    range: Range<usize>,
}

fn tokenize(input: &str) -> Vec<Token<'_>> {
    let mut toks = Vec::new();
    let mut start = None;
    let mut push = |s: usize, e: usize| {
        let raw = &input[s..e];
        let norm = fold(raw.trim_end_matches([',', ';', '.', '!', '?']));
        toks.push(Token { raw, norm, range: s..e });
    };
    for (i, c) in input.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                push(s, i);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        push(s, input.len());
    }
    toks
}

enum Hit {
    Date { date: NaiveDate, default_time: Option<NaiveTime> },
    DateTime(NaiveDateTime),
    Time(NaiveTime),
    Recurrence(Recurrence),
    List(String),
    Tag(String),
    Priority(Priority),
}

impl Hit {
    fn kind(&self) -> SpanKind {
        match self {
            Hit::Date { .. } | Hit::DateTime(_) => SpanKind::Date,
            Hit::Time(_) => SpanKind::Time,
            Hit::Recurrence(_) => SpanKind::Recurrence,
            Hit::List(_) => SpanKind::List,
            Hit::Tag(_) => SpanKind::Tag,
            Hit::Priority(_) => SpanKind::Priority,
        }
    }
}

/// Petits mots qui peuvent précéder une date ou une heure (« à 9h », « le 15 mai »).
const CONNECTORS: &[&str] = &["a", "at", "le", "on", "pour", "for", "vers", "by", "avant", "d'ici"];

struct Parser<'a> {
    toks: Vec<Token<'a>>,
    now: NaiveDateTime,
    today: NaiveDate,
}

impl Parser<'_> {
    fn w(&self, i: usize) -> Option<&str> {
        self.toks.get(i).map(|t| t.norm.as_str())
    }

    fn run(self) -> Parsed {
        let mut out = Parsed::default();
        let mut date = None;
        let mut default_time = None;
        let mut exact = None;
        let mut time = None;
        let mut priority_set = false;
        let mut title = Vec::new();

        let mut i = 0;
        while i < self.toks.len() {
            let Some((len, hit)) = self.hit_at(i) else {
                title.push(self.toks[i].raw);
                i += 1;
                continue;
            };
            let kind = hit.kind();
            let accepted = match hit {
                Hit::Date { date: d, default_time: t } if date.is_none() && exact.is_none() => {
                    date = Some(d);
                    default_time = t;
                    true
                }
                Hit::DateTime(dt) if date.is_none() && exact.is_none() => {
                    exact = Some(dt);
                    true
                }
                Hit::Time(t) if time.is_none() => {
                    time = Some(t);
                    true
                }
                Hit::Recurrence(r) if out.recurrence.is_none() => {
                    out.recurrence = Some(r);
                    true
                }
                Hit::List(name) if out.list.is_none() => {
                    out.list = Some(name);
                    true
                }
                Hit::Tag(tag) => {
                    if !out.tags.contains(&tag) {
                        out.tags.push(tag);
                    }
                    true
                }
                Hit::Priority(p) if !priority_set => {
                    out.priority = p;
                    priority_set = true;
                    true
                }
                _ => false,
            };
            if accepted {
                let range = self.toks[i].range.start..self.toks[i + len - 1].range.end;
                out.spans.push(Span { range, kind });
                i += len;
            } else {
                title.push(self.toks[i].raw);
                i += 1;
            }
        }

        out.due = if let Some(dt) = exact {
            Some(Due::at(dt.date(), time.unwrap_or(dt.time())))
        } else if let Some(d) = date {
            Some(Due { date: d, time: time.or(default_time) })
        } else if let Some(r) = &out.recurrence {
            let mut d = r.first_on_or_after(self.today);
            if time.is_some_and(|t| d == self.today && t <= self.now.time()) {
                d = r.next_after(d);
            }
            Some(Due { date: d, time })
        } else {
            time.map(|t| Due::at(if t > self.now.time() { self.today } else { self.today + Days::new(1) }, t))
        };
        if let (Some(r), Some(due)) = (out.recurrence.take(), out.due) {
            out.recurrence = Some(r.anchored(due.date));
        }
        out.title = title.join(" ");
        out
    }

    fn hit_at(&self, i: usize) -> Option<(usize, Hit)> {
        if let Some(hit) = self.symbol(i) {
            return Some((1, hit));
        }
        if let Some(found) = self.recurrence(i).or_else(|| self.date(i)).or_else(|| self.time(i)) {
            return Some(found);
        }
        let first = self.w(i)?;
        if CONNECTORS.contains(&first)
            && let Some((len, hit)) = self.date(i + 1).or_else(|| self.time(i + 1))
        {
            return Some((len + 1, hit));
        }
        // « le 15 » : prochain 15 du mois.
        if first == "le"
            && let Some(day) = self.w(i + 1).and_then(parse_day)
        {
            return self.next_month_day(day).map(|d| (2, Hit::Date { date: d, default_time: None }));
        }
        None
    }

    fn symbol(&self, i: usize) -> Option<Hit> {
        let raw = self.toks[i].raw.trim_end_matches([',', ';', '.', '?']);
        let name =
            |prefix: char| raw.strip_prefix(prefix).filter(|rest| rest.chars().next().is_some_and(char::is_alphabetic));
        if let Some(list) = name('#') {
            return Some(Hit::List(list.to_string()));
        }
        if let Some(tag) = name('@') {
            return Some(Hit::Tag(tag.to_lowercase()));
        }
        let level = raw.strip_prefix('!').or_else(|| raw.strip_prefix(['p', 'P']))?;
        Priority::from_level(level.parse().ok()?).map(Hit::Priority)
    }

    fn date(&self, i: usize) -> Option<(usize, Hit)> {
        let a = self.w(i)?;
        let b = self.w(i + 1);
        let c = self.w(i + 2);
        let day = |len: usize, date: NaiveDate| Some((len, Hit::Date { date, default_time: None }));
        let evening = NaiveTime::from_hms_opt(19, 0, 0);

        match (a, b, c) {
            ("aujourd'hui" | "aujourdhui" | "auj" | "today", ..) => return day(1, self.today),
            ("tonight", ..) => return Some((1, Hit::Date { date: self.today, default_time: evening })),
            ("ce", Some("soir"), _) => return Some((2, Hit::Date { date: self.today, default_time: evening })),
            ("demain" | "tomorrow", ..) => return day(1, self.today + Days::new(1)),
            ("apres-demain", ..) => return day(1, self.today + Days::new(2)),
            ("apres", Some("demain"), _) => return day(2, self.today + Days::new(2)),
            ("next", Some("week"), _) | ("semaine", Some("prochaine"), _) => {
                return day(2, self.next_weekday(Weekday::Mon));
            }
            ("la", Some("semaine"), Some("prochaine")) => return day(3, self.next_weekday(Weekday::Mon)),
            ("next", Some("month"), _) | ("mois", Some("prochain"), _) => return day(2, self.first_of_next_month()),
            ("le", Some("mois"), Some("prochain")) => return day(3, self.first_of_next_month()),
            ("ce" | "this", Some("week-end" | "weekend" | "we"), _) => return day(2, self.weekend()),
            ("next", Some(w), _) => {
                if let Some(wd) = weekday(w, false) {
                    return day(2, self.next_weekday(wd));
                }
            }
            ("dans" | "in", ..) => return self.offset(i + 1).map(|(len, hit)| (len + 1, hit)),
            _ => {}
        }

        if let Some(wd) = weekday(a, false) {
            let len = if b == Some("prochain") { 2 } else { 1 };
            return day(len, self.next_weekday(wd));
        }
        if let Some(d) = self.numeric_date(a) {
            return day(1, d);
        }
        // « 15 octobre [2027] » ou « october 15 [2027] »
        let (d, m) = match (parse_day(a), b.and_then(month)) {
            (Some(d), Some(m)) => (d, m),
            _ => (b.and_then(parse_day)?, month(a)?),
        };
        match c.and_then(parse_year) {
            Some(y) => day(3, NaiveDate::from_ymd_opt(y, m, d)?),
            None => day(2, self.upcoming(m, d)?),
        }
    }

    /// « dans 3 jours », « in 2 weeks », « dans 30 min », « in 2h ».
    fn offset(&self, i: usize) -> Option<(usize, Hit)> {
        let a = self.w(i)?;
        if let Some(n) = parse_count(a) {
            return self.apply_offset(n, self.w(i + 1)?).map(|hit| (2, hit));
        }
        let split = a.find(|c: char| !c.is_ascii_digit()).filter(|&p| p > 0)?;
        let n = a[..split].parse().ok()?;
        self.apply_offset(n, &a[split..]).map(|hit| (1, hit))
    }

    fn apply_offset(&self, n: u32, unit: &str) -> Option<Hit> {
        let date = |d: NaiveDate| Some(Hit::Date { date: d, default_time: None });
        let at = |delta: Duration| {
            let dt = self.now + delta;
            Some(Hit::DateTime(dt.with_second(0)?.with_nanosecond(0)?))
        };
        match unit {
            "jour" | "jours" | "j" | "day" | "days" | "d" => date(self.today + Days::new(n.into())),
            "semaine" | "semaines" | "sem" | "week" | "weeks" | "w" => date(self.today + Days::new(7 * u64::from(n))),
            "mois" | "month" | "months" => date(self.today.checked_add_months(Months::new(n))?),
            "an" | "ans" | "annee" | "annees" | "year" | "years" | "y" => {
                date(self.today.checked_add_months(Months::new(12 * n))?)
            }
            "h" | "heure" | "heures" | "hour" | "hours" | "hr" | "hrs" => at(Duration::hours(n.into())),
            "min" | "mins" | "minute" | "minutes" => at(Duration::minutes(n.into())),
            _ => None,
        }
    }

    fn time(&self, i: usize) -> Option<(usize, Hit)> {
        let a = self.w(i)?;
        let hit = |len: usize, h: u32, m: u32| NaiveTime::from_hms_opt(h, m, 0).map(|t| (len, Hit::Time(t)));

        if matches!(a, "midi" | "noon") {
            return hit(1, 12, 0);
        }
        // « 9h », « 14h30 »
        if let Some((h, m)) = a.split_once('h') {
            if is_digits(h, 1..=2) && (m.is_empty() || is_digits(m, 2..=2)) {
                return hit(1, h.parse().ok()?, m.parse().unwrap_or(0));
            }
            return None;
        }
        // « 9am », « 9:30pm », « 9 pm »
        for suffix in ["am", "pm"] {
            if let Some(clock) = a.strip_suffix(suffix) {
                let (h, m) = clock_parts(clock)?;
                return hit(1, to_24h(h, suffix)?, m);
            }
        }
        let (h, m) = clock_parts(a)?;
        match self.w(i + 1) {
            Some(suffix @ ("am" | "pm")) => hit(2, to_24h(h, suffix)?, m),
            // Un nombre seul n'est pas une heure ; « 14:30 » oui.
            _ if a.contains(':') => hit(1, h, m),
            _ => None,
        }
    }

    fn recurrence(&self, i: usize) -> Option<(usize, Hit)> {
        let a = self.w(i)?;
        let simple = |r: Recurrence| Some((1, Hit::Recurrence(r)));
        match (a, self.w(i + 1)) {
            ("daily" | "quotidien" | "quotidienne", _) => simple(Recurrence::new(Freq::Daily, 1)),
            ("weekly" | "hebdo" | "hebdomadaire", _) => simple(Recurrence::new(Freq::Weekly, 1)),
            ("monthly" | "mensuel" | "mensuelle", _) => simple(Recurrence::new(Freq::Monthly, 1)),
            ("yearly" | "annually" | "annuel" | "annuelle", _) => simple(Recurrence::new(Freq::Yearly, 1)),
            ("weekdays", _) => simple(Recurrence::weekdays_only()),
            ("en", Some("semaine")) => Some((2, Hit::Recurrence(Recurrence::weekdays_only()))),
            ("tous" | "toutes", Some("les")) => {
                self.recurrence_body(i + 2).map(|(len, r)| (len + 2, Hit::Recurrence(r)))
            }
            ("chaque" | "every", _) => self.recurrence_body(i + 1).map(|(len, r)| (len + 1, Hit::Recurrence(r))),
            _ => None,
        }
    }

    /// Ce qui suit « tous les » / « chaque » / « every » : `[n] unité` ou une liste de jours.
    fn recurrence_body(&self, j: usize) -> Option<(usize, Recurrence)> {
        if let Some(found) = self.recurrence_unit(j, 1) {
            return Some(found);
        }
        let n = parse_count(self.w(j)?)?;
        self.recurrence_unit(j + 1, n).map(|(len, r)| (len + 1, r))
    }

    fn recurrence_unit(&self, j: usize, n: u32) -> Option<(usize, Recurrence)> {
        let u = self.w(j)?;
        let rec = |freq| Some((1, Recurrence::new(freq, n)));
        match u {
            "jour" | "jours" if matches!(self.w(j + 1), Some("ouvre" | "ouvres" | "ouvrable" | "ouvrables")) => {
                Some((2, Recurrence::weekdays_only()))
            }
            "jour" | "jours" | "day" | "days" => rec(Freq::Daily),
            "weekday" | "weekdays" => Some((1, Recurrence::weekdays_only())),
            "semaine" | "semaines" | "week" | "weeks" => rec(Freq::Weekly),
            "mois" | "month" | "months" => rec(Freq::Monthly),
            "an" | "ans" | "annee" | "annees" | "year" | "years" => rec(Freq::Yearly),
            "week-end" | "week-ends" | "weekend" | "weekends" => {
                Some((1, Recurrence::weekly_on(n, [Weekday::Sat, Weekday::Sun])))
            }
            _ => {
                let mut days = vec![weekday(u, true)?];
                let mut k = j + 1;
                loop {
                    let after_comma = self.toks[k - 1].raw.ends_with(',');
                    match self.w(k) {
                        Some("et" | "and") => match self.w(k + 1).and_then(|w| weekday(w, true)) {
                            Some(d) => {
                                days.push(d);
                                k += 2;
                            }
                            None => break,
                        },
                        Some(w) if after_comma => match weekday(w, true) {
                            Some(d) => {
                                days.push(d);
                                k += 1;
                            }
                            None => break,
                        },
                        _ => break,
                    }
                }
                Some((k - j, Recurrence::weekly_on(n, days)))
            }
        }
    }

    /// Prochain `wd`, strictement après aujourd'hui.
    fn next_weekday(&self, wd: Weekday) -> NaiveDate {
        (1..=7).map(|i| self.today + Days::new(i)).find(|d| d.weekday() == wd).expect("jour trouvé sur 7 jours")
    }

    fn weekend(&self) -> NaiveDate {
        match self.today.weekday() {
            Weekday::Sat | Weekday::Sun => self.today,
            _ => self.next_weekday(Weekday::Sat),
        }
    }

    fn first_of_next_month(&self) -> NaiveDate {
        let first = self.today.with_day(1).expect("le 1er existe");
        first + Months::new(1)
    }

    /// Prochain jour `day` du mois (ce mois-ci s'il n'est pas passé).
    fn next_month_day(&self, day: u32) -> Option<NaiveDate> {
        let first = self.today.with_day(1)?;
        (0..12).filter_map(|m| (first + Months::new(m)).with_day(day)).find(|d| *d >= self.today)
    }

    /// Jour/mois sans année : cette année, ou la suivante si la date est passée.
    fn upcoming(&self, month: u32, day: u32) -> Option<NaiveDate> {
        let this_year = NaiveDate::from_ymd_opt(self.today.year(), month, day);
        match this_year {
            Some(d) if d >= self.today => Some(d),
            _ => (1..=4).find_map(|y| NaiveDate::from_ymd_opt(self.today.year() + y, month, day)),
        }
    }

    /// `15/10`, `15/10/2026`, `15/10/26`, `2026-10-15`.
    fn numeric_date(&self, s: &str) -> Option<NaiveDate> {
        if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            return Some(d);
        }
        let parts: Vec<&str> = s.split('/').collect();
        match parts.as_slice() {
            [d, m] if is_digits(d, 1..=2) && is_digits(m, 1..=2) => self.upcoming(m.parse().ok()?, d.parse().ok()?),
            [d, m, y] if is_digits(d, 1..=2) && is_digits(m, 1..=2) && (is_digits(y, 2..=2) || is_digits(y, 4..=4)) => {
                let y: i32 = y.parse().ok()?;
                let y = if y < 100 { 2000 + y } else { y };
                NaiveDate::from_ymd_opt(y, m.parse().ok()?, d.parse().ok()?)
            }
            _ => None,
        }
    }
}

fn is_digits(s: &str, len: std::ops::RangeInclusive<usize>) -> bool {
    len.contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

fn clock_parts(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':').unwrap_or((s, "00"));
    (is_digits(h, 1..=2) && is_digits(m, 2..=2)).then(|| Some((h.parse().ok()?, m.parse().ok()?)))?
}

fn to_24h(h: u32, suffix: &str) -> Option<u32> {
    match (h, suffix) {
        (1..=11, "am") => Some(h),
        (12, "am") => Some(0),
        (1..=11, "pm") => Some(h + 12),
        (12, "pm") => Some(12),
        _ => None,
    }
}

fn parse_day(s: &str) -> Option<u32> {
    let digits = ["er", "st", "nd", "rd", "th"].iter().find_map(|suf| s.strip_suffix(suf)).unwrap_or(s);
    if !is_digits(digits, 1..=2) {
        return None;
    }
    digits.parse().ok().filter(|d| (1..=31).contains(d))
}

fn parse_year(s: &str) -> Option<i32> {
    is_digits(s, 4..=4).then(|| s.parse().ok())?
}

fn parse_count(s: &str) -> Option<u32> {
    if is_digits(s, 1..=3) {
        return s.parse().ok().filter(|n| *n >= 1);
    }
    Some(match s {
        "un" | "une" | "one" | "a" | "an" => 1,
        "deux" | "two" | "other" => 2,
        "trois" | "three" => 3,
        "quatre" | "four" => 4,
        "cinq" | "five" => 5,
        "six" => 6,
        _ => return None,
    })
}

/// Nom de jour, français ou anglais ; `plural` accepte aussi « lundis », « mondays ».
fn weekday(s: &str, plural: bool) -> Option<Weekday> {
    let s = if plural { s.strip_suffix('s').unwrap_or(s) } else { s };
    Some(match s {
        "lundi" | "monday" => Weekday::Mon,
        "mardi" | "tuesday" => Weekday::Tue,
        "mercredi" | "wednesday" => Weekday::Wed,
        "jeudi" | "thursday" => Weekday::Thu,
        "vendredi" | "friday" => Weekday::Fri,
        "samedi" | "saturday" => Weekday::Sat,
        "dimanche" | "sunday" => Weekday::Sun,
        _ => return None,
    })
}

fn month(s: &str) -> Option<u32> {
    Some(match s {
        "janvier" | "janv" | "january" | "jan" => 1,
        "fevrier" | "fevr" | "fev" | "february" | "feb" => 2,
        "mars" | "march" | "mar" => 3,
        "avril" | "avr" | "april" | "apr" => 4,
        "mai" | "may" => 5,
        "juin" | "june" | "jun" => 6,
        "juillet" | "juil" | "july" | "jul" => 7,
        "aout" | "august" | "aug" => 8,
        "septembre" | "sept" | "september" | "sep" => 9,
        "octobre" | "oct" | "october" => 10,
        "novembre" | "nov" | "november" => 11,
        "decembre" | "dec" | "december" => 12,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use Weekday::*;

    /// Mardi 29 septembre 2026, 10h00.
    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 9, 29).unwrap().and_hms_opt(10, 0, 0).unwrap()
    }

    fn p(s: &str) -> Parsed {
        parse(s, now())
    }

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).unwrap()
    }

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn plain_title_untouched() {
        let r = p("Acheter 3 pommes et le pain");
        assert_eq!(r.title, "Acheter 3 pommes et le pain");
        assert_eq!(r.due, None);
        assert!(r.spans.is_empty());
    }

    #[test]
    fn full_example() {
        let r = p("Appeler Paul demain à 9h #Perso @tel !1");
        assert_eq!(r.title, "Appeler Paul");
        assert_eq!(r.due, Some(Due::at(d(9, 30), t(9, 0))));
        assert_eq!(r.list.as_deref(), Some("Perso"));
        assert_eq!(r.tags, vec!["tel"]);
        assert_eq!(r.priority, Priority::High);
        let kinds: Vec<SpanKind> = r.spans.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, vec![SpanKind::Date, SpanKind::Time, SpanKind::List, SpanKind::Tag, SpanKind::Priority]);
        assert_eq!(&"Appeler Paul demain à 9h #Perso @tel !1"[r.spans[1].range.clone()], "à 9h");
    }

    #[test]
    fn relative_days() {
        assert_eq!(p("x aujourd'hui").due, Some(Due::on(d(9, 29))));
        assert_eq!(p("x Aujourd’hui").due, Some(Due::on(d(9, 29))));
        assert_eq!(p("x tomorrow").due, Some(Due::on(d(9, 30))));
        assert_eq!(p("x après-demain").due, Some(Due::on(d(10, 1))));
        assert_eq!(p("x apres demain").due, Some(Due::on(d(10, 1))));
        assert_eq!(p("x ce soir").due, Some(Due::at(d(9, 29), t(19, 0))));
        assert_eq!(p("x ce soir à 21h").due, Some(Due::at(d(9, 29), t(21, 0))));
    }

    #[test]
    fn weekdays_and_weeks() {
        assert_eq!(p("x lundi").due, Some(Due::on(d(10, 5))));
        assert_eq!(p("x mardi").due, Some(Due::on(d(10, 6))), "le jour même renvoie à la semaine suivante");
        assert_eq!(p("x vendredi prochain").due, Some(Due::on(d(10, 2))));
        assert_eq!(p("x next friday").due, Some(Due::on(d(10, 2))));
        assert_eq!(p("x le jeudi").due, Some(Due::on(d(10, 1))));
        assert_eq!(p("x la semaine prochaine").due, Some(Due::on(d(10, 5))));
        assert_eq!(p("x next month").due, Some(Due::on(d(10, 1))));
        assert_eq!(p("x ce week-end").due, Some(Due::on(d(10, 3))));
    }

    #[test]
    fn offsets() {
        assert_eq!(p("x dans 3 jours").due, Some(Due::on(d(10, 2))));
        assert_eq!(p("x in 2 weeks").due, Some(Due::on(d(10, 13))));
        assert_eq!(p("x dans une semaine").due, Some(Due::on(d(10, 6))));
        assert_eq!(p("x dans 2 mois").due, Some(Due::on(d(11, 29))));
        assert_eq!(p("x dans 30 min").due, Some(Due::at(d(9, 29), t(10, 30))));
        assert_eq!(p("x in 2h").due, Some(Due::at(d(9, 29), t(12, 0))));
        assert_eq!(p("Ranger dans 2 cartons").title, "Ranger dans 2 cartons");
    }

    #[test]
    fn absolute_dates() {
        assert_eq!(p("x 15/10").due, Some(Due::on(d(10, 15))));
        assert_eq!(p("x 15/09").due, Some(Due::on(NaiveDate::from_ymd_opt(2027, 9, 15).unwrap())));
        assert_eq!(p("x 2026-12-01").due, Some(Due::on(d(12, 1))));
        assert_eq!(p("x 1/2/27").due, Some(Due::on(NaiveDate::from_ymd_opt(2027, 2, 1).unwrap())));
        assert_eq!(p("x le 15 octobre").due, Some(Due::on(d(10, 15))));
        assert_eq!(p("x 1er déc.").due, Some(Due::on(d(12, 1))));
        assert_eq!(p("x oct 3").due, Some(Due::on(d(10, 3))));
        assert_eq!(p("x 3 mars 2028").due, Some(Due::on(NaiveDate::from_ymd_opt(2028, 3, 3).unwrap())));
        assert_eq!(p("x le 5").due, Some(Due::on(d(10, 5))));
        assert_eq!(p("x 31/02").due, None);
    }

    #[test]
    fn month_names_need_a_day() {
        let r = p("Documentaire sur mars");
        assert_eq!(r.title, "Documentaire sur mars");
        assert_eq!(r.due, None);
    }

    #[test]
    fn times() {
        assert_eq!(p("x demain 14h30").due, Some(Due::at(d(9, 30), t(14, 30))));
        assert_eq!(p("x demain 14:30").due, Some(Due::at(d(9, 30), t(14, 30))));
        assert_eq!(p("x tomorrow at 9pm").due, Some(Due::at(d(9, 30), t(21, 0))));
        assert_eq!(p("x tomorrow 9 am").due, Some(Due::at(d(9, 30), t(9, 0))));
        assert_eq!(p("x demain midi").due, Some(Due::at(d(9, 30), t(12, 0))));
        assert_eq!(p("x 18h").due, Some(Due::at(d(9, 29), t(18, 0))), "heure à venir : aujourd'hui");
        assert_eq!(p("x 8h").due, Some(Due::at(d(9, 30), t(8, 0))), "heure passée : demain");
        assert_eq!(p("x 25h").due, None);
    }

    #[test]
    fn recurrences() {
        let r = p("Sortir les poubelles tous les lundis et jeudis à 20h");
        assert_eq!(r.title, "Sortir les poubelles");
        assert_eq!(r.recurrence, Some(Recurrence::weekly_on(1, [Mon, Thu])));
        assert_eq!(r.due, Some(Due::at(d(10, 1), t(20, 0))));

        let r = p("Arroser tous les 3 jours");
        assert_eq!(r.recurrence, Some(Recurrence::new(Freq::Daily, 3)));
        assert_eq!(r.due, Some(Due::on(d(9, 29))));

        assert_eq!(p("x chaque semaine").recurrence, Some(Recurrence::new(Freq::Weekly, 1)));
        assert_eq!(p("x every other week").recurrence, Some(Recurrence::new(Freq::Weekly, 2)));
        assert_eq!(p("x toutes les deux semaines").recurrence, Some(Recurrence::new(Freq::Weekly, 2)));
        assert_eq!(p("x en semaine").recurrence, Some(Recurrence::weekdays_only()));
        assert_eq!(p("x tous les jours ouvrés").recurrence, Some(Recurrence::weekdays_only()));
        assert_eq!(
            p("x every monday, wednesday and friday").recurrence,
            Some(Recurrence::weekly_on(1, [Mon, Wed, Fri]))
        );
        assert_eq!(p("x tous les week-ends").recurrence, Some(Recurrence::weekly_on(1, [Sat, Sun])));
    }

    #[test]
    fn recurrence_skips_past_time_today() {
        let r = p("Méditer tous les jours à 8h");
        assert_eq!(r.due, Some(Due::at(d(9, 30), t(8, 0))));
        let r = p("Méditer tous les jours à 18h");
        assert_eq!(r.due, Some(Due::at(d(9, 29), t(18, 0))));
    }

    #[test]
    fn monthly_recurrence_is_anchored() {
        let r = p("Loyer tous les mois le 31/10");
        assert_eq!(r.due, Some(Due::on(d(10, 31))));
        assert_eq!(r.recurrence.unwrap().month_day, Some(31));
    }

    #[test]
    fn symbols_need_a_letter() {
        let r = p("Corriger #123 pour a@b.fr");
        assert_eq!(r.title, "Corriger #123 pour a@b.fr");
        assert_eq!(r.list, None);
        assert!(r.tags.is_empty());
    }

    #[test]
    fn priorities() {
        assert_eq!(p("x p2").priority, Priority::Medium);
        assert_eq!(p("x !3").priority, Priority::Low);
        assert_eq!(p("x !9").priority, Priority::None);
        assert_eq!(p("x !9").title, "x !9");
    }

    #[test]
    fn only_first_date_is_taken() {
        let r = p("Préparer lundi la réunion de mardi");
        assert_eq!(r.due, Some(Due::on(d(10, 5))));
        assert_eq!(r.title, "Préparer la réunion de mardi");
    }

    #[test]
    fn connectors_stay_when_nothing_follows() {
        assert_eq!(p("Sortir le chien").title, "Sortir le chien");
        assert_eq!(p("Buy a gift").title, "Buy a gift");
    }

    #[test]
    fn list_name_matching() {
        assert!(list_matches("Courses maison", "courses-maison"));
        assert!(list_matches("Été", "ete"));
        assert!(!list_matches("Travail", "perso"));
    }

    #[test]
    fn into_new_task_sets_reminder() {
        let task = p("x demain 9h").into_new_task();
        assert_eq!(task.remind_at, Some(d(9, 30).and_time(t(9, 0))));
        assert_eq!(p("x demain").into_new_task().remind_at, None);
    }
}
