//! Chinois et japonais, écrits sans espaces : chaque morceau de texte est
//! parcouru pour y repérer dates, heures et récurrences (« 明天下午3点开会 »,
//! « 毎週月曜日と木曜日 », « 3日後 »), qui deviennent des mots reconnus ; le
//! reste garde sa forme et revient dans le titre.
//!
//! Les particules collées à une expression (« 明日の9時に ») partent avec elle.

use chrono::{Datelike, Days, NaiveDate, NaiveDateTime, NaiveTime, Weekday};

use super::lexicon::Unit;
use super::{Hit, Token, at, first_of_next_month, fold, next_month_day, next_weekday, offset_from, upcoming, weekend};
use crate::recurrence::{Freq, Recurrence};

pub fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{30ff}' // hiragana, katakana
        | '\u{3400}'..='\u{4dbf}'
        | '\u{4e00}'..='\u{9fff}'
        | '\u{f900}'..='\u{faff}'
        | '\u{ff66}'..='\u{ff9f}')
}

pub fn has_cjk(s: &str) -> bool {
    s.chars().any(is_cjk)
}

/// Découpe `tok` autour des expressions reconnues. Sans expression, le mot reste entier.
pub(super) fn split(tok: Token<'_>, now: NaiveDateTime) -> Vec<Token<'_>> {
    let text = tok.raw;
    let base = tok.range.start;
    let mut pieces: Vec<(usize, usize, Option<Hit>)> = Vec::new();
    let mut plain_start = 0;
    let mut pos = 0;
    while pos < text.len() {
        match (Scan { s: &text[pos..], now }).expression() {
            Some((mut len, hit)) => {
                // « 在明天… » : la préposition part avec la date.
                let mut start = pos;
                if text[plain_start..pos].ends_with('在') && start - '在'.len_utf8() == plain_start {
                    start -= '在'.len_utf8();
                }
                if plain_start < start {
                    pieces.push((plain_start, start, None));
                }
                len += particles(&text[pos + len..]);
                pieces.push((start, pos + len, Some(hit)));
                pos += len;
                plain_start = pos;
            }
            None => pos += text[pos..].chars().next().map_or(1, char::len_utf8),
        }
    }
    if pieces.is_empty() {
        return vec![tok];
    }
    if plain_start < text.len() {
        pieces.push((plain_start, text.len(), None));
    }
    pieces
        .into_iter()
        .map(|(a, b, hit)| {
            let raw = &text[a..b];
            Token { raw, norm: fold(raw), range: base + a..base + b, hit }
        })
        .collect()
}

/// Longueur des particules qui suivent une expression : « の », « に », « は », « 的 »…
fn particles(s: &str) -> usize {
    ["まで", "の", "に", "は", "的"].iter().find(|p| s.starts_with(**p)).map_or(0, |p| p.len())
}

const ZH_WEEKDAY_PREFIXES: &[&str] = &["个星期", "星期", "礼拜", "周", "週"];
const WEEK: [Weekday; 7] =
    [Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri, Weekday::Sat, Weekday::Sun];

struct Scan<'s> {
    s: &'s str,
    now: NaiveDateTime,
}

/// Position de lecture dans le texte.
#[derive(Clone, Copy)]
struct At<'s> {
    s: &'s str,
    pos: usize,
}

