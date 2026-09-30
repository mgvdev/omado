//! Libellés lisibles pour l'interface et la CLI, dans la langue de l'utilisateur.
//!
//! Les traductions donnent la forme de milieu de phrase (`*_inline` :
//! « aujourd'hui », « jeudi ») ; les autres fonctions l'emploient en tête de
//! libellé, avec une majuscule.

use chrono::{Datelike, NaiveDate, NaiveTime, Timelike, Weekday};

use crate::i18n::capitalize;
use crate::model::Due;
use crate::trc;

/// Nom du jour, en milieu de phrase : « jeudi », « Thursday ».
pub fn weekday_name(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => trc!("weekday", "Monday"),
        Weekday::Tue => trc!("weekday", "Tuesday"),
        Weekday::Wed => trc!("weekday", "Wednesday"),
        Weekday::Thu => trc!("weekday", "Thursday"),
        Weekday::Fri => trc!("weekday", "Friday"),
        Weekday::Sat => trc!("weekday", "Saturday"),
        Weekday::Sun => trc!("weekday", "Sunday"),
    }
}

fn month_abbr(month0: u32) -> &'static str {
    match month0 {
        0 => trc!("abbreviated month", "Jan"),
        1 => trc!("abbreviated month", "Feb"),
        2 => trc!("abbreviated month", "Mar"),
        3 => trc!("abbreviated month", "Apr"),
        4 => trc!("abbreviated month", "May"),
        5 => trc!("abbreviated month", "Jun"),
        6 => trc!("abbreviated month", "Jul"),
        7 => trc!("abbreviated month", "Aug"),
        8 => trc!("abbreviated month", "Sep"),
        9 => trc!("abbreviated month", "Oct"),
        10 => trc!("abbreviated month", "Nov"),
        _ => trc!("abbreviated month", "Dec"),
    }
}

/// « 15 oct. », « 3 févr. 2028 ».
fn calendar_date(date: NaiveDate, today: NaiveDate) -> String {
    let (month, day) = (month_abbr(date.month0()), date.day());
    if date.year() == today.year() {
        // Translators: a date in the current year. {month} is an abbreviated month name from above.
        trc!("date", "{month} {day}", month = month, day = day)
    } else {
        trc!("date", "{month} {day}, {year}", month = month, day = day, year = date.year())
    }
}

/// « aujourd'hui », « demain », « jeudi », « 15 oct. ».
pub fn date_label_inline(date: NaiveDate, today: NaiveDate) -> String {
    match (date - today).num_days() {
        0 => trc!("date", "today").into(),
        1 => trc!("date", "tomorrow").into(),
        -1 => trc!("date", "yesterday").into(),
        2..=6 => weekday_name(date.weekday()).into(),
        _ => calendar_date(date, today),
    }
}

/// « Aujourd'hui », « Demain », « Jeudi », « 15 oct. », « 3 mars 2028 ».
pub fn date_label(date: NaiveDate, today: NaiveDate) -> String {
    capitalize(&date_label_inline(date, today))
}

/// « 9h », « 14h30 » ; « 9 AM », « 2:30 PM » en anglais.
pub fn time_label(time: NaiveTime) -> String {
    let h24 = time.hour();
    let h12 = match h24 % 12 {
        0 => 12,
        h => h,
    };
    let ampm = if h24 < 12 { trc!("time", "AM") } else { trc!("time", "PM") };
    let mm = format!("{:02}", time.minute());
    if time.minute() == 0 {
        // Translators: a time on the hour. Use any of {h24} (0–23), {h12} (1–12), {mm} (“00”) and {ampm}.
        // xgettext:no-rust-format
        trc!("time on the hour", "{h12} {ampm}", h24 = h24, h12 = h12, mm = mm, ampm = ampm)
    } else {
        // Translators: a time with minutes. Use any of {h24} (0–23), {h12} (1–12), {mm} (00–59) and {ampm}.
        // xgettext:no-rust-format
        trc!("time with minutes", "{h12}:{mm} {ampm}", h24 = h24, h12 = h12, mm = mm, ampm = ampm)
    }
}

