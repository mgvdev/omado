//! Cœur d'Omado : modèle, stockage SQLite, saisie rapide et récurrences.

pub mod human;
pub mod model;
pub mod parse;
pub mod recurrence;
pub mod store;

pub use model::{Due, INBOX_ID, List, ListId, NewTask, Priority, Task, TaskId, View};
pub use parse::{Parsed, Span, SpanKind, parse};
pub use recurrence::{Freq, Recurrence};
pub use store::{Completion, Counts, Error, Store};

/// Heure locale courante, sans fuseau (voir [`model`]).
pub fn now() -> chrono::NaiveDateTime {
    chrono::Local::now().naive_local()
}
