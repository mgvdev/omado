//! Récurrences, stockées au format RRULE (RFC 5545).
//!
//! Seul le sous-ensemble utile à une app de tâches est géré : FREQ, INTERVAL,
//! BYDAY (sans préfixe numérique) et BYMONTHDAY. C'est ce que produit la saisie
//! rapide, et ce que CalDAV comprend partout.

use std::fmt;
use std::str::FromStr;

use chrono::{Datelike, Days, Months, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

use crate::i18n::capitalize;
use crate::{tr, trc, trn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Recurrence {
    pub freq: Freq,
    pub interval: u32,
    /// Jours de la semaine (récurrence hebdomadaire), dans l'ordre lundi → dimanche.
    pub weekdays: Vec<Weekday>,
    /// Jour du mois (récurrence mensuelle) ; ramené au dernier jour si le mois est trop court.
    pub month_day: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RRuleError(String);

impl fmt::Display for RRuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&tr!("unsupported recurrence rule: {rule}", rule = self.0))
    }
}

impl std::error::Error for RRuleError {}

impl Recurrence {
    pub fn new(freq: Freq, interval: u32) -> Self {
        Recurrence { freq, interval: interval.max(1), weekdays: Vec::new(), month_day: None }
    }

    pub fn weekly_on(interval: u32, days: impl IntoIterator<Item = Weekday>) -> Self {
        let mut r = Recurrence::new(Freq::Weekly, interval);
        r.weekdays = sorted_days(days);
        r
    }

    pub fn weekdays_only() -> Self {
        use Weekday::*;
        Recurrence::weekly_on(1, [Mon, Tue, Wed, Thu, Fri])
    }

    /// Fixe le jour du mois à partir de la première échéance, pour qu'une
    /// tâche du 31 revienne le 31 (et non le 30 après un mois court).
    pub fn anchored(mut self, first: NaiveDate) -> Self {
        if self.freq == Freq::Monthly && self.month_day.is_none() {
            self.month_day = Some(first.day());
        }
        self
    }

    /// Première occurrence le jour même ou après `date`.
    pub fn first_on_or_after(&self, date: NaiveDate) -> NaiveDate {
        match self.freq {
            Freq::Weekly if !self.weekdays.is_empty() => (0..7)
                .map(|i| date + Days::new(i))
                .find(|d| self.weekdays.contains(&d.weekday()))
                .expect("jour trouvé sur 7 jours"),
            Freq::Monthly => match self.month_day {
                Some(day) => {
                    let this_month = clamp_day(date.year(), date.month(), day);
                    if this_month >= date { this_month } else { self.add_months_anchored(this_month, 1) }
                }
                None => date,
            },
            _ => date,
        }
    }

    /// Occurrence suivante, strictement après `from` (la date d'échéance courante).
    pub fn next_after(&self, from: NaiveDate) -> NaiveDate {
        let n = self.interval.max(1);
        match self.freq {
            Freq::Daily => from + Days::new(n as u64),
            Freq::Weekly if self.weekdays.is_empty() => from + Days::new(7 * n as u64),
            Freq::Weekly => {
                let offset = from.weekday().num_days_from_monday() as u64;
                let rest_of_week = (1..7 - offset).map(|i| from + Days::new(i));
                if let Some(d) = rest_of_week.clone().find(|d| self.weekdays.contains(&d.weekday())) {
                    return d;
                }
                let next_week = from - Days::new(offset) + Days::new(7 * n as u64);
                (0..7)
                    .map(|i| next_week + Days::new(i))
                    .find(|d| self.weekdays.contains(&d.weekday()))
                    .expect("jour trouvé sur 7 jours")
            }
            Freq::Monthly => self.add_months_anchored(from, n),
            Freq::Yearly => self.add_months_anchored(from, 12 * n),
        }
    }