fn datetime_inline(date: NaiveDate, time: NaiveTime, today: NaiveDate) -> String {
    // Translators: a due date with its time, e.g. “tomorrow, 9 AM”. {date} starts lowercase where your language allows.
    trc!("date and time", "{date}, {time}", date = date_label_inline(date, today), time = time_label(time))
}

/// « Demain 9h », « Tomorrow, 9 AM ».
pub fn datetime_label(date: NaiveDate, time: NaiveTime, today: NaiveDate) -> String {
    capitalize(&datetime_inline(date, time, today))
}

pub fn due_label(due: &Due, today: NaiveDate) -> String {
    match due.time {
        Some(t) => datetime_label(due.date, t, today),
        None => date_label(due.date, today),
    }
}

/// En-tête de groupe dans la vue « Planifié » : « Jeudi 1 oct. », « Demain · 1 oct. ».
pub fn day_heading(date: NaiveDate, today: NaiveDate) -> String {
    let date_text = calendar_date(date, today);
    let day = match (date - today).num_days() {
        0 => trc!("date", "today"),
        1 => trc!("date", "tomorrow"),
        _ => {
            let weekday = weekday_name(date.weekday());
            return capitalize(&trc!("day heading", "{weekday}, {date}", weekday = weekday, date = date_text));
        }
    };
    capitalize(&trc!("day heading", "{day} · {date}", day = day, date = date_text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{available, with_language};

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn labels_en() {
        let today = d(2026, 9, 29);
        assert_eq!(date_label(today, today), "Today");
        assert_eq!(date_label(d(2026, 10, 1), today), "Thursday");
        assert_eq!(date_label(d(2026, 10, 15), today), "Oct 15");
        assert_eq!(date_label(d(2027, 2, 3), today), "Feb 3, 2027");
        assert_eq!(due_label(&Due::at(d(2026, 9, 30), t(14, 5)), today), "Tomorrow, 2:05 PM");
        assert_eq!(time_label(t(9, 0)), "9 AM");
        assert_eq!(time_label(t(0, 30)), "12:30 AM");
        assert_eq!(time_label(t(12, 0)), "12 PM");
        assert_eq!(day_heading(d(2026, 10, 1), today), "Thursday, Oct 1");
        assert_eq!(day_heading(d(2026, 9, 30), today), "Tomorrow · Sep 30");
    }

    #[test]
    fn labels_fr() {
        with_language("fr", || {
            let today = d(2026, 9, 29);
            assert_eq!(date_label(today, today), "Aujourd'hui");
            assert_eq!(date_label_inline(today, today), "aujourd'hui");
            assert_eq!(date_label(d(2026, 10, 1), today), "Jeudi");
            assert_eq!(date_label(d(2026, 10, 15), today), "15 oct.");
            assert_eq!(date_label(d(2027, 2, 3), today), "3 févr. 2027");
            assert_eq!(due_label(&Due::at(d(2026, 9, 30), t(14, 5)), today), "Demain 14h05");
            assert_eq!(time_label(t(9, 0)), "9h");
            assert_eq!(day_heading(d(2026, 10, 1), today), "Jeudi 1 oct.");
        });
    }

    /// Chaque langue remplit bien ses modèles : aucun `{…}` ne reste.
    #[test]
    fn labels_complete_in_every_language() {
        let today = d(2026, 9, 29);
        for code in available() {
            with_language(code, || {
                let samples = [
                    date_label(d(2026, 10, 15), today),
                    date_label(d(2027, 2, 3), today),
                    due_label(&Due::at(today, t(9, 0)), today),
                    due_label(&Due::at(d(2026, 10, 2), t(14, 30)), today),
                    day_heading(d(2026, 10, 1), today),
                    day_heading(today, today),
                ];
                for s in samples {
                    assert!(!s.contains(['{', '}']) && !s.is_empty(), "{code} : « {s} »");
                }
            });
        }
    }
}
