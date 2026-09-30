//! Saisie rapide façon Todoist : `Appeler Paul demain 9h #perso @tel !1`.
//!
//! Le texte est découpé en mots ; les expressions reconnues (dates, heures,
//! récurrences, `#liste`, `@étiquette`, `!priorité`) sont retirées du titre et
//! renvoyées avec leur position, pour que l'interface puisse les surligner.
//!
//! Les mots viennent de `lexicon` (anglais, français, russe et la langue de
//! l'interface) ; le chinois et le japonais, écrits sans espaces, passent par
//! `cjk`, qui découpe le texte autour des expressions qu'il reconnaît.

mod cjk;
mod lexicon;

use std::ops::Range;

use chrono::{Datelike, Days, Duration, Months, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Weekday};

use crate::model::{Due, NewTask, Priority};
use crate::recurrence::{Freq, Recurrence};
use lexicon::{Lexicon, Unit, W};

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
    let toks = tokenize(input)
        .into_iter()
        .flat_map(|t| if cjk::has_cjk(t.raw) { cjk::split(t, now) } else { vec![t] })
        .collect();
    Parser { input, toks, now, today: now.date(), lex: Lexicon::for_language(crate::i18n::language()) }.run()
}

/// Minuscules sans accents, pour comparer des mots saisis à la main.
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' | 'ą' => 'a',
            'é' | 'è' | 'ê' | 'ë' | 'ę' => 'e',
            'î' | 'ï' | 'í' | 'ì' => 'i',
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ç' | 'ć' => 'c',
            'ñ' | 'ń' => 'n',
            'ł' => 'l',
            'ś' => 's',
            'ź' | 'ż' => 'z',
            'ё' => 'е',
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
    /// Déjà reconnu (morceau de chinois ou de japonais).
    hit: Option<Hit>,
}

fn tokenize(input: &str) -> Vec<Token<'_>> {
    let mut toks = Vec::new();
    let mut start = None;
    let mut push = |s: usize, e: usize| {
        let raw = &input[s..e];
        let norm = fold(raw.trim_end_matches([',', ';', '.', '!', '?']));
        toks.push(Token { raw, norm, range: s..e, hit: None });
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

#[derive(Debug, Clone)]
enum Hit {
    Date {
        date: NaiveDate,
        default_time: Option<NaiveTime>,
    },
    DateTime(NaiveDateTime),
    Time(NaiveTime),
    /// Moment de la journée (« demain soir ») : l'heure par défaut d'une date qui précède.
    DayPart(NaiveTime),
    Recurrence(Recurrence),
    List(String),
    Tag(String),
    Priority(Priority),
}

impl Hit {
    fn kind(&self) -> SpanKind {
        match self {
            Hit::Date { .. } | Hit::DateTime(_) => SpanKind::Date,
            Hit::Time(_) | Hit::DayPart(_) => SpanKind::Time,
            Hit::Recurrence(_) => SpanKind::Recurrence,
            Hit::List(_) => SpanKind::List,
            Hit::Tag(_) => SpanKind::Tag,
            Hit::Priority(_) => SpanKind::Priority,
        }
    }
}

struct Parser<'a> {
    input: &'a str,
    toks: Vec<Token<'a>>,
    now: NaiveDateTime,
    today: NaiveDate,
    lex: &'static Lexicon,
}

