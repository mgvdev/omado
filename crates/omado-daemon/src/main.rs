//! `omado-daemon` : envoie les rappels, même quand la fenêtre d'Omado est fermée.
//!
//! Lancé comme service utilisateur systemd. La base est relue régulièrement,
//! ce qui suffit à voir les tâches ajoutées par l'interface ou la CLI.
//! Les notifications proposent « Terminé » et « Reporter » ; un clic ouvre l'app.

use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{Duration as Delta, NaiveDateTime};
use notify_rust::{Hint, Notification, Timeout, Urgency};
use omado_core::human::due_label;
use omado_core::{INBOX_ID, Priority, Store, Task, now};

/// Identifiant de l'app : nom du fichier .desktop et de l'icône.
const APP_ID: &str = "dev.omado.Omado";
/// Intervalle maximal entre deux relectures de la base.
const POLL: Duration = Duration::from_secs(15);
/// Au-delà, les rappels manqués (machine éteinte) sont regroupés.
const MAX_INDIVIDUAL: usize = 5;
const SNOOZE_SHORT: i64 = 10;
const SNOOZE_LONG: i64 = 60;

fn main() -> Result<()> {
    let once = std::env::args().any(|a| a == "--once");
    let store = Store::open_default().context("ouverture de la base Omado")?;
    eprintln!("omado-daemon : surveillance des rappels");
    loop {
        if let Err(e) = fire_due(&store) {
            eprintln!("omado-daemon : {e:#}");
        }
        if once {
            // Laisse le temps au serveur de notifications de recevoir les envois.
            thread::sleep(Duration::from_millis(300));
            return Ok(());
        }
        thread::sleep(time_until_next(&store).unwrap_or(POLL));
    }
}

fn time_until_next(store: &Store) -> Result<Duration> {
    let now = now();
    Ok(match store.next_reminder_after(now)? {
        Some(at) => (at - now).to_std().unwrap_or_default().min(POLL),
        None => POLL,
    })
}

fn fire_due(store: &Store) -> Result<()> {
    let now = now();
    let due = store.due_reminders(now)?;
    if due.is_empty() {
        return Ok(());
    }
    let lists = store.lists()?;
    let list_name = |t: &Task| lists.iter().find(|l| l.id == t.list_id && l.id != INBOX_ID).map(|l| l.name.clone());

    let (individual, grouped) = due.split_at(due.len().min(MAX_INDIVIDUAL));
    for task in individual {
        notify_task(task, list_name(task), now)?;
        store.mark_reminded(task.id, now)?;
    }
    if !grouped.is_empty() {
        let titles: Vec<&str> = grouped.iter().map(|t| t.title.as_str()).collect();
        Notification::new()
            .appname("Omado")
            .hint(Hint::DesktopEntry(APP_ID.into()))
            .icon(APP_ID)
            .summary(&format!("{} autres rappels", grouped.len()))
            .body(&titles.join("\n"))
            .show()?;
        for task in grouped {
            store.mark_reminded(task.id, now)?;
        }
    }
    Ok(())
}

fn notify_task(task: &Task, list: Option<String>, fired_at: NaiveDateTime) -> Result<()> {
    let mut body = Vec::new();
    if let Some(due) = &task.due {
        body.push(due_label(due, fired_at.date()));
    }
    if let Some(list) = list {
        body.push(list);
    }
    let handle = Notification::new()
        .appname("Omado")
        .hint(Hint::DesktopEntry(APP_ID.into()))
        .icon(APP_ID)
        .summary(&task.title)
        .body(&body.join(" · "))
        .urgency(if task.priority == Priority::High { Urgency::Critical } else { Urgency::Normal })
        .timeout(Timeout::Never)
        .action("default", "Ouvrir")
        .action("done", "Terminé")
        .action("snooze", &format!("+{SNOOZE_SHORT} min"))
        .action("snooze-long", "+1 h")
        .show()?;

    // Chaque notification attend sa réponse dans son propre fil, avec sa connexion à la base.
    let id = task.id;
    thread::spawn(move || {
        handle.wait_for_action(|action| {
            let result = Store::open_default().and_then(|store| match action {
                "done" => store.complete(id, now()).map(drop),
                "snooze" => store.snooze(id, now() + Delta::minutes(SNOOZE_SHORT)),
                "snooze-long" => store.snooze(id, now() + Delta::minutes(SNOOZE_LONG)),
                "default" => {
                    if let Err(e) = Command::new("omado-gtk").spawn() {
                        eprintln!("omado-daemon : impossible d'ouvrir Omado : {e}");
                    }
                    Ok(())
                }
                _ => Ok(()),
            });
            if let Err(e) = result {
                eprintln!("omado-daemon : action « {action} » : {e}");
            }
        });
    });
    Ok(())
}
