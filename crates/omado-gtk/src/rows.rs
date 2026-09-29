//! Lignes des listes : tâches (zone principale) et listes (barre latérale).

use std::time::Duration;

use chrono::NaiveDateTime;
use omado_core::human::due_label;
use omado_core::{List, Parsed, Priority, Task, TaskId};
use relm4::factory::{DynamicIndex, FactoryComponent, FactorySender};
use relm4::gtk::{self, glib, pango, prelude::*};

use crate::anim;
use crate::theme;

// --- Tâches ---------------------------------------------------------------

pub struct TaskInit {
    pub task: Task,
    pub nested: bool,
    /// Nom de la liste, affiché dans les vues qui mélangent les listes.
    pub list_name: Option<String>,
    /// Sous-tâches terminées / total.
    pub progress: Option<(usize, usize)>,
    /// Apparue depuis le dernier affichage : la ligne se déplie et s'éclaire.
    pub fresh: bool,
    /// Modifiée sur place (reprogrammée…) : elle s'éclaire sans bouger.
    pub flash: bool,
    pub now: NaiveDateTime,
}

pub struct TaskItem {
    pub task: Task,
    init: Option<TaskInit>,
}

#[derive(Debug)]
pub enum TaskMsg {
    /// Cochée : la ligne s'estompe et se barre avant de partir.
    Completing,
    /// Se replie (avant d'être retirée de la liste).
    Collapse,
    /// Inverse la case, comme un clic (raccourci `x`).
    Check,
}

#[derive(Debug)]
pub enum TaskOutput {
    Toggle(TaskId, bool),
}

pub struct TaskWidgets {
    revealer: gtk::Revealer,
    check: gtk::CheckButton,
    content: gtk::Box,
    title: gtk::Label,
}

impl FactoryComponent for TaskItem {
    type ParentWidget = gtk::ListBox;
    type CommandOutput = ();
    type Input = TaskMsg;
    type Output = TaskOutput;
    type Init = TaskInit;
    type Root = gtk::Revealer;
    type Widgets = TaskWidgets;
    type Index = DynamicIndex;

    fn init_model(init: TaskInit, _: &DynamicIndex, _: FactorySender<Self>) -> Self {
        TaskItem { task: init.task.clone(), init: Some(init) }
    }

    fn init_root(&self) -> gtk::Revealer {
        gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(anim::ROW_MS)
            .build()
    }

    fn init_widgets(
        &mut self,
        _: &DynamicIndex,
        revealer: gtk::Revealer,
        row: &gtk::ListBoxRow,
        sender: FactorySender<Self>,
    ) -> TaskWidgets {
        let init = self.init.take().expect("initialisé une seule fois");
        let task = &init.task;
        if init.nested {
            row.add_css_class("subtask");
        }

        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        content.add_css_class("task-row");
        revealer.set_child(Some(&content));
        if init.fresh || init.flash {
            content.add_css_class("fresh");
        }
        if init.fresh {
            // Déplier une fois la ligne affichée, pour que la transition se voie.
            revealer.connect_map(|r| r.set_reveal_child(true));
        } else {
            revealer.set_transition_duration(0);
            revealer.set_reveal_child(true);
            revealer.set_transition_duration(anim::ROW_MS);
        }

        let check = gtk::CheckButton::builder().active(task.is_completed()).valign(gtk::Align::Start).build();
        check.add_css_class("task-check");
        check.add_css_class(prio_class(task.priority));
        check.set_tooltip_text(Some(if task.is_completed() { "Rouvrir (x)" } else { "Terminer (x)" }));
        let id = task.id;
        check.connect_toggled(move |b| {
            let _ = sender.output(TaskOutput::Toggle(id, b.is_active()));
        });
        content.append(&check);

        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let title = gtk::Label::builder().label(&task.title).xalign(0.0).wrap(true).build();
        title.set_wrap_mode(pango::WrapMode::WordChar);
        title.add_css_class("task-title");
        if task.is_completed() {
            title.add_css_class("done");
            strike(&title);
        }
        text.append(&title);

        let meta = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let add_meta = |label: &str, class: Option<&str>| {
            let l = gtk::Label::new(Some(label));
            l.add_css_class("meta");
            if let Some(c) = class {
                l.add_css_class(c);
            }
            meta.append(&l);
        };
        if let Some(due) = &task.due {
            let class = if task.is_completed() {
                None
            } else if due.is_overdue(init.now) {
                Some("overdue")
            } else if due.date == init.now.date() {
                Some("today")
            } else {
                None
            };
            add_meta(&due_label(due, init.now.date()), class);
        }
        if let Some(r) = &task.recurrence {
            add_meta(&format!("↻ {}", r.describe().to_lowercase()), None);
        }
        if let Some((done, total)) = init.progress {
            add_meta(&format!("☑ {done}/{total}"), None);
        }
        if !task.notes.trim().is_empty() {
            add_meta("✎", None);
        }
        if let Some(name) = &init.list_name {
            add_meta(&format!("#{name}"), Some("list"));
        }
        for tag in &task.tags {
            add_meta(&format!("@{tag}"), Some("tag"));
        }
        if meta.first_child().is_some() {
            text.append(&meta);
        }
        content.append(&text);

        if let Some(n) = task.priority.level() {
            let mark = gtk::Label::new(Some(&"!".repeat(4 - n as usize)));
            mark.add_css_class("prio-mark");
            mark.add_css_class(prio_class(task.priority));
            mark.set_valign(gtk::Align::Start);
            content.append(&mark);
        }
        TaskWidgets { revealer, check, content, title }
    }