impl<'s> At<'s> {
    fn rest(&self) -> &'s str {
        &self.s[self.pos..]
    }

    fn eat(&mut self, lit: &str) -> bool {
        let ok = self.rest().starts_with(lit);
        if ok {
            self.pos += lit.len();
        }
        ok
    }

    /// Le premier littéral présent, dans l'ordre donné (les plus longs d'abord).
    fn eat_any(&mut self, lits: &[&str]) -> Option<usize> {
        let i = lits.iter().position(|l| self.rest().starts_with(l))?;
        self.pos += lits[i].len();
        Some(i)
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    /// Nombre en chiffres (aussi pleine chasse) ou en caractères : « 15 », « １５ », « 十五 », « 两 ».
    fn number(&mut self) -> Option<u32> {
        let digit = |c: char| match c {
            '0'..='9' => Some(c as u32 - '0' as u32),
            '０'..='９' => Some(c as u32 - '０' as u32),
            _ => None,
        };
        let digits: String = self.rest().chars().map_while(digit).map(|d| char::from_digit(d, 10).unwrap()).collect();
        if !digits.is_empty() {
            self.pos += self.rest().chars().take(digits.len()).map(char::len_utf8).sum::<usize>();
            return digits.parse().ok();
        }
        let kanji = |c: char| match c {
            '〇' | '零' => Some(0),
            '一' => Some(1),
            '二' | '两' => Some(2),
            '三' => Some(3),
            '四' => Some(4),
            '五' => Some(5),
            '六' => Some(6),
            '七' => Some(7),
            '八' => Some(8),
            '九' => Some(9),
            '十' => Some(10),
            _ => None,
        };
        let chars: Vec<char> = self.rest().chars().take_while(|c| kanji(*c).is_some()).collect();
        if chars.is_empty() {
            return None;
        }
        let (mut value, mut current) = (0, 0);
        for c in &chars {
            match kanji(*c) {
                Some(10) => {
                    value += current.max(1) * 10;
                    current = 0;
                }
                d => current = d.unwrap_or(0),
            }
        }
        self.pos += chars.iter().map(|c| c.len_utf8()).sum::<usize>();
        Some(value + current)
    }

    /// Jour de la semaine : « 周一 », « 星期五 », « 礼拜天 », « 月曜日 », « 金曜 ».
    fn weekday(&mut self) -> Option<Weekday> {
        let save = *self;
        if self.eat_any(ZH_WEEKDAY_PREFIXES).is_some() {
            if let Some(i) = self.eat_any(&["一", "二", "三", "四", "五", "六", "日", "天"]) {
                return Some(WEEK[i.min(6)]);
            }
            *self = save;
        }
        if let Some(i) = self.eat_any(&["月", "火", "水", "木", "金", "土", "日"])
            && self.eat("曜")
        {
            self.eat("日");
            return Some(WEEK[i]);
        }
        *self = save;
        None
    }

    /// Suite de jours : « 周一和周四 », « 月曜日と木曜日 », « 月・木 ».
    fn weekday_list(&mut self, first: Weekday) -> Vec<Weekday> {
        let mut days = vec![first];
        loop {
            let save = *self;
            if self.eat_any(&["和", "、", "与", "跟", "及", "と", "・", "，", ","]).is_none() {
                break;
            }
            match self.weekday() {
                Some(d) => days.push(d),
                None => {
                    *self = save;
                    break;
                }
            }
        }
        days
    }
}

