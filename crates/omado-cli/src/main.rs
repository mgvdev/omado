//! `omado` : ajouter et consulter ses tâches depuis le terminal.
//! Sans argument, ouvre l'application graphique.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use omado_core::human::due_label;
use omado_core::{Completion, INBOX_ID, List, ListId, Store, Task, View, now, parse, tr};

#[derive(Parser)]
#[command(name = "omado", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    Add {
        #[arg(required = true, trailing_var_arg = true)]
        text: Vec<String>,
        #[arg(short, long)]
        list: Option<String>,
    },
    Ls {
        view: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Search {
        #[arg(required = true, trailing_var_arg = true)]
        query: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    Done {
        id: String,
    },
    Undo {
        id: String,
    },
    Rm {
        id: String,
    },
    Lists {
        #[arg(long)]
        json: bool,
    },
    Parse {
        #[arg(required = true, trailing_var_arg = true)]
        text: Vec<String>,
    },
    Quick,
}

/// Textes d'aide, traduits (les attributs de `clap` ne passent pas par l'extraction des chaînes).
fn command() -> clap::Command {
    Cli::command()
        .about(tr!("Omado: your tasks, in Omarchy"))
        .mut_subcommand("add", |c| {
            // Translators: quick entry understands English and French only: keep the example in English.
            c.about(tr!("Add a task, in quick entry syntax: “Call Paul tomorrow 9am #personal @phone !1”"))
                .mut_arg("list", |a| a.help(tr!("Destination list (otherwise #list in the text, or the inbox)")))
        })
        .mut_subcommand("ls", |c| {
            c.about(tr!("Show a view: today (default), upcoming, all, done, inbox, #list or @tag"))
        })
        .mut_subcommand("search", |c| c.about(tr!("Search titles and notes")))
        .mut_subcommand("done", |c| c.about(tr!("Complete a task (the start of its ID is enough)")))
        .mut_subcommand("undo", |c| c.about(tr!("Reopen a task")))
        .mut_subcommand("rm", |c| c.about(tr!("Delete a task")))
        .mut_subcommand("lists", |c| c.about(tr!("Show the lists")))
        .mut_subcommand("parse", |c| c.about(tr!("Show how an entry would be understood, without creating anything")))
        .mut_subcommand("quick", |c| c.about(tr!("Open the quick entry window")))
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", tr!("omado: {error}", error = format!("{e:#}")));
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::from_arg_matches(&command().get_matches()).unwrap_or_else(|e| e.exit());
    let Some(cmd) = cli.cmd else { return launch_gui(&[]) };
    if let Cmd::Quick = cmd {
        return launch_gui(&["--quick"]);
    }
    if let Cmd::Parse { text } = &cmd {
        print_parse(&text.join(" "));
        return Ok(());
    }

    let store = Store::open_default().context(tr!("opening the Omado database"))?;
    let out = Printer::new(&store)?;
    match cmd {
        Cmd::Add { text, list } => {
            let default_list = match list {
                Some(name) => Some(find_list(&store, &name)?.id),
                None => None,
            };
            let task = store.add_quick(&text.join(" "), default_list, now())?;
            print!("{} ", tr!("Added:"));
            // Recréé : `#liste` a pu créer une nouvelle liste.
            Printer::new(&store)?.task(&task, "");
        }
        Cmd::Ls { view, json } => {
            let view = resolve_view(&store, view.as_deref().unwrap_or("today"))?;
            out.tasks(&store.tasks(&view, now())?, json, matches!(view, View::List(_) | View::All))?;
        }
        Cmd::Search { query, json } => {
            out.tasks(&store.tasks(&View::Search(query.join(" ")), now())?, json, false)?;
        }
        Cmd::Done { id } => {
            let task = store.resolve_task(&id)?;
            match store.complete(task.id, now())? {
                Completion::Completed => println!("{}", tr!("Completed: {title}", title = task.title)),
                Completion::Rescheduled(due) => {
                    let when = due_label(&due, now().date());
                    println!("{}", tr!("{title} → next time: {when}", title = task.title, when = when))
                }
            }
        }
        Cmd::Undo { id } => {
            let task = store.resolve_task(&id)?;
            store.uncomplete(task.id)?;
            println!("{}", tr!("Reopened: {title}", title = task.title));
        }
        Cmd::Rm { id } => {
            let task = store.resolve_task(&id)?;
            store.delete_task(task.id)?;
            println!("{}", tr!("Deleted: {title}", title = task.title));
        }
        Cmd::Lists { json } => {
            let lists = store.lists()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&lists)?);
            } else {
                let counts = store.counts(now())?;
                for l in lists {
                    let n = counts.per_list.get(&l.id).copied().unwrap_or(0);
                    println!("{:>4}  {}", n, l.display_name());
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
    Err(err).with_context(|| tr!("couldn't start {program}", program = program.display()))
}

fn find_list(store: &Store, name: &str) -> Result<List> {
    store.find_list(name.trim_start_matches('#'))?.with_context(|| tr!("list not found: {name}", name = name))
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
        _ => bail!(tr!("unknown view: {name} (today, upcoming, all, done, inbox, #list, @tag)", name = name)),
    })
}

fn print_parse(input: &str) {
    let p = parse(input, now());
    let today = now().date();
    let mut fields = vec![(tr!("Title"), p.title.clone())];
    if let Some(due) = &p.due {
        fields.push((tr!("Due"), due_label(due, today)));
    }
    if let Some(r) = &p.recurrence {
        fields.push((tr!("Repeats"), format!("{} ({r})", r.describe())));
    }
    if let Some(l) = &p.list {
        fields.push((tr!("List"), l.clone()));
    }
    if !p.tags.is_empty() {
        fields.push((tr!("Tags"), p.tags.join(", ")));
    }
    if let Some(n) = p.priority.level() {
        fields.push((tr!("Priority"), format!("!{n}")));
    }
    // Libellés alignés ; un caractère CJK occupe deux colonnes.
    let width = |s: &str| s.chars().map(|c| if c.len_utf8() >= 3 { 2 } else { 1 }).sum::<usize>();
    let widest = fields.iter().map(|(label, _)| width(label)).max().unwrap_or(0);
    for (label, value) in fields {
        let label = format!("{label}{}", " ".repeat(widest - width(label)));
        // Translators: one line of `omado parse`, e.g. “Title   : Call Paul”.
        println!("{}", tr!("{label}: {value}", label = label, value = value));
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
            println!("{}", self.paint("2", tr!("Nothing here.")));
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
