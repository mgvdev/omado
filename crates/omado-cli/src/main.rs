//! `omado` : ajouter et consulter ses tâches depuis le terminal.
//! Sans argument, ouvre l'application graphique.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use omado_core::human::due_label;
use omado_core::{Completion, INBOX_ID, List, ListId, Store, Task, View, now, parse};

#[derive(Parser)]
#[command(name = "omado", version, about = "Omado : vos tâches, dans Omarchy")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Ajoute une tâche. Saisie rapide : « Appeler Paul demain 9h #perso @tel !1 »
    Add {
        #[arg(required = true, trailing_var_arg = true)]
        text: Vec<String>,
        /// Liste de destination (sinon `#liste` dans le texte, ou la boîte de réception)
        #[arg(short, long)]
        list: Option<String>,
    },
    /// Affiche une vue : today (défaut), upcoming, all, done, inbox, #liste ou @étiquette
    Ls {
        view: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Cherche dans les titres et les notes
    Search {
        #[arg(required = true, trailing_var_arg = true)]
        query: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Coche une tâche (début d'identifiant suffisant)
    Done { id: String },
    /// Décoche une tâche
    Undo { id: String },
    /// Supprime une tâche
    Rm { id: String },
    /// Affiche les listes
    Lists {
        #[arg(long)]
        json: bool,
    },
    /// Montre comment une saisie serait comprise, sans rien créer
    Parse {
        #[arg(required = true, trailing_var_arg = true)]
        text: Vec<String>,
    },
    /// Ouvre la fenêtre de saisie rapide
    Quick,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let Some(cmd) = cli.cmd else { return launch_gui(&[]) };
    if let Cmd::Quick = cmd {
        return launch_gui(&["--quick"]);
    }
    if let Cmd::Parse { text } = &cmd {
        print_parse(&text.join(" "));
        return Ok(());
    }

    let store = Store::open_default().context("ouverture de la base Omado")?;
    let out = Printer::new(&store)?;
    match cmd {
        Cmd::Add { text, list } => {
            let default_list = match list {
                Some(name) => Some(find_list(&store, &name)?.id),
                None => None,
            };
            let task = store.add_quick(&text.join(" "), default_list, now())?;
            print!("Ajoutée : ");
            // Recréé : `#liste` a pu créer une nouvelle liste.
            Printer::new(&store)?.task(&task, "");
        }
        Cmd::Ls { view, json } => {
            let view = resolve_view(&store, view.as_deref().unwrap_or("today"))?;
            out.tasks(&store.tasks(&view, now())?, json, matches!(view, View::List(_)))?;
        }
        Cmd::Search { query, json } => {
            out.tasks(&store.tasks(&View::Search(query.join(" ")), now())?, json, false)?;
        }
        Cmd::Done { id } => {
            let task = store.resolve_task(&id)?;
            match store.complete(task.id, now())? {
                Completion::Completed => println!("Terminée : {}", task.title),
                Completion::Rescheduled(due) => {
                    println!("{} → prochaine fois : {}", task.title, due_label(&due, now().date()))
                }
            }
        }
        Cmd::Undo { id } => {
            let task = store.resolve_task(&id)?;
            store.uncomplete(task.id)?;
            println!("Rouverte : {}", task.title);
        }
        Cmd::Rm { id } => {
            let task = store.resolve_task(&id)?;
            store.delete_task(task.id)?;
            println!("Supprimée : {}", task.title);
        }
        Cmd::Lists { json } => {
            let lists = store.lists()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&lists)?);
            } else {
                let counts = store.counts(now())?;
                for l in lists {
                    let n = counts.per_list.get(&l.id).copied().unwrap_or(0);
                    println!("{:>4}  {}", n, l.name);
                }
            }
        }
        Cmd::Parse { .. } | Cmd::Quick => unreachable!("traités plus haut"),
    }
    Ok(())
}

/// Lance `omado-gtk`, installé à côté de ce binaire ou dans le PATH.
fn launch_gui(args: &[&str]) -> Result<()> {
    let sibling = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("omado-gtk")));
    let program = sibling.filter(|p| p.exists()).unwrap_or_else(|| PathBuf::from("omado-gtk"));
    let err = Command::new(&program).args(args).exec();
    Err(err).with_context(|| format!("impossible de lancer {}", program.display()))
}