impl Scan<'_> {
    fn today(&self) -> NaiveDate {
        self.now.date()
    }

    /// Expression reconnue au début du texte : longueur en octets et sens.
    fn expression(&self) -> Option<(usize, Hit)> {
        let start = At { s: self.s, pos: 0 };
        self.recurrence(start).or_else(|| self.offset(start)).or_else(|| self.date(start)).or_else(|| self.time(start))
    }

    fn recurrence(&self, mut c: At) -> Option<(usize, Hit)> {
        let rec = |c: At, r: Recurrence| Some((c.pos, Hit::Recurrence(r)));
        if c.eat_any(&["每个工作日", "每工作日", "工作日", "毎平日", "平日"]).is_some() {
            return rec(c, Recurrence::weekdays_only());
        }
        if c.eat_any(&["每个周末", "每周末", "毎週末"]).is_some() {
            return rec(c, Recurrence::weekly_on(1, [Weekday::Sat, Weekday::Sun]));
        }
        if c.eat_any(&["隔周", "隔週"]).is_some() {
            return rec(c, Recurrence::new(Freq::Weekly, 2));
        }
        if c.eat_any(&["每", "毎"]).is_some() {
            let mut days = c;
            if let Some(first) = days.weekday() {
                let list = days.weekday_list(first);
                return rec(days, Recurrence::weekly_on(1, list));
            }
            let mut week = c;
            if week.eat_any(&["个星期", "星期", "礼拜", "周", "週"]).is_some() {
                // « 毎週月曜日と木曜日 »
                let mut days = week;
                if let Some(first) = days.weekday() {
                    let list = days.weekday_list(first);
                    return rec(days, Recurrence::weekly_on(1, list));
                }
                return rec(week, Recurrence::new(Freq::Weekly, 1));
            }
            if c.eat_any(&["天", "日"]).is_some() {
                return rec(c, Recurrence::new(Freq::Daily, 1));
            }
            if c.eat_any(&["个月", "月"]).is_some() {
                return rec(c, Recurrence::new(Freq::Monthly, 1));
            }
            if c.eat("年") {
                return rec(c, Recurrence::new(Freq::Yearly, 1));
            }
            let n = c.number()?;
            let freq = unit_freq(&mut c)?;
            return rec(c, Recurrence::new(freq, n));
        }
        // « 3日ごと », « 2週間ごと », « 1日おき » (un jour sur deux)
        let n = c.number()?;
        let freq = unit_freq(&mut c)?;
        match c.eat_any(&["ごと", "毎", "おき"])? {
            2 => rec(c, Recurrence::new(freq, n + 1)),
            _ => rec(c, Recurrence::new(freq, n)),
        }
    }

    /// « 3天后 », « 2周以后 », « 3日後 », « 2時間後 », « 半小时后 ».
    fn offset(&self, mut c: At) -> Option<(usize, Hit)> {
        let (n, unit) = if c.eat_any(&["半个小时", "半小时"]).is_some() {
            (30, Unit::Minute)
        } else {
            let n = c.number()?;
            let i = c.eat_any(&OFFSET_UNITS.map(|(form, _)| form))?;
            (n, OFFSET_UNITS[i].1)
        };
        c.eat_any(&["以后", "之后", "后", "以後", "後"])?;
        offset_from(self.now, n, unit).map(|hit| (c.pos, hit))
    }

    fn date(&self, mut c: At) -> Option<(usize, Hit)> {
        let today = self.today();
        let day = |c: At, date: NaiveDate| Some((c.pos, Hit::Date { date, default_time: None }));
        if c.eat_any(&["今天晚上", "今晚", "今夜", "今晩"]).is_some() {
            return Some((c.pos, Hit::Date { date: today, default_time: Some(at(19, 0)) }));
        }
        if let Some(i) = c.eat_any(&[
            "大后天",
            "后天",
            "明後日",
            "あさって",
            "明天",
            "明日",
            "あした",
            "あす",
            "今天",
            "今日",
            "きょう",
        ]) {
            let offset = [3, 2, 2, 2, 1, 1, 1, 1, 0, 0, 0][i];
            return day(c, today + Days::new(offset));
        }
        let monday = today - Days::new(today.weekday().num_days_from_monday().into());
        if c.eat_any(&["下个周末", "下周末"]).is_some() {
            return day(c, monday + Days::new(12));
        }
        if c.eat_any(&["这个周末", "这周末", "本周末", "今週末", "周末", "週末"]).is_some() {
            return day(c, weekend(today));
        }
        // Semaine prochaine ou cette semaine, avec ou sans jour : « 下周五 », « 来週の月曜日 ».
        let week = c.eat_any(&[
            "下个星期",
            "下星期",
            "下礼拜",
            "下周",
            "下週",
            "来週",
            "这个星期",
            "这星期",
            "这周",
            "本周",
            "今週",
        ]);
        if let Some(w) = week {
            let next = w <= 5;
            c.eat("の");
            // « 下周五 » : le « 周 » est déjà lu, reste le chiffre.
            let bare = c.eat_any(&["一", "二", "三", "四", "五", "六", "日", "天"]).map(|i| WEEK[i.min(6)]);
            let monday = if next { monday + Days::new(7) } else { monday };
            return match bare.or_else(|| c.weekday()) {
                Some(wd) => {
                    let d = monday + Days::new(wd.num_days_from_monday().into());
                    day(c, if d < today { d + Days::new(7) } else { d })
                }
                None if next => day(c, monday),
                None => None,
            };
        }
        if c.eat_any(&["下个月", "下月", "来月"]).is_some() {
            return day(c, first_of_next_month(today));
        }
        if let Some(wd) = c.weekday() {
            return day(c, next_weekday(today, wd));
        }
        // « 2026年10月15日 », « 10月15号 », « 15号 », « 15日 »
        let save = c;
        let first = c.number()?;
        let (year, month, dom) = if c.eat("年") {
            let m = c.number()?;
            c.eat("月").then_some(())?;
            (Some(first as i32), Some(m), c.number()?)
        } else if c.eat("月") {
            (None, Some(first), c.number()?)
        } else {
            c = save;
            (None, None, c.number()?)
        };
        c.eat_any(&["日", "号", "號"])?;
        // « 15日間 », « 3日後 » : une durée, pas une date.
        if c.peek().is_some_and(|ch| "間间後后ごとおき以".contains(ch)) {
            return None;
        }
        let date = match (year, month) {
            (Some(y), Some(m)) => NaiveDate::from_ymd_opt(y, m, dom)?,
            (None, Some(m)) => upcoming(today, m, dom)?,
            _ => next_month_day(today, dom)?,
        };
        day(c, date)
    }

    /// « 下午3点 », « 3点半 », « 15:30 », « 午後3時 », « 9時15分 », « 中午 », « 晚上 ».
    fn time(&self, mut c: At) -> Option<(usize, Hit)> {
        const AM: &[&str] = &["上午", "早上", "早晨", "凌晨", "午前", "朝"];
        const PM: &[&str] = &["下午", "晚上", "傍晚", "夜里", "午後", "夕方", "夜"];
        const NOON: &[&str] = &["中午", "正午", "昼"];
        let marker = if c.eat_any(AM).is_some() {
            Some("am")
        } else if c.eat_any(PM).is_some() {
            Some("pm")
        } else if c.eat_any(NOON).is_some() {
            Some("noon")
        } else {
            None
        };
        let after_marker = c;
        let mut in_kanji = false;
        let clock = (|| {
            in_kanji = c.peek().is_some_and(|ch| !ch.is_ascii_digit() && !('０'..='９').contains(&ch));
            let h = c.number()?;
            let m = if c.eat_any(&[":", "："]).is_some() {
                c.number()?
            } else {
                c.eat_any(&["点", "點", "時", "时"])?;
                if c.eat("半") {
                    in_kanji = false;
                    30
                } else {
                    let save = c;
                    match c.number() {
                        Some(m) if m < 60 => {
                            c.eat_any(&["分钟", "分"]);
                            in_kanji = false;
                            m
                        }
                        _ => {
                            c = save;
                            if c.eat("钟") {
                                in_kanji = false;
                            }
                            0
                        }
                    }
                }
            };
            Some((h, m))
        })();
        let Some((mut h, m)) = clock else {
            // Un moment de la journée seul : « 中午 », « 明天晚上 », « 明日の朝に ».
            let c = after_marker;
            let one_char = c.pos <= '朝'.len_utf8();
            if one_char && c.peek().is_some_and(|ch| is_cjk(ch) && particles(c.rest()) == 0) {
                return None;
            }
            return match marker? {
                "noon" => Some((c.pos, Hit::Time(at(12, 0)))),
                "am" => Some((c.pos, Hit::DayPart(at(9, 0)))),
                _ if c.s.starts_with("下午") || c.s.starts_with("午後") => Some((c.pos, Hit::DayPart(at(15, 0)))),
                _ => Some((c.pos, Hit::DayPart(at(19, 0)))),
            };
        };
        // « 一点 » (un peu), « 一時 » (un moment) : en caractères, il faut un repère de plus.
        if in_kanji && marker.is_none() {
            return None;
        }
        match marker {
            Some("pm") if h < 12 => h += 12,
            Some("noon") if h < 11 => h += 12,
            Some("am") if h == 12 => h = 0,
            // Sans matin ni soir, « 3点 » s'entend l'après-midi (comme « at 3 »).
            None if (1..=6).contains(&h) && !self.s[..c.pos].contains([':', '：']) => h += 12,
            _ => {}
        }
        NaiveTime::from_hms_opt(h, m, 0).map(|t| (c.pos, Hit::Time(t)))
    }
}