impl Parser<'_> {
    fn w(&self, i: usize) -> Option<&str> {
        self.toks.get(i).map(|t| t.norm.as_str())
    }

    /// Sens reconnus à partir du mot `i`, locutions les plus longues d'abord.
    fn lex(&self, i: usize) -> Vec<(W, usize)> {
        let words: Vec<&str> = self.toks.iter().skip(i).take(6).map(|t| t.norm.as_str()).collect();
        self.lex.at(&words).collect()
    }

    fn find(&self, i: usize, pred: impl Fn(W) -> bool) -> Option<(W, usize)> {
        self.lex(i).into_iter().find(|(w, _)| pred(*w))
    }

    fn is(&self, i: usize, meaning: W) -> Option<usize> {
        self.find(i, |w| w == meaning).map(|(_, len)| len)
    }

    fn run(self) -> Parsed {
        let mut out = Parsed::default();
        let mut date = None;
        let mut default_time = None;
        let mut exact = None;
        let mut time = None;
        let mut day_part = None;
        let mut priority_set = false;
        // Mots gardés dans le titre, avec leur place dans le texte.
        let mut title: Vec<(&str, Range<usize>)> = Vec::new();
        // Fin de la dernière date reconnue, pour « demain soir ».
        let mut date_end = None;

        let mut i = 0;
        while i < self.toks.len() {
            let Some((len, hit)) = self.hit_at(i) else {
                title.push((self.toks[i].raw, self.toks[i].range.clone()));
                i += 1;
                continue;
            };
            let kind = hit.kind();
            let accepted = match hit {
                Hit::Date { date: d, default_time: t } if date.is_none() && exact.is_none() => {
                    date = Some(d);
                    default_time = t;
                    date_end = Some(i + len);
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
                Hit::DayPart(t) if day_part.is_none() && date_end == Some(i) => {
                    day_part = Some(t);
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
                title.push((self.toks[i].raw, self.toks[i].range.clone()));
                i += 1;
            }
        }

        let time = time.or(day_part);
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
        out.title = join_title(self.input, &title);
        out
    }

    fn hit_at(&self, i: usize) -> Option<(usize, Hit)> {
        if let Some(hit) = &self.toks[i].hit {
            return Some((1, hit.clone()));
        }
        if let Some(hit) = self.symbol(i) {
            return Some((1, hit));
        }
        if let Some(found) = self.recurrence(i).or_else(|| self.date(i, false)).or_else(|| self.time(i)) {
            return Some(found);
        }
        for (meaning, len) in self.lex(i) {
            let found = match meaning {
                // « à 9h », « le 15 mai », « am Montag », « в пятницу »
                W::Connector => self.date(i + len, true).or_else(|| self.time(i + len)),
                // « at 9 », « um 9 », « a las 9 » : une heure sans unité
                W::At => self.clock(i + len, true).map(|(l, t)| (l, Hit::Time(t))),
                // « le 15 », « el día 15 », « the 5th »
                W::DayOfMonth | W::DayOfMonthOrdinal => self
                    .w(i + len)
                    .and_then(|w| parse_day(w, meaning == W::DayOfMonthOrdinal))
                    .and_then(|day| next_month_day(self.today, day))
                    .map(|date| (1, Hit::Date { date, default_time: None })),
                W::Morning => Some((0, Hit::DayPart(at(9, 0)))),
                W::Afternoon => Some((0, Hit::DayPart(at(15, 0)))),
                W::Evening => Some((0, Hit::DayPart(at(19, 0)))),
                _ => None,
            };
            if let Some((l, hit)) = found {
                return Some((len + l, hit));
            }
        }
        // « 15 числа »
        let day = self.w(i).and_then(|w| parse_day(w, false))?;
        let after = self.is(i + 1, W::DayOfMonthAfter)?;
        next_month_day(self.today, day).map(|date| (1 + after, Hit::Date { date, default_time: None }))
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

    /// Une date. `announced` : un mot comme « le », « na », « am » précède, ce
    /// qui autorise les noms de jour ambigus (portugais « segunda »).
    fn date(&self, i: usize, announced: bool) -> Option<(usize, Hit)> {
        let day = |len: usize, date: NaiveDate| Some((len, Hit::Date { date, default_time: None }));
        for (meaning, len) in self.lex(i) {
            match meaning {
                W::Today => return day(len, self.today),
                W::Tomorrow => return day(len, self.today + Days::new(1)),
                W::AfterTomorrow => return day(len, self.today + Days::new(2)),
                W::Tonight => return Some((len, Hit::Date { date: self.today, default_time: Some(at(19, 0)) })),
                W::ThisWeekend => return day(len, weekend(self.today)),
                W::NextWeek => return day(len, next_weekday(self.today, Weekday::Mon)),
                W::NextMonth => return day(len, first_of_next_month(self.today)),
                W::Next => {
                    if let Some((wd, l)) = self.weekday(i + len, true) {
                        return day(len + l, next_weekday(self.today, wd));
                    }
                }
                W::In => {
                    if let Some((l, hit)) = self.offset(i + len) {
                        return Some((len + l, hit));
                    }
                }
                _ => {}
            }
        }
        if let Some((wd, len)) = self.weekday(i, announced) {
            let len = len + self.is(i + len, W::NextAfter).unwrap_or(0);
            return day(len, next_weekday(self.today, wd));
        }
        if let Some(d) = self.numeric_date(i) {
            return day(1, d);
        }
        self.calendar_date(i).map(|(len, d)| (len, Hit::Date { date: d, default_time: None }))
    }

    /// « 15 octobre [2027] », « 15 de octubre de 2027 », « 15. Oktober », « october 15 ».
    fn calendar_date(&self, i: usize) -> Option<(usize, NaiveDate)> {
        let month_at = |j: usize| match self.find(j, |w| matches!(w, W::Month(_))) {
            Some((W::Month(m), len)) => Some((m, len)),
            _ => None,
        };
        let of_at = |j: usize| self.is(j, W::Of).unwrap_or(0);
        let (day, month, mut len) = match self.w(i).and_then(|w| parse_day(w, false)) {
            Some(d) => {
                let skip = of_at(i + 1);
                let (m, l) = month_at(i + 1 + skip)?;
                (d, m, 1 + skip + l)
            }
            None => {
                let (m, l) = month_at(i)?;
                (self.w(i + l).and_then(|w| parse_day(w, false))?, m, l + 1)
            }
        };
        let skip = of_at(i + len);
        match self.w(i + len + skip).and_then(parse_year) {
            Some(y) => {
                len += skip + 1;
                NaiveDate::from_ymd_opt(y, month, day).map(|d| (len, d))
            }
            None => upcoming(self.today, month, day).map(|d| (len, d)),
        }
    }

    /// Nom de jour ; les formes ambiguës seulement si `announced`.
    fn weekday(&self, i: usize, announced: bool) -> Option<(Weekday, usize)> {
        self.lex(i).into_iter().find_map(|(meaning, len)| match meaning {
            W::Weekday(wd) => Some((wd, len)),
            W::WeakWeekday(wd) if announced => Some((wd, len)),
            _ => None,
        })
    }

    /// Un nombre : chiffres, ou « trois », « drei », « три »…
    fn count(&self, i: usize) -> Option<(u32, usize)> {
        let w = self.w(i)?;
        if is_digits(w, 1..=3) {
            return w.parse().ok().filter(|n| *n >= 1).map(|n| (n, 1));
        }
        match self.find(i, |m| matches!(m, W::Number(_)))? {
            (W::Number(n), len) => Some((n, len)),
            _ => None,
        }
    }

    fn unit(&self, i: usize) -> Option<(Unit, usize)> {
        match self.find(i, |m| matches!(m, W::Unit(_)))? {
            (W::Unit(u), len) => Some((u, len)),
            _ => None,
        }
    }

    /// Ce qui suit « dans » : « 3 jours », « une semaine », « 2h », « неделю » (une).
    fn offset(&self, i: usize) -> Option<(usize, Hit)> {
        if let Some((n, len)) = self.count(i)
            && let Some((unit, l)) = self.unit(i + len)
        {
            return self.apply_offset(n, unit).map(|hit| (len + l, hit));
        }
        if let Some((unit, len)) = self.unit(i) {
            return self.apply_offset(1, unit).map(|hit| (len, hit));
        }
        // Collé : « 2h », « 30min », « 3d ».
        let a = self.w(i)?;
        let split = a.find(|c: char| !c.is_ascii_digit()).filter(|&p| p > 0)?;
        let n = a[..split].parse().ok()?;
        let unit = match self.lex.at(&[&a[split..]]).find(|(m, _)| matches!(m, W::Unit(_)))? {
            (W::Unit(u), _) => u,
            _ => return None,
        };
        self.apply_offset(n, unit).map(|hit| (1, hit))
    }

    fn apply_offset(&self, n: u32, unit: Unit) -> Option<Hit> {
        offset_from(self.now, n, unit)
    }

    fn time(&self, i: usize) -> Option<(usize, Hit)> {
        match self.find(i, |w| matches!(w, W::Noon | W::Midnight)) {
            Some((W::Noon, len)) => return Some((len, Hit::Time(at(12, 0)))),
            Some((_, len)) => return Some((len, Hit::Time(at(0, 0)))),
            None => {}
        }
        self.clock(i, false).map(|(len, t)| (len, Hit::Time(t)))
    }

    /// Une heure : « 9h », « 14h30 », « 9am », « 14:30 », « 9 Uhr », « 5 de la tarde ».
    /// Un nombre seul n'en est une qu'annoncé (`after_at` : « at 9 », « um 9 ») ;
    /// sans matin ni soir, 1 h à 6 h s'entend alors l'après-midi (« um 3 » : 15 h).
    fn clock(&self, i: usize, after_at: bool) -> Option<(usize, NaiveTime)> {
        let a = self.w(i)?;
        let valid = |h: u32, m: u32| NaiveTime::from_hms_opt(h, m, 0);
        // « 9h », « 14h30 »
        if let Some((h, m)) = a.split_once('h') {
            if is_digits(h, 1..=2) && (m.is_empty() || is_digits(m, 2..=2)) {
                return valid(h.parse().ok()?, m.parse().unwrap_or(0)).map(|t| (1, t));
            }
            return None;
        }
        // « 9am », « 9:30pm »
        for suffix in ["am", "pm"] {
            if let Some(clock) = a.strip_suffix(suffix) {
                let (h, m) = clock_parts(clock)?;
                return valid(to_24h(h, suffix)?, m).map(|t| (1, t));
            }
        }
        let (mut h, mut m) = clock_parts(a)?;
        let (mut len, mut explicit, mut marked, mut hour_word) = (1, a.contains(':'), false, false);
        loop {
            match self.find(i + len, |w| matches!(w, W::HourWord | W::Am | W::Pm | W::Half)) {
                Some((W::HourWord, l)) if !hour_word => {
                    (explicit, hour_word, len) = (true, true, len + l);
                }
                Some((W::Am, l)) if !marked => {
                    h = to_24h(h, "am")?;
                    (marked, len) = (true, len + l);
                }
                Some((W::Pm, l)) if !marked => {
                    h = to_24h(h, "pm")?;
                    (marked, len) = (true, len + l);
                }
                Some((W::Half, l)) if m == 0 => {
                    (m, explicit, len) = (30, true, len + l);
                }
                _ => break,
            }
        }
        if !(explicit || marked || after_at) {
            return None;
        }
        if after_at && !marked && !a.contains(':') && (1..=6).contains(&h) {
            h += 12;
        }
        valid(h, m).map(|t| (len, t))
    }

    fn recurrence(&self, i: usize) -> Option<(usize, Hit)> {
        let rec = |len: usize, r: Recurrence| Some((len, Hit::Recurrence(r)));
        for (meaning, len) in self.lex(i) {
            match meaning {
                W::Daily => return rec(len, Recurrence::new(Freq::Daily, 1)),
                W::Weekly => return rec(len, Recurrence::new(Freq::Weekly, 1)),
                W::Monthly => return rec(len, Recurrence::new(Freq::Monthly, 1)),
                W::Yearly => return rec(len, Recurrence::new(Freq::Yearly, 1)),
                W::WeekdaysOnly => return rec(len, Recurrence::weekdays_only()),
                W::Weekends => return rec(len, Recurrence::weekly_on(1, [Weekday::Sat, Weekday::Sun])),
                W::Habitual(wd) => {
                    let (l, days) = self.weekday_list(i + len, wd);
                    return rec(len + l, Recurrence::weekly_on(1, days));
                }
                W::Every => {
                    if let Some((l, r)) = self.recurrence_body(i + len) {
                        return rec(len + l, r);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Ce qui suit « every », « tous les », « jeden », « cada »… : `[n] unité` ou des jours.
    fn recurrence_body(&self, j: usize) -> Option<(usize, Recurrence)> {
        if let Some(found) = self.recurrence_unit(j, 1) {
            return Some(found);
        }
        let (n, len) = match self.is(j, W::Other) {
            Some(len) => (2, len),
            None => self.count(j)?,
        };
        self.recurrence_unit(j + len, n).map(|(l, r)| (len + l, r))
    }

    fn recurrence_unit(&self, j: usize, n: u32) -> Option<(usize, Recurrence)> {
        for (meaning, len) in self.lex(j) {
            let r = match meaning {
                W::Unit(Unit::Day) => Recurrence::new(Freq::Daily, n),
                W::Unit(Unit::Week) => Recurrence::new(Freq::Weekly, n),
                W::Unit(Unit::Month) => Recurrence::new(Freq::Monthly, n),
                W::Unit(Unit::Year) => Recurrence::new(Freq::Yearly, n),
                W::WeekdayUnit => Recurrence::weekdays_only(),
                W::Weekend => Recurrence::weekly_on(n, [Weekday::Sat, Weekday::Sun]),
                W::Weekday(wd) | W::WeakWeekday(wd) | W::Habitual(wd) => {
                    let (l, days) = self.weekday_list(j + len, wd);
                    return Some((len + l, Recurrence::weekly_on(n, days)));
                }
                _ => continue,
            };
            return Some((len, r));
        }
        None
    }

    /// Suite d'une liste de jours après `first` : « , mercredi et vendredi »,
    /// « und donnerstags », « и четвергам », « e às quintas ».
    fn weekday_list(&self, mut k: usize, first: Weekday) -> (usize, Vec<Weekday>) {
        let start = k;
        let mut days = vec![first];
        let item = |j: usize| {
            self.lex(j).into_iter().find_map(|(m, l)| match m {
                W::Weekday(wd) | W::WeakWeekday(wd) | W::Habitual(wd) => Some((wd, l)),
                _ => None,
            })
        };
        loop {
            let after_comma = k > 0 && self.toks.get(k - 1).is_some_and(|t| t.raw.ends_with(','));
            let joiner = self.is(k, W::And).unwrap_or(0);
            if joiner == 0 && !after_comma {
                break;
            }
            match item(k + joiner) {
                Some((wd, l)) => {
                    days.push(wd);
                    k += joiner + l;
                }
                None => break,
            }
        }
        (k - start, days)
    }

    /// `15/10`, `15/10/2026`, `15/10/26`, `2026-10-15`, `15.10.`, `15.10.2026`.
    fn numeric_date(&self, i: usize) -> Option<NaiveDate> {
        let s = self.w(i)?;
        if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            return Some(d);
        }
        // Avec des points, il faut l'année ou le point final (« 15.10 » peut être une heure).
        let raw = self.toks[i].raw.trim_end_matches([',', ';']);
        let dotted = raw.split('.').collect::<Vec<_>>();
        let parts: Vec<&str> = match dotted.as_slice() {
            [d, m, ""] => vec![d, m],
            [_, _, y] if !y.is_empty() => dotted.clone(),
            _ => s.split('/').collect(),
        };
        match parts.as_slice() {
            [d, m] if is_digits(d, 1..=2) && is_digits(m, 1..=2) => {
                upcoming(self.today, m.parse().ok()?, d.parse().ok()?)
            }
            [d, m, y] if is_digits(d, 1..=2) && is_digits(m, 1..=2) && (is_digits(y, 2..=2) || is_digits(y, 4..=4)) => {
                let y: i32 = y.parse().ok()?;
                let y = if y < 100 { 2000 + y } else { y };
                NaiveDate::from_ymd_opt(y, m.parse().ok()?, d.parse().ok()?)
            }
            _ => None,
        }
    }
}

/// Titre à partir des mots restants, séparés par une espace, sauf s'ils se
/// touchaient dans la saisie : « 买牛奶明天和面包 » donne « 买牛奶和面包 ».
fn join_title(input: &str, words: &[(&str, Range<usize>)]) -> String {
    let mut out = String::new();
    for (k, (word, range)) in words.iter().enumerate() {
        if k > 0 && input[words[k - 1].1.end..range.start].chars().any(char::is_whitespace) {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Dans `n` unités à partir de `now` : une date, ou une heure pour les heures et minutes.
fn offset_from(now: NaiveDateTime, n: u32, unit: Unit) -> Option<Hit> {
    let today = now.date();
    let date = |d: NaiveDate| Some(Hit::Date { date: d, default_time: None });
    let later = |delta: Duration| {
        let dt = now + delta;
        Some(Hit::DateTime(dt.with_second(0)?.with_nanosecond(0)?))
    };
    match unit {
        Unit::Day => date(today + Days::new(n.into())),
        Unit::Week => date(today + Days::new(7 * u64::from(n))),
        Unit::Month => date(today.checked_add_months(Months::new(n))?),
        Unit::Year => date(today.checked_add_months(Months::new(12 * n))?),
        Unit::Hour => later(Duration::hours(n.into())),
        Unit::Minute => later(Duration::minutes(n.into())),
    }
}

fn at(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).expect("heure valide")
}

/// Prochain `wd`, strictement après aujourd'hui.
fn next_weekday(today: NaiveDate, wd: Weekday) -> NaiveDate {
    (1..=7).map(|i| today + Days::new(i)).find(|d| d.weekday() == wd).expect("jour trouvé sur 7 jours")
}

fn weekend(today: NaiveDate) -> NaiveDate {
    match today.weekday() {
        Weekday::Sat | Weekday::Sun => today,
        _ => next_weekday(today, Weekday::Sat),
    }
}

fn first_of_next_month(today: NaiveDate) -> NaiveDate {
    today.with_day(1).expect("le 1er existe") + Months::new(1)
}

/// Prochain jour `day` du mois (ce mois-ci s'il n'est pas passé).
fn next_month_day(today: NaiveDate, day: u32) -> Option<NaiveDate> {
    let first = today.with_day(1)?;
    (0..12).filter_map(|m| (first + Months::new(m)).with_day(day)).find(|d| *d >= today)
}

/// Jour/mois sans année : cette année, ou la suivante si la date est passée.
fn upcoming(today: NaiveDate, month: u32, day: u32) -> Option<NaiveDate> {
    match NaiveDate::from_ymd_opt(today.year(), month, day) {
        Some(d) if d >= today => Some(d),
        _ => (1..=4).find_map(|y| NaiveDate::from_ymd_opt(today.year() + y, month, day)),
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
        // Déjà sur 24 h : « 20 Uhr abends ».
        (13..=23, "pm") => Some(h),
        _ => None,
    }
}

/// Numéro de jour : « 15 », « 1er », « 5th », « 15-го » ; `ordinal` exige le suffixe.
fn parse_day(s: &str, ordinal: bool) -> Option<u32> {
    let stripped = ["er", "st", "nd", "rd", "th", "-го", "-е"].iter().find_map(|suf| s.strip_suffix(suf));
    let digits = match stripped {
        Some(d) => d,
        None if ordinal => return None,
        None => s,
    };
    if !is_digits(digits, 1..=2) {
        return None;
    }
    digits.parse().ok().filter(|d| (1..=31).contains(d))
}

fn parse_year(s: &str) -> Option<i32> {
    is_digits(s, 4..=4).then(|| s.parse().ok())?
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

    // --- Autres langues ------------------------------------------------------

    /// Saisie dans la langue d'interface `lang`.
    fn pl(lang: &str, s: &str) -> Parsed {
        crate::i18n::with_language(lang, || p(s))
    }

    #[track_caller]
    fn due(lang: &str, s: &str) -> Option<Due> {
        pl(lang, s).due
    }

    #[track_caller]
    fn untouched(lang: &str, s: &str) {
        let r = pl(lang, s);
        assert_eq!((r.title.as_str(), r.due, r.recurrence), (s, None, None), "{lang} : « {s} »");
    }

    #[test]
    fn english_more() {
        let r = p("Pay rent every month on the 5th");
        assert_eq!(r.title, "Pay rent");
        assert_eq!(r.due, Some(Due::on(d(10, 5))));
        assert_eq!(r.recurrence.unwrap().month_day, Some(5));
        assert_eq!(p("Call Paul at 3").due, Some(Due::at(d(9, 29), t(15, 0))), "« at 3 » : l'après-midi");
        assert_eq!(p("Call Paul at 9").due, Some(Due::at(d(9, 30), t(9, 0))));
        assert_eq!(p("Gym tomorrow morning").due, Some(Due::at(d(9, 30), t(9, 0))));
        assert_eq!(p("x in a week").due, Some(Due::on(d(10, 6))));
        assert_eq!(p("x the 15th").due, Some(Due::on(d(10, 15))));
        assert_eq!(p("Gym mondays and thursdays").recurrence, Some(Recurrence::weekly_on(1, [Mon, Thu])));
        untouched("en", "Read the 3 books");
        untouched("en", "Plan movie night");
    }

    #[test]
    fn german() {
        let r = pl("de", "Paul anrufen morgen um 9 #Privat @telefon !1");
        assert_eq!(r.title, "Paul anrufen");
        assert_eq!(r.due, Some(Due::at(d(9, 30), t(9, 0))));
        assert_eq!((r.list.as_deref(), r.tags, r.priority), (Some("Privat"), vec!["telefon".into()], Priority::High));
        let r = pl("de", "Müll rausbringen jeden Montag und Donnerstag um 20 Uhr");
        assert_eq!(r.title, "Müll rausbringen");
        assert_eq!(r.recurrence, Some(Recurrence::weekly_on(1, [Mon, Thu])));
        assert_eq!(r.due, Some(Due::at(d(10, 1), t(20, 0))));
        assert_eq!(due("de", "x übermorgen"), Some(Due::on(d(10, 1))));
        assert_eq!(due("de", "x in 3 Tagen"), Some(Due::on(d(10, 2))));
        assert_eq!(due("de", "x nächsten Freitag"), Some(Due::on(d(10, 2))));
        assert_eq!(due("de", "x am 15. Oktober"), Some(Due::on(d(10, 15))));
        assert_eq!(due("de", "x 15.10."), Some(Due::on(d(10, 15))));
        assert_eq!(due("de", "x heute Abend"), Some(Due::at(d(9, 29), t(19, 0))));
        assert_eq!(due("de", "x morgen früh"), Some(Due::at(d(9, 30), t(9, 0))));
        assert_eq!(due("de", "x um 3"), Some(Due::at(d(9, 29), t(15, 0))));
        assert_eq!(pl("de", "x alle 2 Wochen").recurrence, Some(Recurrence::new(Freq::Weekly, 2)));
        assert_eq!(pl("de", "x montags und freitags").recurrence, Some(Recurrence::weekly_on(1, [Mon, Fri])));
        assert_eq!(pl("de", "x werktags").recurrence, Some(Recurrence::weekdays_only()));
        untouched("de", "Alle Mails beantworten");
    }

    #[test]
    fn spanish() {
        let r = pl("es", "Llamar a Paul mañana a las 9 #personal");
        assert_eq!(r.title, "Llamar a Paul");
        assert_eq!(r.due, Some(Due::at(d(9, 30), t(9, 0))));
        let r = pl("es", "Sacar la basura todos los lunes y jueves a las 8 de la noche");
        assert_eq!(r.title, "Sacar la basura");
        assert_eq!(r.recurrence, Some(Recurrence::weekly_on(1, [Mon, Thu])));
        assert_eq!(r.due, Some(Due::at(d(10, 1), t(20, 0))));
        assert_eq!(due("es", "x pasado mañana"), Some(Due::on(d(10, 1))));
        assert_eq!(due("es", "x en 3 días"), Some(Due::on(d(10, 2))));
        assert_eq!(due("es", "x el próximo viernes"), Some(Due::on(d(10, 2))));
        assert_eq!(due("es", "x el 15 de octubre"), Some(Due::on(d(10, 15))));
        assert_eq!(due("es", "x el lunes"), Some(Due::on(d(10, 5))));
        assert_eq!(due("es", "x mañana por la mañana"), Some(Due::at(d(9, 30), t(9, 0))));
        assert_eq!(due("es", "x a las 5 y media de la tarde"), Some(Due::at(d(9, 29), t(17, 30))));
        assert_eq!(due("es", "x manana"), Some(Due::on(d(9, 30))), "sans tilde");
        assert_eq!(pl("es", "x los lunes").recurrence, Some(Recurrence::weekly_on(1, [Mon])));
        assert_eq!(pl("es", "x cada 2 semanas").recurrence, Some(Recurrence::new(Freq::Weekly, 2)));
        untouched("es", "Comprar 2 o 3 manzanas");
    }

    #[test]
    fn italian() {
        let r = pl("it", "Chiamare Paul domani alle 9");
        assert_eq!(r.title, "Chiamare Paul");
        assert_eq!(r.due, Some(Due::at(d(9, 30), t(9, 0))));
        assert_eq!(due("it", "x dopodomani"), Some(Due::on(d(10, 1))));
        assert_eq!(due("it", "x tra 3 giorni"), Some(Due::on(d(10, 2))));
        assert_eq!(due("it", "x venerdì prossimo"), Some(Due::on(d(10, 2))));
        assert_eq!(due("it", "x il 15 ottobre"), Some(Due::on(d(10, 15))));
        assert_eq!(due("it", "x domani sera"), Some(Due::at(d(9, 30), t(19, 0))));
        assert_eq!(pl("it", "x ogni lunedì e giovedì").recurrence, Some(Recurrence::weekly_on(1, [Mon, Thu])));
        assert_eq!(pl("it", "x ogni 2 settimane").recurrence, Some(Recurrence::new(Freq::Weekly, 2)));
        untouched("it", "Leggere tra le righe");
    }

    #[test]
    fn portuguese() {
        let r = pl("pt_BR", "Ligar para o Paulo amanhã às 9");
        assert_eq!(r.title, "Ligar para o Paulo");
        assert_eq!(r.due, Some(Due::at(d(9, 30), t(9, 0))));
        assert_eq!(due("pt_BR", "x depois de amanhã"), Some(Due::on(d(10, 1))));
        assert_eq!(due("pt_BR", "x daqui a 3 dias"), Some(Due::on(d(10, 2))));
        assert_eq!(due("pt_BR", "x na sexta"), Some(Due::on(d(10, 2))));
        assert_eq!(due("pt_BR", "x 15 de outubro"), Some(Due::on(d(10, 15))));
        assert_eq!(pl("pt_BR", "x às segundas e quintas").recurrence, Some(Recurrence::weekly_on(1, [Mon, Thu])));
        assert_eq!(pl("pt_BR", "x toda segunda").recurrence, Some(Recurrence::weekly_on(1, [Mon])));
        assert_eq!(pl("pt_BR", "x a cada 2 semanas").recurrence, Some(Recurrence::new(Freq::Weekly, 2)));
        untouched("pt_BR", "Ligar pela segunda vez");
    }

    #[test]
    fn russian() {
        let r = pl("ru", "Позвонить Павлу завтра в 9");
        assert_eq!(r.title, "Позвонить Павлу");
        assert_eq!(r.due, Some(Due::at(d(9, 30), t(9, 0))));
        assert_eq!(due("ru", "x послезавтра"), Some(Due::on(d(10, 1))));
        assert_eq!(due("ru", "x через 3 дня"), Some(Due::on(d(10, 2))));
        assert_eq!(due("ru", "x через неделю"), Some(Due::on(d(10, 6))));
        assert_eq!(due("ru", "x в пятницу"), Some(Due::on(d(10, 2))));
        assert_eq!(due("ru", "x 15 октября"), Some(Due::on(d(10, 15))));
        assert_eq!(due("ru", "x в 9 вечера"), Some(Due::at(d(9, 29), t(21, 0))));
        assert_eq!(pl("ru", "x по понедельникам и четвергам").recurrence, Some(Recurrence::weekly_on(1, [Mon, Thu])));
        assert_eq!(pl("ru", "x каждые 2 недели").recurrence, Some(Recurrence::new(Freq::Weekly, 2)));
        // Le russe est actif quelle que soit la langue de l'interface.
        assert_eq!(due("en", "x завтра"), Some(Due::on(d(9, 30))));
    }

    #[test]
    fn polish() {
        let r = pl("pl", "Zadzwonić do Pawła jutro o 9");
        assert_eq!(r.title, "Zadzwonić do Pawła");
        assert_eq!(r.due, Some(Due::at(d(9, 30), t(9, 0))));
        assert_eq!(due("pl", "x pojutrze"), Some(Due::on(d(10, 1))));
        assert_eq!(due("pl", "x za 3 dni"), Some(Due::on(d(10, 2))));
        assert_eq!(due("pl", "x w piątek"), Some(Due::on(d(10, 2))));
        assert_eq!(due("pl", "x 15 października"), Some(Due::on(d(10, 15))));
        assert_eq!(due("pl", "x jutro wieczorem"), Some(Due::at(d(9, 30), t(19, 0))));
        assert_eq!(pl("pl", "x w poniedziałki i czwartki").recurrence, Some(Recurrence::weekly_on(1, [Mon, Thu])));
        assert_eq!(pl("pl", "x co 2 tygodnie").recurrence, Some(Recurrence::new(Freq::Weekly, 2)));
    }

    #[test]
    fn chinese() {
        let r = p("明天下午3点开会");
        assert_eq!(r.title, "开会");
        assert_eq!(r.due, Some(Due::at(d(9, 30), t(15, 0))));
        let r = p("每周一和周四倒垃圾");
        assert_eq!(r.title, "倒垃圾");
        assert_eq!(r.recurrence, Some(Recurrence::weekly_on(1, [Mon, Thu])));
        assert_eq!(p("3天后交报告").due, Some(Due::on(d(10, 2))));
        assert_eq!(p("10月15日订火车票").due, Some(Due::on(d(10, 15))));
        assert_eq!(p("下周五交报告").due, Some(Due::on(d(10, 9))));
        assert_eq!(p("周五交报告").due, Some(Due::on(d(10, 2))));
        assert_eq!(p("每3天浇水").recurrence, Some(Recurrence::new(Freq::Daily, 3)));
        assert_eq!(p("买牛奶明天和面包").title, "买牛奶和面包");
        let r = p("给妈妈打电话 #家庭 @电话 !1");
        assert_eq!((r.title.as_str(), r.list.as_deref(), r.priority), ("给妈妈打电话", Some("家庭"), Priority::High));
        untouched("zh_CN", "喝一点水");
    }

    #[test]
    fn japanese() {
        let r = p("明日の9時に会議");
        assert_eq!(r.title, "会議");
        assert_eq!(r.due, Some(Due::at(d(9, 30), t(9, 0))));
        let r = p("毎週月曜日と木曜日にゴミ出し");
        assert_eq!(r.title, "ゴミ出し");
        assert_eq!(r.recurrence, Some(Recurrence::weekly_on(1, [Mon, Thu])));
        assert_eq!(p("3日後に提出").due, Some(Due::on(d(10, 2))));
        assert_eq!(p("来週の金曜日").due, Some(Due::on(d(10, 9))));
        assert_eq!(p("午後3時に電話").due, Some(Due::at(d(9, 29), t(15, 0))));
        assert_eq!(p("明日の朝に散歩").due, Some(Due::at(d(9, 30), t(9, 0))));
        untouched("ja", "朝ごはん");
        untouched("ja", "一時停止");
    }

    /// Les langues à alphabet latin ne s'activent qu'en langue d'interface.
    #[test]
    fn latin_languages_follow_interface() {
        untouched("en", "Comprar el lunes");
        assert_eq!(due("es", "Comprar el lunes"), Some(Due::on(d(10, 5))));
        untouched("es", "Kaufen 2 o 9 Äpfel");
        assert_eq!(due("pl", "x o 9"), Some(Due::at(d(9, 30), t(9, 0))));
    }

    /// Textes entre guillemets, quels qu'ils soient : “…”, « … », „…“, 「…」.
    fn quoted(s: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut rest = s;
        while let Some(open) = rest.find(['“', '«', '„', '「']) {
            let after = &rest[open + rest[open..].chars().next().map_or(1, char::len_utf8)..];
            let Some(close) = after.find(['”', '»', '“', '」']) else { break };
            out.push(after[..close].trim());
            rest = &after[close..][after[close..].chars().next().map_or(1, char::len_utf8)..];
        }
        out
    }

    /// Les exemples montrés par l'interface, une fois traduits, sont compris dans leur langue.
    #[test]
    fn translated_examples_parse() {
        use crate::tr;
        for code in crate::i18n::available().chain(["en"]) {
            crate::i18n::with_language(code, || {
                let r = p(tr!("Call Paul tomorrow 9am #personal @phone !1"));
                assert_eq!(r.due, Some(Due::at(d(9, 30), t(9, 0))), "{code} : exemple de saisie");
                assert!(r.list.is_some() && !r.tags.is_empty() && r.priority == Priority::High, "{code} : {r:?}");
                let mut examples: Vec<&str> = tr!("tomorrow 9am, friday, every monday…")
                    .trim_end_matches(['…', ' '])
                    .split([',', '、', '，'])
                    .map(str::trim)
                    .collect();
                for text in [
                    tr!("New task… e.g. “Call Paul tomorrow 9am #personal @phone !1”  (n)"),
                    tr!("Add a task, in quick entry syntax: “Call Paul tomorrow 9am #personal @phone !1”"),
                    tr!("Didn't get that. Try “tomorrow 9am”, “Oct 15” or “every monday”."),
                    tr!("Add a date: “friday 2pm”, “Oct 15”…"),
                ] {
                    let found = quoted(text);
                    assert!(!found.is_empty(), "{code} : pas d'exemple entre guillemets dans « {text} »");
                    examples.extend(found);
                }
                for example in examples {
                    let r = p(example);
                    assert!(r.due.is_some() || r.recurrence.is_some(), "{code} : « {example} » n'est pas compris");
                }
            });
        }
    }
}