fn find_list(store: &Store, name: &str) -> Result<List> {
    store.find_list(name.trim_start_matches('#'))?.with_context(|| format!("liste introuvable : {name}"))
}

fn resolve_view(store: &Store, name: &str) -> Result<View> {
    Ok(match name {
        "today" | "auj" | "aujourdhui" => View::Today,
        "upcoming" | "planifie" => View::Upcoming,
        "all" | "tout" => View::All,
        "done" | "termine" => View::Completed,
        "inbox" => View::List(INBOX_ID),
        _ if name.starts_with('@') => View::Tag(name[1..].to_string()),
        _ if name.starts_with('#') => View::List(find_list(store, name)?.id),
        _ => bail!("vue inconnue : {name} (today, upcoming, all, done, inbox, #liste, @étiquette)"),
    })
}

fn print_parse(input: &str) {
    let p = parse(input, now());
    let today = now().date();
    println!("Titre       : {}", p.title);
    if let Some(due) = &p.due {
        println!("Échéance    : {}", due_label(due, today));
    }
    if let Some(r) = &p.recurrence {
        println!("Récurrence  : {} ({r})", r.describe());
    }
    if let Some(l) = &p.list {
        println!("Liste       : {l}");
    }
    if !p.tags.is_empty() {
        println!("Étiquettes  : {}", p.tags.join(", "));
    }
    if let Some(n) = p.priority.level() {
        println!("Priorité    : !{n}");
    }
}

/// Affichage des tâches, en couleur quand la sortie est un terminal
/// (les couleurs du terminal suivent déjà le thème Omarchy).
struct Printer {
    lists: HashMap<ListId, String>,
    color: bool,
}

impl Printer {
    fn new(store: &Store) -> Result<Self> {
        let lists = store.lists()?.into_iter().map(|l| (l.id, l.name)).collect();
        Ok(Printer { lists, color: std::io::stdout().is_terminal() })
    }

    fn paint(&self, code: &str, s: &str) -> String {
        if self.color { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
    }

    fn tasks(&self, tasks: &[Task], json: bool, nest: bool) -> Result<()> {
        if json {
            println!("{}", serde_json::to_string_pretty(tasks)?);
            return Ok(());
        }
        if tasks.is_empty() {
            println!("{}", self.paint("2", "Rien ici."));
            return Ok(());
        }
        if !nest {
            tasks.iter().for_each(|t| self.task(t, ""));
            return Ok(());
        }
        // Vue de liste : les sous-tâches sous leur parent.
        let present: Vec<_> = tasks.iter().map(|t| t.id).collect();
        for t in tasks.iter().filter(|t| t.parent_id.is_none_or(|p| !present.contains(&p))) {
            self.task(t, "");
            for child in tasks.iter().filter(|c| c.parent_id == Some(t.id)) {
                self.task(child, "    ");
            }
        }
        Ok(())
    }

    fn task(&self, t: &Task, indent: &str) {
        let now = now();
        let check = if t.is_completed() { self.paint("32", "✓") } else { "○".into() };
        let short = self.paint("2", &t.id.to_string()[..8]);
        let mut meta = Vec::new();
        if let Some(due) = &t.due {
            let label = due_label(due, now.date());
            meta.push(if due.is_overdue(now) && !t.is_completed() {
                self.paint("31", &label)
            } else {
                self.paint("36", &label)
            });
        }
        if t.recurrence.is_some() {
            meta.push("↻".into());
        }
        if t.list_id != INBOX_ID
            && let Some(name) = self.lists.get(&t.list_id)
        {
            meta.push(self.paint("34", &format!("#{name}")));
        }
        meta.extend(t.tags.iter().map(|tag| self.paint("35", &format!("@{tag}"))));
        if let Some(n) = t.priority.level() {
            meta.push(self.paint(if n == 1 { "1;31" } else { "33" }, &format!("!{n}")));
        }
        let title = if t.is_completed() { self.paint("2;9", &t.title) } else { t.title.clone() };
        if meta.is_empty() {
            println!("{indent}{check} {short}  {title}");
        } else {
            println!("{indent}{check} {short}  {title}  {}", meta.join(" "));
        }
    }
}
