//! Stockage SQLite partagé par l'interface, la CLI et le démon.
//!
//! Plusieurs processus ouvrent la même base : mode WAL, délai d'attente sur
//! verrou, et `data_version()` pour détecter les écritures des autres.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Days, Local, NaiveDate, NaiveDateTime, NaiveTime};
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OptionalExtension, Row, params};
use uuid::Uuid;

use crate::model::{Due, INBOX_ID, List, ListId, NewTask, Priority, Task, TaskId, View};
use crate::parse::{self, Parsed, fold, list_matches};
use crate::{tr, trn};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{}", message(self))]
    EmptyTitle,
    #[error("{}", message(self))]
    NotFound,
    #[error("{}", message(self))]
    InboxProtected,
    #[error("{}", message(self))]
    Ambiguous(usize, String),
}

/// Message traduit des erreurs propres à Omado (hors des attributs, que
/// l'extraction des chaînes ne lit pas).
fn message(e: &Error) -> String {
    match e {
        Error::EmptyTitle => tr!("the title can't be empty").into(),
        Error::NotFound => tr!("not found").into(),
        Error::InboxProtected => tr!("the inbox can't be deleted").into(),
        Error::Ambiguous(n, prefix) => trn!(
            "ambiguous ID: {n} task starts with “{prefix}”",
            "ambiguous ID: {n} tasks start with “{prefix}”",
            *n,
            prefix = prefix
        ),
        Error::Sqlite(e) => e.to_string(),
        Error::Io(e) => e.to_string(),
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Résultat du cochage d'une tâche.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    Completed,
    /// Tâche récurrente : reprogrammée à l'occurrence suivante.
    Rescheduled(Due),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub today: usize,
    pub overdue: usize,
    pub upcoming: usize,
    pub all: usize,
    pub per_list: HashMap<ListId, usize>,
}

const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE lists (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    color       TEXT,
    position    INTEGER NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE TABLE tasks (
    id            TEXT PRIMARY KEY,
    list_id       TEXT NOT NULL REFERENCES lists(id) ON DELETE CASCADE,
    parent_id     TEXT REFERENCES tasks(id) ON DELETE CASCADE,
    title         TEXT NOT NULL,
    notes         TEXT NOT NULL DEFAULT '',
    priority      INTEGER NOT NULL DEFAULT 0,
    due_date      TEXT,
    due_time      TEXT,
    remind_at     TEXT,
    reminded_at   TEXT,
    rrule         TEXT,
    completed_at  TEXT,
    position      INTEGER NOT NULL,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE TABLE task_tags (
    task_id  TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    tag      TEXT NOT NULL,
    PRIMARY KEY (task_id, tag)
);
CREATE INDEX tasks_by_list ON tasks(list_id, position);
CREATE INDEX tasks_by_due ON tasks(due_date) WHERE completed_at IS NULL;
CREATE INDEX tasks_by_reminder ON tasks(remind_at) WHERE completed_at IS NULL;
CREATE INDEX tasks_by_parent ON tasks(parent_id);
";

const TASK_COLS: &str = "t.id, t.list_id, t.parent_id, t.title, t.notes, t.priority, t.due_date, t.due_time, \
                         t.remind_at, t.rrule, t.completed_at, t.position, t.created_at, t.updated_at";

/// Chemin par défaut : `$OMADO_DB`, sinon `~/.local/share/omado/omado.db`.
pub fn default_path() -> PathBuf {
    if let Some(p) = std::env::var_os("OMADO_DB") {
        return p.into();
    }
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("omado").join("omado.db")
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open_default() -> Result<Self> {
        Self::open(&default_path())
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.create_scalar_function(
            "omado_fold",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| Ok(fold(&ctx.get::<String>(0)?)),
        )?;
        let mut store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version >= SCHEMA_VERSION {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        tx.execute_batch(SCHEMA)?;
        let now = stamp();
        tx.execute(
            "INSERT INTO lists (id, name, color, position, created_at, updated_at) VALUES (?1, ?2, NULL, 0, ?3, ?3)",
            params![INBOX_ID.to_string(), "Inbox", now],
        )?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    }

    /// Change dès qu'une autre connexion a écrit dans la base.
    pub fn data_version(&self) -> Result<i64> {
        Ok(self.conn.pragma_query_value(None, "data_version", |r| r.get(0))?)
    }

    // --- Listes -----------------------------------------------------------

    pub fn lists(&self) -> Result<Vec<List>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, color, position, created_at, updated_at FROM lists ORDER BY position, created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(List {
                id: uuid_col(r, 0)?,
                name: r.get(1)?,
                color: r.get(2)?,
                position: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn list(&self, id: ListId) -> Result<List> {
        self.lists()?.into_iter().find(|l| l.id == id).ok_or(Error::NotFound)
    }

    pub fn create_list(&self, name: &str) -> Result<List> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::EmptyTitle);
        }
        let now = stamp();
        let id = Uuid::new_v4();
        let position: i64 =
            self.conn.query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM lists", [], |r| r.get(0))?;
        self.conn.execute(
            "INSERT INTO lists (id, name, color, position, created_at, updated_at) VALUES (?1, ?2, NULL, ?3, ?4, ?4)",
            params![id.to_string(), name, position, now],
        )?;
        self.list(id)
    }

    /// Liste désignée par `#nom`. La boîte de réception répond aussi à son nom
    /// affiché et à « inbox », quel que soit le nom enregistré.
    pub fn find_list(&self, typed: &str) -> Result<Option<List>> {
        let matches = |l: &List| {
            list_matches(&l.name, typed)
                || (l.is_inbox() && (list_matches(l.display_name(), typed) || list_matches("inbox", typed)))
        };
        Ok(self.lists()?.into_iter().find(matches))
    }

    pub fn rename_list(&self, id: ListId, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::EmptyTitle);
        }
        self.touch("UPDATE lists SET name = ?2, updated_at = ?3 WHERE id = ?1", id, name)
    }

    pub fn set_list_color(&self, id: ListId, color: Option<&str>) -> Result<()> {
        self.touch("UPDATE lists SET color = ?2, updated_at = ?3 WHERE id = ?1", id, color)
    }

    /// Supprime la liste et ses tâches.
    pub fn delete_list(&self, id: ListId) -> Result<()> {
        if id == INBOX_ID {
            return Err(Error::InboxProtected);
        }
        self.conn.execute("DELETE FROM lists WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    fn touch(&self, sql: &str, id: Uuid, value: impl rusqlite::ToSql) -> Result<()> {
        match self.conn.execute(sql, params![id.to_string(), value, stamp()])? {
            0 => Err(Error::NotFound),
            _ => Ok(()),
        }
    }

    // --- Tâches -----------------------------------------------------------

    pub fn add_task(&self, new: NewTask) -> Result<Task> {
        let title = new.title.trim();
        if title.is_empty() {
            return Err(Error::EmptyTitle);
        }
        let list_id = match new.parent_id {
            Some(parent) => self.task(parent)?.list_id,
            None => new.list_id.unwrap_or(INBOX_ID),
        };
        let id = Uuid::new_v4();
        let now = stamp();
        let position: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM tasks WHERE list_id = ?1",
            [list_id.to_string()],
            |r| r.get(0),
        )?;
        let recurrence = match (new.recurrence, new.due) {
            (Some(r), Some(due)) => Some(r.anchored(due.date)),
            (r, _) => r,
        };
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO tasks (id, list_id, parent_id, title, notes, priority, due_date, due_time, remind_at, rrule,
                                position, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
            params![
                id.to_string(),
                list_id.to_string(),
                new.parent_id.map(|p| p.to_string()),
                title,
                new.notes,
                new.priority.to_db(),
                new.due.map(|d| d.date),
                new.due.and_then(|d| d.time),
                new.remind_at,
                recurrence.map(|r| r.to_string()),
                position,
                now,
            ],
        )?;
        write_tags(&tx, id, &new.tags)?;
        tx.commit()?;
        self.task(id)
    }

    /// Crée une tâche depuis la saisie rapide ; `#liste` crée la liste si besoin.
    pub fn add_quick(&self, input: &str, default_list: Option<ListId>, now: NaiveDateTime) -> Result<Task> {
        self.add_parsed(parse::parse(input, now), default_list)
    }

    /// Comme [`Store::add_quick`], pour une saisie déjà analysée (et éventuellement complétée).
    pub fn add_parsed(&self, parsed: Parsed, default_list: Option<ListId>) -> Result<Task> {
        let list_id = match &parsed.list {
            Some(name) => Some(match self.find_list(name)? {
                Some(list) => list.id,
                None => self.create_list(name)?.id,
            }),
            None => default_list,
        };
        let mut new = parsed.into_new_task();
        new.list_id = list_id;
        self.add_task(new)
    }

    pub fn task(&self, id: TaskId) -> Result<Task> {
        let sql = format!("SELECT {TASK_COLS} FROM tasks t WHERE t.id = ?1");
        let task = self.conn.query_row(&sql, [id.to_string()], task_from_row).optional()?.ok_or(Error::NotFound)?;
        self.with_tags(vec![task]).map(|mut v| v.remove(0))
    }

    /// Retrouve une tâche par le début de son identifiant (comme `git` avec les commits).
    pub fn resolve_task(&self, prefix: &str) -> Result<Task> {
        let prefix = prefix.trim().to_lowercase();
        if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return Err(Error::NotFound);
        }
        let mut found = self.query_tasks("t.id LIKE ?1", "t.id LIMIT 2", &[&format!("{prefix}%")])?;
        match found.len() {
            0 => Err(Error::NotFound),
            1 => Ok(found.remove(0)),
            n => Err(Error::Ambiguous(n, prefix)),
        }
    }

    /// Enregistre les champs modifiables d'une tâche (titre, notes, échéance, etc.).
    pub fn update_task(&self, task: &Task) -> Result<()> {
        if task.title.trim().is_empty() {
            return Err(Error::EmptyTitle);
        }
        let previous = self.task(task.id)?;
        let recurrence = match (&task.recurrence, task.due) {
            (Some(r), Some(due)) => Some(r.clone().anchored(due.date)),
            (r, _) => r.clone(),
        };
        let tx = self.conn.unchecked_transaction()?;
        // Un rappel déplacé doit pouvoir sonner à nouveau.
        let reset_reminder = previous.remind_at != task.remind_at;
        tx.execute(
            "UPDATE tasks SET list_id = ?2, title = ?3, notes = ?4, priority = ?5, due_date = ?6, due_time = ?7,
                    remind_at = ?8, rrule = ?9, updated_at = ?10,
                    reminded_at = CASE WHEN ?11 THEN NULL ELSE reminded_at END
             WHERE id = ?1",
            params![
                task.id.to_string(),
                task.list_id.to_string(),
                task.title.trim(),
                task.notes,
                task.priority.to_db(),
                task.due.map(|d| d.date),
                task.due.and_then(|d| d.time),
                task.remind_at,
                recurrence.map(|r| r.to_string()),
                stamp(),
                reset_reminder,
            ],
        )?;
        if previous.list_id != task.list_id {
            tx.execute(
                "UPDATE tasks SET list_id = ?2 WHERE parent_id = ?1",
                params![task.id.to_string(), task.list_id.to_string()],
            )?;
        }
        tx.execute("DELETE FROM task_tags WHERE task_id = ?1", [task.id.to_string()])?;
        write_tags(&tx, task.id, &task.tags)?;
        tx.commit()?;
        Ok(())
    }

    /// Coche une tâche ; une tâche récurrente passe à sa prochaine occurrence.
    pub fn complete(&self, id: TaskId, now: NaiveDateTime) -> Result<Completion> {
        let task = self.task(id)?;
        if let (Some(rec), Some(due)) = (&task.recurrence, task.due) {
            let today = now.date();
            let mut next = rec.next_after(due.date);
            while next <= today {
                next = rec.next_after(next);
            }
            let shift = Days::new((next - due.date).num_days() as u64);
            let new_due = Due { date: next, time: due.time };
            self.conn.execute(
                "UPDATE tasks SET due_date = ?2, remind_at = ?3, reminded_at = NULL, updated_at = ?4 WHERE id = ?1",
                params![id.to_string(), next, task.remind_at.map(|r| r + shift), stamp()],
            )?;
            // Les sous-tâches repartent de zéro avec la nouvelle occurrence.
            self.conn.execute("UPDATE tasks SET completed_at = NULL WHERE parent_id = ?1", [id.to_string()])?;
            return Ok(Completion::Rescheduled(new_due));
        }
        self.conn.execute(
            "UPDATE tasks SET completed_at = ?2, updated_at = ?3 WHERE id = ?1 OR (parent_id = ?1 AND completed_at IS NULL)",
            params![id.to_string(), now, stamp()],
        )?;
        Ok(Completion::Completed)
    }

    pub fn uncomplete(&self, id: TaskId) -> Result<()> {
        self.touch("UPDATE tasks SET completed_at = ?2, updated_at = ?3 WHERE id = ?1", id, None::<NaiveDateTime>)
    }

    pub fn delete_task(&self, id: TaskId) -> Result<()> {
        match self.conn.execute("DELETE FROM tasks WHERE id = ?1", [id.to_string()])? {
            0 => Err(Error::NotFound),
            _ => Ok(()),
        }
    }

    /// Instantané d'une tâche et de ses sous-tâches, pour pouvoir annuler.
    pub fn snapshot(&self, id: TaskId) -> Result<Vec<Task>> {
        let mut tasks = vec![self.task(id)?];
        tasks.extend(self.subtasks(id)?);
        Ok(tasks)
    }

    /// Remet des tâches dans l'état d'un instantané (parents avant enfants).
    /// Les rappels déjà passés ne sonnent pas une seconde fois.
    pub fn restore(&self, tasks: &[Task]) -> Result<()> {
        let now = stamp();
        let tx = self.conn.unchecked_transaction()?;
        for t in tasks {
            tx.execute(
                "INSERT INTO tasks (id, list_id, parent_id, title, notes, priority, due_date, due_time, remind_at,
                                    reminded_at, rrule, completed_at, position, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, CASE WHEN ?9 <= ?15 THEN ?9 END, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(id) DO UPDATE SET
                    list_id = excluded.list_id, parent_id = excluded.parent_id, title = excluded.title,
                    notes = excluded.notes, priority = excluded.priority, due_date = excluded.due_date,
                    due_time = excluded.due_time, remind_at = excluded.remind_at, reminded_at = excluded.reminded_at,
                    rrule = excluded.rrule, completed_at = excluded.completed_at, position = excluded.position,
                    updated_at = excluded.updated_at",
                params![
                    t.id.to_string(),
                    t.list_id.to_string(),
                    t.parent_id.map(|p| p.to_string()),
                    t.title,
                    t.notes,
                    t.priority.to_db(),
                    t.due.map(|d| d.date),
                    t.due.and_then(|d| d.time),
                    t.remind_at,
                    t.recurrence.as_ref().map(|r| r.to_string()),
                    t.completed_at,
                    t.position,
                    t.created_at,
                    t.updated_at,
                    now,
                ],
            )?;
            tx.execute("DELETE FROM task_tags WHERE task_id = ?1", [t.id.to_string()])?;
            write_tags(&tx, t.id, &t.tags)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Réordonne les tâches d'une liste selon l'ordre donné.
    pub fn reorder(&self, ordered: &[TaskId]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (pos, id) in ordered.iter().enumerate() {
            tx.execute("UPDATE tasks SET position = ?2 WHERE id = ?1", params![id.to_string(), pos as i64 + 1])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn subtasks(&self, parent: TaskId) -> Result<Vec<Task>> {
        self.query_tasks("t.parent_id = ?1", "t.completed_at IS NOT NULL, t.position", &[&parent.to_string()])
    }

    /// Tâches d'une vue. Les vues de liste incluent les sous-tâches ; l'appelant les regroupe.
    pub fn tasks(&self, view: &View, now: NaiveDateTime) -> Result<Vec<Task>> {
        let today = now.date();
        let by_due = "t.due_date, t.due_time IS NULL, t.due_time, t.priority DESC, t.position";
        match view {
            View::Today => self.query_tasks("t.completed_at IS NULL AND t.due_date <= ?1", by_due, &[&today]),
            View::Upcoming => self.query_tasks("t.completed_at IS NULL AND t.due_date > ?1", by_due, &[&today]),
            View::All => self.query_tasks(
                "t.completed_at IS NULL",
                "t.due_date IS NULL, t.due_date, t.due_time IS NULL, t.due_time, t.priority DESC, t.position",
                &[],
            ),
            View::Completed => self.query_tasks("t.completed_at IS NOT NULL", "t.completed_at DESC LIMIT 200", &[]),
            View::List(id) => self.query_tasks(
                "t.list_id = ?1 AND (t.completed_at IS NULL OR t.parent_id IS NOT NULL)",
                "t.position",
                &[&id.to_string()],
            ),
            View::Tag(tag) => self.query_tasks(
                "t.completed_at IS NULL AND t.id IN (SELECT task_id FROM task_tags WHERE tag = ?1)",
                by_due,
                &[&tag.to_lowercase()],
            ),
            View::Search(q) => {
                let pattern = format!("%{}%", fold(q.trim()).replace(['%', '_'], ""));
                self.query_tasks(
                    "(omado_fold(t.title) LIKE ?1 OR omado_fold(t.notes) LIKE ?1)",
                    "t.completed_at IS NOT NULL, t.due_date IS NULL, t.due_date, t.position",
                    &[&pattern],
                )
            }
        }
    }

    pub fn counts(&self, now: NaiveDateTime) -> Result<Counts> {
        let today = now.date();
        let count = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> Result<usize> {
            Ok(self.conn.query_row(sql, p, |r| r.get::<_, i64>(0))? as usize)
        };
        let open = "SELECT COUNT(*) FROM tasks WHERE completed_at IS NULL";
        let mut counts = Counts {
            today: count(&format!("{open} AND due_date <= ?1"), &[&today])?,
            overdue: count(&format!("{open} AND due_date < ?1"), &[&today])?,
            upcoming: count(&format!("{open} AND due_date > ?1"), &[&today])?,
            all: count(open, &[])?,
            per_list: HashMap::new(),
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT list_id, COUNT(*) FROM tasks WHERE completed_at IS NULL AND parent_id IS NULL GROUP BY list_id",
        )?;
        for row in stmt.query_map([], |r| Ok((uuid_col(r, 0)?, r.get::<_, i64>(1)?)))? {
            let (id, n) = row?;
            counts.per_list.insert(id, n as usize);
        }
        Ok(counts)
    }

    /// Étiquettes utilisées par des tâches ouvertes, avec leur nombre.
    pub fn tags(&self) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT tt.tag, COUNT(*) FROM task_tags tt JOIN tasks t ON t.id = tt.task_id
             WHERE t.completed_at IS NULL GROUP BY tt.tag ORDER BY tt.tag",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // --- Rappels ----------------------------------------------------------

    /// Rappels arrivés à échéance et pas encore envoyés.
    pub fn due_reminders(&self, now: NaiveDateTime) -> Result<Vec<Task>> {
        self.query_tasks(
            "t.completed_at IS NULL AND t.remind_at <= ?1 AND (t.reminded_at IS NULL OR t.reminded_at < t.remind_at)",
            "t.remind_at",
            &[&now],
        )
    }

    pub fn mark_reminded(&self, id: TaskId, at: NaiveDateTime) -> Result<()> {
        self.conn.execute("UPDATE tasks SET reminded_at = ?2 WHERE id = ?1", params![id.to_string(), at])?;
        Ok(())
    }

    pub fn next_reminder_after(&self, now: NaiveDateTime) -> Result<Option<NaiveDateTime>> {
        Ok(self.conn.query_row(
            "SELECT MIN(remind_at) FROM tasks WHERE completed_at IS NULL AND remind_at > ?1",
            [now],
            |r| r.get(0),
        )?)
    }

    /// Reporte le rappel ; l'échéance ne bouge pas.
    pub fn snooze(&self, id: TaskId, until: NaiveDateTime) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET remind_at = ?2, reminded_at = NULL, updated_at = ?3 WHERE id = ?1",
            params![id.to_string(), until, stamp()],
        )?;
        Ok(())
    }

    // --- Interne ----------------------------------------------------------

    fn query_tasks(&self, filter: &str, order: &str, p: &[&dyn rusqlite::ToSql]) -> Result<Vec<Task>> {
        let sql = format!("SELECT {TASK_COLS} FROM tasks t WHERE {filter} ORDER BY {order}");
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let tasks = stmt.query_map(p, task_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        self.with_tags(tasks)
    }

    fn with_tags(&self, mut tasks: Vec<Task>) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare_cached("SELECT tag FROM task_tags WHERE task_id = ?1 ORDER BY tag")?;
        for task in &mut tasks {
            task.tags = stmt.query_map([task.id.to_string()], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        }
        Ok(tasks)
    }
}

fn write_tags(conn: &Connection, id: TaskId, tags: &[String]) -> Result<()> {
    let mut stmt = conn.prepare_cached("INSERT OR IGNORE INTO task_tags (task_id, tag) VALUES (?1, ?2)")?;
    for tag in tags {
        let tag = tag.trim().trim_start_matches('@').to_lowercase();
        if !tag.is_empty() {
            stmt.execute(params![id.to_string(), tag])?;
        }
    }
    Ok(())
}

fn stamp() -> NaiveDateTime {
    Local::now().naive_local()
}

fn uuid_col(r: &Row, idx: usize) -> rusqlite::Result<Uuid> {
    let s: String = r.get(idx)?;
    Uuid::parse_str(&s)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e)))
}