    fn update_with_view(&mut self, widgets: &mut TaskWidgets, msg: TaskMsg, _: FactorySender<Self>) {
        match msg {
            TaskMsg::Completing => {
                widgets.content.add_css_class("completing");
                strike(&widgets.title);
            }
            TaskMsg::Collapse => widgets.revealer.set_reveal_child(false),
            TaskMsg::Check => widgets.check.set_active(!widgets.check.is_active()),
        }
    }
}

fn strike(label: &gtk::Label) {
    let attrs = pango::AttrList::new();
    attrs.insert(pango::AttrInt::new_strikethrough(true));
    label.set_attributes(Some(&attrs));
}

pub fn prio_class(p: Priority) -> &'static str {
    match p {
        Priority::High => "p1",
        Priority::Medium => "p2",
        Priority::Low => "p3",
        Priority::None => "p0",
    }
}

// --- Listes (barre latérale) ----------------------------------------------

pub struct ListItem {
    list: List,
    count: usize,
}

impl FactoryComponent for ListItem {
    type ParentWidget = gtk::ListBox;
    type CommandOutput = ();
    type Input = ();
    type Output = ();
    type Init = (List, usize);
    type Root = gtk::Box;
    type Widgets = ();
    type Index = DynamicIndex;

    fn init_model((list, count): (List, usize), _: &DynamicIndex, _: FactorySender<Self>) -> Self {
        ListItem { list, count }
    }

    fn init_root(&self) -> gtk::Box {
        gtk::Box::new(gtk::Orientation::Horizontal, 10)
    }

    fn init_widgets(&mut self, _: &DynamicIndex, root: gtk::Box, _: &gtk::ListBoxRow, _: FactorySender<Self>) {
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("dot");
        dot.set_valign(gtk::Align::Center);
        if self.list.is_inbox() {
            dot.add_css_class("inbox");
        } else if let Some(c) = &self.list.color {
            dot.add_css_class(c);
        }
        root.append(&dot);
        let name = gtk::Label::builder().label(&self.list.name).xalign(0.0).hexpand(true).build();
        name.set_ellipsize(pango::EllipsizeMode::End);
        root.append(&name);
        if self.count > 0 {
            let count = gtk::Label::new(Some(&self.count.to_string()));
            count.add_css_class("dim");
            root.append(&count);
        }
    }
}

// --- Aperçu de la saisie rapide ---------------------------------------------

/// Remplit `container` de pastilles décrivant ce que la saisie a compris.
/// Seules les pastilles qui n'étaient pas déjà là apparaissent avec un « pop ».
pub fn fill_chips(container: &gtk::Box, parsed: &Parsed, known_list: impl Fn(&str) -> bool, today: chrono::NaiveDate) {
    let mut before = Vec::new();
    while let Some(child) = container.first_child() {
        if let Some(label) = child.downcast_ref::<gtk::Label>() {
            before.push(label.label().to_string());
        }
        container.remove(&child);
    }
    let chip = |text: &str, class: &str| {
        let l = gtk::Label::new(Some(text));
        l.add_css_class("chip");
        l.add_css_class(class);
        if !before.iter().any(|b| b == text) {
            l.add_css_class("pop");
        }
        container.append(&l);
        l
    };
    if let Some(due) = &parsed.due {
        chip(&due_label(due, today), "date");
    }
    if let Some(r) = &parsed.recurrence {
        chip(&format!("↻ {}", r.describe()), "recurrence");
    }
    if let Some(list) = &parsed.list {
        if known_list(list) {
            chip(&format!("#{list}"), "list");
        } else {
            chip(&format!("#{list} · nouvelle liste"), "list").add_css_class("new");
        }
    }
    for tag in &parsed.tags {
        chip(&format!("@{tag}"), "tag");
    }
    if let Some(n) = parsed.priority.level() {
        let label = match n {
            1 => "!1 haute",
            2 => "!2 moyenne",
            _ => "!3 basse",
        };
        chip(label, "priority");
    }
    container.set_visible(container.first_child().is_some());
}

/// Colore, dans le champ lui-même, les mots que la saisie a reconnus.
pub fn highlight_entry(entry: &gtk::Entry, parsed: &Parsed) {
    let attrs = pango::AttrList::new();
    for span in &parsed.spans {
        let Some((r, g, b)) = theme::span_rgb(span.kind) else { continue };
        let scale = |c: u8| u16::from(c) * 257;
        let mut color = pango::AttrColor::new_foreground(scale(r), scale(g), scale(b));
        color.set_start_index(span.range.start as u32);
        color.set_end_index(span.range.end as u32);
        attrs.insert(color);
    }
    entry.set_attributes(&attrs);
}

/// Le bouton de coche doit rester visible un instant avant que la ligne ne se replie.
pub const COMPLETE_PAUSE: Duration = Duration::from_millis(320);

/// Laisse le temps à une ligne de se replier.
pub fn after_collapse(f: impl FnOnce() + 'static) {
    glib::timeout_add_local_once(Duration::from_millis(anim::ROW_MS as u64 + 40), f);
}
