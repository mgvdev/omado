//! Libellés lisibles pour l'interface et la CLI.

use chrono::{Datelike, NaiveDate, NaiveTime, Timelike, Weekday};

use crate::model::Due;

const MONTHS: [&str; 12] =
    ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."];

pub fn weekday_name(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "Lundi",
        Weekday::Tue => "Mardi",
        Weekday::Wed => "Mercredi",
        Weekday::Thu => "Jeudi",
        Weekday::Fri => "Vendredi",
        Weekday::Sat => "Samedi",
        Weekday::Sun => "Dimanche",
    }
}

/// « Aujourd'hui », « Demain », « Jeudi », « 15 oct. », « 3 mars 2028 ».
pub fn date_label(date: NaiveDate, today: NaiveDate) -> String {
    match (date - today).num_days() {
        0 => "Aujourd'hui".into(),
        1 => "Demain".into(),
        -1 => "Hier".into(),
        2..=6 => weekday_name(date.weekday()).into(),
        _ if date.year() == today.year() => format!("{} {}", date.day(), MONTHS[date.month0() as usize]),
        _ => format!("{} {} {}", date.day(), MONTHS[date.month0() as usize], date.year()),
    }
}

/// « 9h », « 14h30 ».
pub fn time_label(time: NaiveTime) -> String {
    match time.minute() {
        0 => format!("{}h", time.hour()),
        m => format!("{}h{m:02}", time.hour()),
    }
}

pub fn due_label(due: &Due, today: NaiveDate) -> String {
    match due.time {
        Some(t) => format!("{} {}", date_label(due.date, today), time_label(t)),
        None => date_label(due.date, today),
    }
}

/// En-tête de groupe dans la vue « Planifié » : « Jeudi 1 oct. ».
pub fn day_heading(date: NaiveDate, today: NaiveDate) -> String {
    let base = format!("{} {}", date.day(), MONTHS[date.month0() as usize]);
    let base = if date.year() == today.year() { base } else { format!("{base} {}", date.year()) };
    match (date - today).num_days() {
        0 => format!("Aujourd'hui · {base}"),
        1 => format!("Demain · {base}"),
        _ => format!("{} {base}", weekday_name(date.weekday())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn labels() {
        let today = d(2026, 9, 29);
        assert_eq!(date_label(today, today), "Aujourd'hui");
        assert_eq!(date_label(d(2026, 10, 1), today), "Jeudi");
        assert_eq!(date_label(d(2026, 10, 15), today), "15 oct.");
        assert_eq!(date_label(d(2027, 2, 3), today), "3 févr. 2027");
        let due = Due::at(d(2026, 9, 30), NaiveTime::from_hms_opt(14, 5, 0).unwrap());
        assert_eq!(due_label(&due, today), "Demain 14h05");
        assert_eq!(time_label(NaiveTime::from_hms_opt(9, 0, 0).unwrap()), "9h");
        assert_eq!(day_heading(d(2026, 10, 1), today), "Jeudi 1 oct.");
    }
}