fn task_from_row(r: &Row) -> rusqlite::Result<Task> {
    let date: Option<NaiveDate> = r.get(6)?;
    let time: Option<NaiveTime> = r.get(7)?;
    let rrule: Option<String> = r.get(9)?;
    Ok(Task {
        id: uuid_col(r, 0)?,
        list_id: uuid_col(r, 1)?,
        parent_id: r
            .get::<_, Option<String>>(2)?
            .map(|s| Uuid::parse_str(&s))
            .transpose()
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e)))?,
        title: r.get(3)?,
        notes: r.get(4)?,
        priority: Priority::from_db(r.get(5)?),
        due: date.map(|date| Due { date, time }),
        remind_at: r.get(8)?,
        // Une règle illisible (écrite par une version future) est ignorée plutôt que bloquante.
        recurrence: rrule.and_then(|s| s.parse().ok()),
        tags: Vec::new(),
        completed_at: r.get(10)?,
        position: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recurrence::{Freq, Recurrence};

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 9, 29).unwrap().and_hms_opt(10, 0, 0).unwrap()
    }

    fn day(m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, d).unwrap()
    }

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn inbox_exists_and_is_protected() {
        let s = store();
        let lists = s.lists().unwrap();
        assert_eq!(lists.len(), 1);
        assert!(lists[0].is_inbox());
        assert!(matches!(s.delete_list(INBOX_ID), Err(Error::InboxProtected)));
    }

    #[test]
    fn quick_add_resolves_or_creates_list() {
        let s = store();
        let courses = s.create_list("Courses maison").unwrap();
        let t = s.add_quick("Lait demain #courses-maison @frais !2", None, now()).unwrap();
        assert_eq!(t.list_id, courses.id);
        assert_eq!(t.title, "Lait");
        assert_eq!(t.tags, vec!["frais"]);
        assert_eq!(t.priority, Priority::Medium);

        let t = s.add_quick("Rapport #Travail", None, now()).unwrap();
        assert_eq!(s.list(t.list_id).unwrap().name, "Travail");
        assert_eq!(s.lists().unwrap().len(), 3);

        assert!(matches!(s.add_quick("demain #travail", None, now()), Err(Error::EmptyTitle)));
    }

    #[test]
    fn smart_views() {
        let s = store();
        s.add_quick("En retard hier", None, now()).unwrap();
        s.add_quick("Réunion aujourd'hui 18h", None, now()).unwrap();
        s.add_quick("Plus tard vendredi", None, now()).unwrap();
        s.add_quick("Sans date", None, now()).unwrap();
        let titles = |v: View| s.tasks(&v, now()).unwrap().into_iter().map(|t| t.title).collect::<Vec<_>>();
        // « hier » n'est pas reconnu : la tâche n'a pas de date.
        assert_eq!(titles(View::Today), vec!["Réunion"]);
        assert_eq!(titles(View::Upcoming), vec!["Plus tard"]);
        assert_eq!(titles(View::All), vec!["Réunion", "Plus tard", "En retard hier", "Sans date"]);
        assert_eq!(titles(View::Search("RETARD".into())), vec!["En retard hier"]);

        let c = s.counts(now()).unwrap();
        assert_eq!((c.today, c.upcoming, c.all), (1, 1, 4));
        assert_eq!(c.per_list[&INBOX_ID], 4);
    }

    #[test]
    fn overdue_is_in_today() {
        let s = store();
        let yesterday = now() - chrono::Duration::days(1);
        s.add_quick("Vieux truc aujourd'hui", None, yesterday).unwrap();
        let today = s.tasks(&View::Today, now()).unwrap();
        assert_eq!(today.len(), 1);
        assert!(today[0].due.unwrap().is_overdue(now()));
        assert_eq!(s.counts(now()).unwrap().overdue, 1);
    }

    #[test]
    fn complete_and_uncomplete() {
        let s = store();
        let parent = s.add_quick("Parent", None, now()).unwrap();
        let child =
            s.add_task(NewTask { parent_id: Some(parent.id), title: "Enfant".into(), ..Default::default() }).unwrap();
        assert_eq!(s.complete(parent.id, now()).unwrap(), Completion::Completed);
        assert!(s.task(child.id).unwrap().is_completed(), "cocher le parent coche les sous-tâches");
        assert_eq!(s.tasks(&View::Completed, now()).unwrap().len(), 2);
        s.uncomplete(parent.id).unwrap();
        assert!(!s.task(parent.id).unwrap().is_completed());
    }

    #[test]
    fn completing_recurring_task_reschedules() {
        let s = store();
        let t = s.add_quick("Méditer tous les jours à 18h", None, now()).unwrap();
        assert_eq!(t.remind_at, Some(day(9, 29).and_hms_opt(18, 0, 0).unwrap()));
        let done = s.complete(t.id, now()).unwrap();
        let expected = Due::at(day(9, 30), NaiveTime::from_hms_opt(18, 0, 0).unwrap());
        assert_eq!(done, Completion::Rescheduled(expected));
        let t = s.task(t.id).unwrap();
        assert!(!t.is_completed());
        assert_eq!(t.remind_at, Some(day(9, 30).and_hms_opt(18, 0, 0).unwrap()));
    }

    #[test]
    fn overdue_recurring_task_skips_to_future() {
        let s = store();
        let mut t = s
            .add_task(NewTask {
                title: "Arroser".into(),
                due: Some(Due::on(day(9, 20))),
                recurrence: Some(Recurrence::new(Freq::Daily, 2)),
                ..Default::default()
            })
            .unwrap();
        s.complete(t.id, now()).unwrap();
        t = s.task(t.id).unwrap();
        assert_eq!(t.due, Some(Due::on(day(9, 30))));
    }

    #[test]
    fn reminders_fire_once() {
        let s = store();
        let t = s.add_quick("Appeler dans 30 min", None, now()).unwrap();
        assert!(s.due_reminders(now()).unwrap().is_empty());
        let later = now() + chrono::Duration::minutes(31);
        assert_eq!(s.next_reminder_after(now()).unwrap(), t.remind_at);
        assert_eq!(s.due_reminders(later).unwrap().len(), 1);
        s.mark_reminded(t.id, later).unwrap();
        assert!(s.due_reminders(later).unwrap().is_empty());

        s.snooze(t.id, later + chrono::Duration::minutes(10)).unwrap();
        assert!(s.due_reminders(later).unwrap().is_empty());
        assert_eq!(s.due_reminders(later + chrono::Duration::minutes(10)).unwrap().len(), 1);
    }

    #[test]
    fn update_task_and_tags() {
        let s = store();
        let mut t = s.add_quick("Brouillon @a", None, now()).unwrap();
        let travail = s.create_list("Travail").unwrap();
        t.title = "Final".into();
        t.tags = vec!["b".into(), "@C".into()];
        t.list_id = travail.id;
        t.notes = "détails".into();
        s.update_task(&t).unwrap();
        let t = s.task(t.id).unwrap();
        assert_eq!(t.title, "Final");
        assert_eq!(t.tags, vec!["b", "c"]);
        assert_eq!(t.list_id, travail.id);
        assert_eq!(s.tags().unwrap(), vec![("b".to_string(), 1), ("c".to_string(), 1)]);
        assert_eq!(s.tasks(&View::Tag("C".into()), now()).unwrap().len(), 1);
    }

    #[test]
    fn delete_list_cascades() {
        let s = store();
        let l = s.create_list("Temp").unwrap();
        let t = s.add_quick("x #temp", None, now()).unwrap();
        s.delete_list(l.id).unwrap();
        assert!(matches!(s.task(t.id), Err(Error::NotFound)));
    }

    #[test]
    fn restore_undoes_delete_and_completion() {
        let s = store();
        let parent = s.add_quick("Parent demain 9h @x", None, now()).unwrap();
        s.add_task(NewTask { parent_id: Some(parent.id), title: "Enfant".into(), ..Default::default() }).unwrap();
        let snap = s.snapshot(parent.id).unwrap();
        s.delete_task(parent.id).unwrap();
        s.restore(&snap).unwrap();
        assert_eq!(s.snapshot(parent.id).unwrap(), snap);

        s.complete(parent.id, now()).unwrap();
        s.restore(&snap).unwrap();
        assert!(s.snapshot(parent.id).unwrap().iter().all(|t| !t.is_completed()));

        // Restaurer un rappel passé ne le fait pas sonner à nouveau.
        let old = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap().and_hms_opt(9, 0, 0).unwrap();
        let mut past =
            s.add_task(NewTask { title: "Ancien".into(), remind_at: Some(old), ..Default::default() }).unwrap();
        past.title = "Ancien restauré".into();
        s.delete_task(past.id).unwrap();
        s.restore(&[past]).unwrap();
        assert!(s.due_reminders(now()).unwrap().iter().all(|t| t.title != "Ancien restauré"));
    }

    #[test]
    fn reorder_tasks() {
        let s = store();
        let a = s.add_quick("A", None, now()).unwrap();
        let b = s.add_quick("B", None, now()).unwrap();
        s.reorder(&[b.id, a.id]).unwrap();
        let titles: Vec<_> = s.tasks(&View::List(INBOX_ID), now()).unwrap().into_iter().map(|t| t.title).collect();
        assert_eq!(titles, vec!["B", "A"]);
    }

    #[test]
    fn data_version_sees_other_connections() {
        let dir = std::env::temp_dir().join(format!("omado-test-{}", Uuid::new_v4()));
        let path = dir.join("t.db");
        let a = Store::open(&path).unwrap();
        let b = Store::open(&path).unwrap();
        let before = a.data_version().unwrap();
        b.add_quick("depuis b", None, now()).unwrap();
        assert_ne!(a.data_version().unwrap(), before);
        drop((a, b));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
