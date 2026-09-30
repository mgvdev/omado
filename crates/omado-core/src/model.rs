//! Types du domaine : listes, tâches, échéances, priorités.
//!
//! Les dates sont « flottantes » (heure locale, sans fuseau), comme les VTODO
//! sans TZID : une tâche « demain 9h » sonne à 9h quel que soit le fuseau.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::recurrence::Recurrence;
use crate::tr;

pub type ListId = Uuid;
pub type TaskId = Uuid;

/// Identifiant fixe de la boîte de réception, créée à l'initialisation.
pub const INBOX_ID: ListId = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0001);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct List {
    pub id: ListId,
    pub name: String,
    /// Nom de couleur du thème Omarchy (`accent`, `red`, `green`…).
    pub color: Option<String>,
    pub position: i64,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

impl List {
    pub fn is_inbox(&self) -> bool {
        self.id == INBOX_ID
    }

    /// Nom à afficher : la boîte de réception, qu'on ne renomme pas, suit la langue.
    pub fn display_name(&self) -> &str {
        if self.is_inbox() { tr!("Inbox") } else { &self.name }
    }
}

/// Priorité façon Todoist : `!1` est la plus haute.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Priority {
    #[default]
    None,
    Low,
    Medium,
    High,
}

impl Priority {
    pub fn to_db(self) -> i64 {
        match self {
            Priority::None => 0,
            Priority::Low => 1,
            Priority::Medium => 2,
            Priority::High => 3,
        }
    }

    pub fn from_db(v: i64) -> Self {
        match v {
            1 => Priority::Low,
            2 => Priority::Medium,
            3 => Priority::High,
            _ => Priority::None,
        }
    }

    /// Niveau `!n` saisi par l'utilisateur (1 = haute).
    pub fn from_level(n: u8) -> Option<Self> {
        match n {
            1 => Some(Priority::High),
            2 => Some(Priority::Medium),
            3 => Some(Priority::Low),
            4 => Some(Priority::None),
            _ => None,
        }
    }

    pub fn level(self) -> Option<u8> {
        match self {
            Priority::High => Some(1),
            Priority::Medium => Some(2),
            Priority::Low => Some(3),
            Priority::None => None,
        }
    }
}

/// Échéance : un jour, éventuellement une heure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Due {
    pub date: NaiveDate,
    pub time: Option<NaiveTime>,
}

impl Due {
    pub fn on(date: NaiveDate) -> Self {
        Due { date, time: None }
    }

    pub fn at(date: NaiveDate, time: NaiveTime) -> Self {
        Due { date, time: Some(time) }
    }

    /// Instant de l'échéance ; fin de journée si aucune heure n'est fixée.
    pub fn deadline(&self) -> NaiveDateTime {
        match self.time {
            Some(t) => self.date.and_time(t),
            None => self.date.and_hms_opt(23, 59, 59).expect("heure valide"),
        }
    }

    pub fn is_overdue(&self, now: NaiveDateTime) -> bool {
        match self.time {
            Some(_) => self.deadline() < now,
            None => self.date < now.date(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub list_id: ListId,
    pub parent_id: Option<TaskId>,
    pub title: String,
    pub notes: String,
    pub priority: Priority,
    pub due: Option<Due>,
    /// Moment du rappel ; par défaut l'heure d'échéance quand elle existe.
    pub remind_at: Option<NaiveDateTime>,
    pub recurrence: Option<Recurrence>,
    pub tags: Vec<String>,
    pub completed_at: Option<NaiveDateTime>,
    pub position: i64,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

impl Task {
    pub fn is_completed(&self) -> bool {
        self.completed_at.is_some()
    }
}

/// Données nécessaires pour créer une tâche.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewTask {
    pub list_id: Option<ListId>,
    pub parent_id: Option<TaskId>,
    pub title: String,
    pub notes: String,
    pub priority: Priority,
    pub due: Option<Due>,
    pub remind_at: Option<NaiveDateTime>,
    pub recurrence: Option<Recurrence>,
    pub tags: Vec<String>,
}

/// Vues calculées affichées dans la barre latérale.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum View {
    /// Échues aujourd'hui ou en retard.
    Today,
    /// Toutes les tâches avec une échéance future.
    Upcoming,
    /// Tout ce qui n'est pas terminé.
    All,
    /// Tâches terminées, les plus récentes d'abord.
    Completed,
    List(ListId),
    Tag(String),
    Search(String),
}