/// Unité d'une récurrence : « 天 », « 周 », « 个月 », « 週間 », « か月 », « 年 ».
fn unit_freq(c: &mut At) -> Option<Freq> {
    let i = c.eat_any(&FREQ_UNITS.map(|(form, _)| form))?;
    Some(FREQ_UNITS[i].1)
}

/// Unités d'un délai, les formes longues d'abord (« 分钟 » avant « 分 »).
const OFFSET_UNITS: [(&str, Unit); 18] = [
    ("个小时", Unit::Hour),
    ("小时", Unit::Hour),
    ("時間", Unit::Hour),
    ("分钟", Unit::Minute),
    ("分", Unit::Minute),
    ("天", Unit::Day),
    ("日", Unit::Day),
    ("个星期", Unit::Week),
    ("星期", Unit::Week),
    ("週間", Unit::Week),
    ("周", Unit::Week),
    ("週", Unit::Week),
    ("个月", Unit::Month),
    ("か月", Unit::Month),
    ("ヶ月", Unit::Month),
    ("カ月", Unit::Month),
    ("ケ月", Unit::Month),
    ("年", Unit::Year),
];

const FREQ_UNITS: [(&str, Freq); 13] = [
    ("天", Freq::Daily),
    ("日", Freq::Daily),
    ("个星期", Freq::Weekly),
    ("星期", Freq::Weekly),
    ("週間", Freq::Weekly),
    ("周", Freq::Weekly),
    ("週", Freq::Weekly),
    ("个月", Freq::Monthly),
    ("か月", Freq::Monthly),
    ("ヶ月", Freq::Monthly),
    ("カ月", Freq::Monthly),
    ("月", Freq::Monthly),
    ("年", Freq::Yearly),
];