    fn add_months_anchored(&self, from: NaiveDate, months: u32) -> NaiveDate {
        let day = self.month_day.unwrap_or(from.day());
        let first = NaiveDate::from_ymd_opt(from.year(), from.month(), 1).expect("date valide") + Months::new(months);
        clamp_day(first.year(), first.month(), day)
    }

    /// Libellé lisible : « Tous les 3 jours », « Every Monday and Thursday ».
    pub fn describe(&self) -> String {
        capitalize(&self.describe_inline())
    }

    /// Libellé en milieu de phrase : « tous les 3 jours », « every Monday ».
    pub fn describe_inline(&self) -> String {
        let n = self.interval;
        match self.freq {
            Freq::Daily if n == 1 => tr!("every day").into(),
            Freq::Daily => trn!("every {n} day", "every {n} days", n),
            Freq::Weekly if self.weekdays.is_empty() && n == 1 => tr!("every week").into(),
            Freq::Weekly if self.weekdays.is_empty() => trn!("every {n} week", "every {n} weeks", n),
            Freq::Weekly if *self == Recurrence::weekdays_only() => tr!("every weekday").into(),
            Freq::Weekly => {
                let days = join(self.weekdays.iter().map(|d| recurring_weekday(*d)).collect());
                if n == 1 {
                    // Translators: {days} is a list of weekdays from “weekday in a recurrence”, e.g. “Monday and Thursday”.
                    tr!("every {days}", days = days)
                } else {
                    trn!("every {n} week on {days}", "every {n} weeks on {days}", n, days = days)
                }
            }
            Freq::Monthly if n == 1 => tr!("every month").into(),
            Freq::Monthly => trn!("every {n} month", "every {n} months", n),
            Freq::Yearly if n == 1 => tr!("every year").into(),
            Freq::Yearly => trn!("every {n} year", "every {n} years", n),
        }
    }
}

impl fmt::Display for Recurrence {
    /// Représentation RRULE, sans le préfixe `RRULE:`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let freq = match self.freq {
            Freq::Daily => "DAILY",
            Freq::Weekly => "WEEKLY",
            Freq::Monthly => "MONTHLY",
            Freq::Yearly => "YEARLY",
        };
        write!(f, "FREQ={freq}")?;
        if self.interval > 1 {
            write!(f, ";INTERVAL={}", self.interval)?;
        }
        if !self.weekdays.is_empty() {
            let days: Vec<&str> = self.weekdays.iter().map(|d| weekday_code(*d)).collect();
            write!(f, ";BYDAY={}", days.join(","))?;
        }
        if let Some(day) = self.month_day {
            write!(f, ";BYMONTHDAY={day}")?;
        }
        Ok(())
    }
}

impl From<Recurrence> for String {
    fn from(r: Recurrence) -> Self {
        r.to_string()
    }
}

impl TryFrom<String> for Recurrence {
    type Error = RRuleError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl FromStr for Recurrence {
    type Err = RRuleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || RRuleError(s.to_string());
        let body = s.strip_prefix("RRULE:").unwrap_or(s);
        let mut freq = None;
        let mut interval = 1;
        let mut weekdays = Vec::new();
        let mut month_day = None;
        for part in body.split(';').filter(|p| !p.is_empty()) {
            let (key, value) = part.split_once('=').ok_or_else(err)?;
            match key {
                "FREQ" => {
                    freq = Some(match value {
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY" => Freq::Yearly,
                        _ => return Err(err()),
                    })
                }
                "INTERVAL" => interval = value.parse().ok().filter(|n| *n >= 1).ok_or_else(err)?,
                "BYDAY" => {
                    weekdays = value.split(',').map(weekday_from_code).collect::<Option<Vec<_>>>().ok_or_else(err)?;
                }
                "BYMONTHDAY" => month_day = Some(value.parse().ok().filter(|d| (1..=31).contains(d)).ok_or_else(err)?),
                "WKST" => {}
                _ => return Err(err()),
            }
        }
        Ok(Recurrence { freq: freq.ok_or_else(err)?, interval, weekdays: sorted_days(weekdays), month_day })
    }
}

fn sorted_days(days: impl IntoIterator<Item = Weekday>) -> Vec<Weekday> {
    let mut v: Vec<Weekday> = days.into_iter().collect();
    v.sort_by_key(|d| d.num_days_from_monday());
    v.dedup();
    v
}

fn clamp_day(year: i32, month: u32, day: u32) -> NaiveDate {
    (1..=day).rev().find_map(|d| NaiveDate::from_ymd_opt(year, month, d)).expect("le 1er du mois existe")
}

fn weekday_code(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "MO",
        Weekday::Tue => "TU",
        Weekday::Wed => "WE",
        Weekday::Thu => "TH",
        Weekday::Fri => "FR",
        Weekday::Sat => "SA",
        Weekday::Sun => "SU",
    }
}

fn weekday_from_code(code: &str) -> Option<Weekday> {
    Some(match code {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

/// Jour de la semaine tel qu'il s'emploie dans « tous les lundis », « every Monday ».
fn recurring_weekday(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => trc!("weekday in a recurrence", "Monday"),
        Weekday::Tue => trc!("weekday in a recurrence", "Tuesday"),
        Weekday::Wed => trc!("weekday in a recurrence", "Wednesday"),
        Weekday::Thu => trc!("weekday in a recurrence", "Thursday"),
        Weekday::Fri => trc!("weekday in a recurrence", "Friday"),
        Weekday::Sat => trc!("weekday in a recurrence", "Saturday"),
        Weekday::Sun => trc!("weekday in a recurrence", "Sunday"),
    }
}

/// « lundis, mercredis et vendredis ».
fn join(items: Vec<&str>) -> String {
    match items.as_slice() {
        [] => String::new(),
        [one] => one.to_string(),
        [init @ .., last] => {
            // Translators: separator between the first items of a list, e.g. “Monday, Wednesday and Friday”.
            let init = init.join(trc!("list", ", "));
            trc!("list", "{items} and {last}", items = init, last = last)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Weekday::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn rrule_round_trip() {
        for s in
            ["FREQ=DAILY", "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE", "FREQ=MONTHLY;BYMONTHDAY=31", "FREQ=YEARLY;INTERVAL=3"]
        {
            assert_eq!(s.parse::<Recurrence>().unwrap().to_string(), s);
        }
        assert_eq!(
            "RRULE:FREQ=WEEKLY;BYDAY=WE,MO".parse::<Recurrence>().unwrap().to_string(),
            "FREQ=WEEKLY;BYDAY=MO,WE"
        );
    }

    #[test]
    fn rrule_rejects_unsupported() {
        assert!("FREQ=HOURLY".parse::<Recurrence>().is_err());
        assert!("FREQ=MONTHLY;BYDAY=1MO".parse::<Recurrence>().is_err());
        assert!("FREQ=DAILY;COUNT=3".parse::<Recurrence>().is_err());
        assert!("INTERVAL=2".parse::<Recurrence>().is_err());
    }

    #[test]
    fn daily_and_plain_weekly() {
        assert_eq!(Recurrence::new(Freq::Daily, 3).next_after(d(2026, 9, 29)), d(2026, 10, 2));
        assert_eq!(Recurrence::new(Freq::Weekly, 2).next_after(d(2026, 9, 29)), d(2026, 10, 13));
    }

    #[test]
    fn weekly_by_day() {
        // 2026-09-29 est un mardi.
        let r = Recurrence::weekly_on(1, [Mon, Thu]);
        assert_eq!(r.next_after(d(2026, 9, 29)), d(2026, 10, 1));
        assert_eq!(r.next_after(d(2026, 10, 1)), d(2026, 10, 5));
        let every_other = Recurrence::weekly_on(2, [Mon, Thu]);
        assert_eq!(every_other.next_after(d(2026, 10, 1)), d(2026, 10, 12));
        assert_eq!(Recurrence::weekdays_only().next_after(d(2026, 10, 2)), d(2026, 10, 5));
    }

    #[test]
    fn monthly_keeps_anchor_day() {
        let r = Recurrence::new(Freq::Monthly, 1).anchored(d(2026, 1, 31));
        let feb = r.next_after(d(2026, 1, 31));
        assert_eq!(feb, d(2026, 2, 28));
        assert_eq!(r.next_after(feb), d(2026, 3, 31));
    }

    #[test]
    fn yearly_leap_day() {
        let r = Recurrence::new(Freq::Yearly, 1).anchored(d(2028, 2, 29));
        assert_eq!(r.next_after(d(2028, 2, 29)), d(2029, 2, 28));
    }

    #[test]
    fn first_occurrence() {
        assert_eq!(Recurrence::weekly_on(1, [Mon]).first_on_or_after(d(2026, 9, 29)), d(2026, 10, 5));
        assert_eq!(Recurrence::weekly_on(1, [Tue]).first_on_or_after(d(2026, 9, 29)), d(2026, 9, 29));
        let mut monthly = Recurrence::new(Freq::Monthly, 1);
        monthly.month_day = Some(15);
        assert_eq!(monthly.first_on_or_after(d(2026, 9, 29)), d(2026, 10, 15));
    }

    #[test]
    fn describe_en() {
        assert_eq!(Recurrence::new(Freq::Daily, 1).describe(), "Every day");
        assert_eq!(Recurrence::new(Freq::Daily, 3).describe_inline(), "every 3 days");
        assert_eq!(Recurrence::weekdays_only().describe(), "Every weekday");
        assert_eq!(Recurrence::weekly_on(1, [Mon, Wed, Fri]).describe(), "Every Monday, Wednesday and Friday");
        assert_eq!(Recurrence::weekly_on(2, [Mon, Thu]).describe(), "Every 2 weeks on Monday and Thursday");
        assert_eq!(Recurrence::new(Freq::Monthly, 2).describe(), "Every 2 months");
    }

    #[test]
    fn describe_fr() {
        crate::i18n::with_language("fr", || {
            assert_eq!(Recurrence::new(Freq::Daily, 1).describe(), "Tous les jours");
            assert_eq!(Recurrence::new(Freq::Daily, 3).describe_inline(), "tous les 3 jours");
            assert_eq!(Recurrence::weekdays_only().describe(), "En semaine");
            assert_eq!(Recurrence::weekly_on(1, [Mon, Wed, Fri]).describe(), "Tous les lundis, mercredis et vendredis");
            assert_eq!(Recurrence::new(Freq::Monthly, 2).describe(), "Tous les 2 mois");
        });
    }

    /// Chaque langue remplit bien ses modèles : aucun `{…}` ne reste.
    #[test]
    fn describe_complete_in_every_language() {
        let samples = [
            Recurrence::new(Freq::Daily, 1),
            Recurrence::new(Freq::Daily, 2),
            Recurrence::new(Freq::Daily, 5),
            Recurrence::new(Freq::Daily, 21),
            Recurrence::new(Freq::Weekly, 3),
            Recurrence::weekdays_only(),
            Recurrence::weekly_on(1, [Mon, Wed, Fri]),
            Recurrence::weekly_on(2, [Tue, Sun]),
            Recurrence::new(Freq::Monthly, 1),
            Recurrence::new(Freq::Monthly, 6),
            Recurrence::new(Freq::Yearly, 2),
        ];
        for code in crate::i18n::available() {
            crate::i18n::with_language(code, || {
                for r in &samples {
                    let s = r.describe();
                    assert!(!s.contains(['{', '}']) && !s.is_empty(), "{code} : « {s} »");
                }
            });
        }
    }
}
