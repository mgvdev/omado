//! Panneau de détail : modifier une tâche sans quitter la liste.
//!
//! Chaque champ enregistre dès qu'on le valide (Entrée) ou qu'on le quitte.
//! L'échéance se saisit en langage naturel, comme la saisie rapide.

use std::cell::Cell;
use std::rc::Rc;

use chrono::{Datelike, NaiveDate, NaiveTime};
use omado_core::human::{date_label_inline, datetime_label, due_label};
use omado_core::{Due, List, NewTask, Priority, Store, Task, TaskId, now, parse, tr};
use relm4::gtk::{self, gdk, glib, pango, prelude::*};
use relm4::{ComponentParts, ComponentSender, RelmWidgetExt, SimpleComponent};

use crate::anim;
use crate::rows::priority_name;

pub struct Detail {
    store: Rc<Store>,
    task: Option<Task>,
    subtasks: Vec<Task>,
    lists: Vec<List>,
    notes_dirty: bool,
    /// Incrémenté à chaque frappe dans les notes : seul le dernier délai enregistre.
    notes_generation: u64,
    widgets: DetailWidgets,
    input: relm4::Sender<DetailMsg>,
}

#[derive(Debug)]
pub enum DetailMsg {
    Show(TaskId),
    /// Relit la tâche affichée (modifiée ailleurs).
    Reload,
    Close,
    Title(String),
    Schedule(String),
    PickDate(NaiveDate),
    ClearSchedule,
    Remind(bool),
    Priority(u32),
    List(u32),
    Tags(String),
    NotesChanged,
    SaveNotes,
    SaveNotesAfterPause(u64),
    AddSubtask(String),
    ToggleSubtask(TaskId, bool),
    DeleteSubtask(TaskId),
    Delete,
}

#[derive(Debug)]
pub enum DetailOutput {
    Changed,
    Closed,
    /// Tâche supprimée ; l'instantané permet d'annuler.
    Deleted(Vec<Task>),
}

/// Widgets à mettre à jour ; `filling` coupe les signaux pendant qu'on les remplit.
#[derive(Clone)]
struct DetailWidgets {
    body: gtk::Box,
    title: gtk::TextView,
    schedule: gtk::Label,
    recurrence: gtk::Label,
    when: gtk::Entry,
    when_error: gtk::Label,
    calendar: gtk::Calendar,
    remind: gtk::Switch,
    remind_label: gtk::Label,
    priority: gtk::DropDown,
    list_row: gtk::Box,
    list: gtk::DropDown,
    tags: gtk::Entry,
    notes: gtk::TextView,
    subtasks: gtk::Box,
    subtask_entry: gtk::Entry,
    footnote: gtk::Label,
    filling: Rc<Cell<bool>>,
}

/// Libellés de la liste déroulante, dans l'ordre de `Priority::to_db`.
fn priority_choices() -> Vec<String> {
    let none = omado_core::i18n::capitalize(priority_name(Priority::None));
    let levels = [Priority::Low, Priority::Medium, Priority::High]
        .map(|p| format!("!{} · {}", p.level().unwrap_or_default(), priority_name(p)));
    std::iter::once(none).chain(levels).collect()
}

/// Intitulé de champ, en capitales.
fn field(label: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(label.to_uppercase()).xalign(0.0).margin_top(14).build();
    l.add_css_class("field-label");
    l
}

/// Texte d'un champ multiligne ; un saut de ligne collé devient une espace.
fn text_of(view: &gtk::TextView) -> String {
    let buffer = view.buffer();
    buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).replace('\n', " ")
}

/// Enregistre à la sortie du champ.
fn on_leave(widget: &impl IsA<gtk::Widget>, f: impl Fn() + 'static) {
    let focus = gtk::EventControllerFocus::new();
    focus.connect_leave(move |_| f());
    widget.add_controller(focus);
}

impl SimpleComponent for Detail {
    type Init = Rc<Store>;
    type Input = DetailMsg;
    type Output = DetailOutput;
    type Root = gtk::Box;
    type Widgets = ();

    fn init_root() -> gtk::Box {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("detail");
        root.set_width_request(360);
        root
    }

    fn init(store: Rc<Store>, root: gtk::Box, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let filling = Rc::new(Cell::new(false));
        let input = sender.input_sender().clone();
        let send = move |m: DetailMsg| input.emit(m);

        // En-tête : fermer.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_top(8);
        header.set_margin_end(8);
        header.set_margin_start(14);
        let head_label = gtk::Label::builder().label(tr!("Details").to_uppercase()).xalign(0.0).hexpand(true).build();
        head_label.add_css_class("field-label");
        header.append(&head_label);
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("flat");
        close.set_tooltip_text(Some(tr!("Close (Esc)")));
        {
            let send = send.clone();
            close.connect_clicked(move |_| send(DetailMsg::Close));
        }
        header.append(&close);
        root.append(&header);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
        body.set_margin_start(14);
        body.set_margin_end(14);
        body.set_margin_bottom(14);
        let scroller =
            gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&body).build();
        root.append(&scroller);

        // Titre : passe à la ligne s'il est long ; Entrée valide sans ajouter de saut de ligne.
        let title = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).accepts_tab(false).build();
        title.add_css_class("detail-title");
        {
            let send = send.clone();
            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(move |k, key, _, _| match key {
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    if let Some(view) = k.widget().and_downcast::<gtk::TextView>() {
                        send(DetailMsg::Title(text_of(&view)));
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            });
            title.add_controller(keys);
        }
        {
            let send = send.clone();
            let view = title.clone();
            on_leave(&title, move || send(DetailMsg::Title(text_of(&view))));
        }
        body.append(&title);

        // Échéance
        body.append(&field(tr!("When")));
        let schedule = gtk::Label::builder().xalign(0.0).wrap(true).build();
        schedule.add_css_class("schedule");
        body.append(&schedule);
        let recurrence = gtk::Label::builder().xalign(0.0).build();
        recurrence.add_css_class("recurrence-label");
        body.append(&recurrence);

        let when_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        // Translators: date examples that quick entry understands in your language (a test checks them).
        let examples = tr!("tomorrow 9am, friday, every monday…");
        let when = gtk::Entry::builder().placeholder_text(examples).hexpand(true).build();
        {
            let send = send.clone();
            when.connect_activate(move |e| send(DetailMsg::Schedule(e.text().into())));
        }
        when_row.append(&when);
        let calendar = gtk::Calendar::new();
        {
            let send = send.clone();
            let filling = filling.clone();
            calendar.connect_day_selected(move |c| {
                if filling.get() {
                    return;
                }
                let d = c.date();
                if let Some(date) = NaiveDate::from_ymd_opt(d.year(), d.month() as u32, d.day_of_month() as u32) {
                    send(DetailMsg::PickDate(date));
                }
            });
        }
        let pick = gtk::MenuButton::builder()
            .icon_name("x-office-calendar-symbolic")
            .tooltip_text(tr!("Pick a date"))
            .popover(&gtk::Popover::builder().child(&calendar).build())
            .build();
        pick.add_css_class("flat");
        when_row.append(&pick);
        let clear = gtk::Button::from_icon_name("edit-clear-symbolic");
        clear.add_css_class("flat");
        clear.set_tooltip_text(Some(tr!("Remove the due date")));
        {
            let send = send.clone();
            clear.connect_clicked(move |_| send(DetailMsg::ClearSchedule));
        }
        when_row.append(&clear);
        body.append(&when_row);
        let when_error = gtk::Label::builder()
            // Translators: date examples that quick entry understands in your language (a test checks them).
            .label(tr!("Didn't get that. Try “tomorrow 9am”, “Oct 15” or “every monday”."))
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .build();
        when_error.add_css_class("parse-error");
        body.append(&when_error);

        // Rappel
        let remind_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        remind_row.set_margin_top(6);
        let remind_label = gtk::Label::builder().xalign(0.0).hexpand(true).build();
        remind_row.append(&remind_label);
        let remind = gtk::Switch::builder().valign(gtk::Align::Center).build();
        {
            let send = send.clone();
            let filling = filling.clone();
            remind.connect_active_notify(move |s| {
                if !filling.get() {
                    send(DetailMsg::Remind(s.is_active()));
                }
            });
        }
        remind_row.append(&remind);
        body.append(&remind_row);

        // Priorité et liste
        body.append(&field(tr!("Priority")));
        let choices = priority_choices();
        let priority = gtk::DropDown::from_strings(&choices.iter().map(String::as_str).collect::<Vec<_>>());
        {
            let send = send.clone();
            let filling = filling.clone();
            priority.connect_selected_notify(move |d| {
                if !filling.get() {
                    send(DetailMsg::Priority(d.selected()));
                }
            });
        }
        body.append(&priority);

        let list_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        list_row.append(&field(tr!("List")));
        let list = gtk::DropDown::from_strings(&[]);
        {
            let send = send.clone();
            let filling = filling.clone();
            list.connect_selected_notify(move |d| {
                if !filling.get() {
                    send(DetailMsg::List(d.selected()));
                }
            });
        }
        list_row.append(&list);
        body.append(&list_row);

        // Étiquettes
        body.append(&field(tr!("Tags")));
        let tags = gtk::Entry::builder().placeholder_text(tr!("home urgent…")).build();
        {
            let send = send.clone();
            tags.connect_activate(move |e| send(DetailMsg::Tags(e.text().into())));
        }
        {
            let send = send.clone();
            let e = tags.clone();
            on_leave(&tags, move || send(DetailMsg::Tags(e.text().into())));
        }
        body.append(&tags);

        // Notes
        body.append(&field(tr!("Notes")));
        let notes = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).height_request(90).build();
        notes.add_css_class("notes");
        {
            let send = send.clone();
            let filling = filling.clone();
            notes.buffer().connect_changed(move |_| {
                if !filling.get() {
                    send(DetailMsg::NotesChanged);
                }
            });
        }
        {
            let send = send.clone();
            on_leave(&notes, move || send(DetailMsg::SaveNotes));
        }
        body.append(&notes);

        // Sous-tâches
        body.append(&field(tr!("Subtasks")));
        let subtasks = gtk::Box::new(gtk::Orientation::Vertical, 2);
        body.append(&subtasks);
        let subtask_entry = gtk::Entry::builder().placeholder_text(tr!("＋ Add a subtask")).build();
        {
            let send = send.clone();
            subtask_entry.connect_activate(move |e| {
                send(DetailMsg::AddSubtask(e.text().into()));
                e.set_text("");
            });
        }
        body.append(&subtask_entry);

        // Pied : dates et suppression
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        footer.set_margin_top(18);
        let footnote = gtk::Label::builder().xalign(0.0).hexpand(true).wrap(true).build();
        footnote.add_css_class("footnote");
        footer.append(&footnote);
        let delete = gtk::Button::with_label(tr!("Delete"));
        delete.add_css_class("flat");
        delete.add_css_class("danger");
        {
            let send = send.clone();
            delete.connect_clicked(move |_| send(DetailMsg::Delete));
        }
        footer.append(&delete);
        body.append(&footer);

        let widgets = DetailWidgets {
            body: body.clone(),
            title,
            schedule,
            recurrence,
            when,
            when_error,
            calendar,
            remind,
            remind_label,
            priority,
            list_row,
            list,
            tags,
            notes,
            subtasks,
            subtask_entry,
            footnote,
            filling,
        };
        let input = sender.input_sender().clone();
        let model = Detail {
            store,
            task: None,
            subtasks: Vec::new(),
            lists: Vec::new(),
            notes_dirty: false,
            notes_generation: 0,
            widgets,
            input,
        };
        ComponentParts { model, widgets: () }
    }

    fn update(&mut self, msg: DetailMsg, sender: ComponentSender<Self>) {
        match msg {
            DetailMsg::Show(id) => {
                self.flush_notes(&sender);
                self.load(id);
                self.widgets.when.set_text("");
                self.widgets.when_error.set_visible(false);
                self.fill();
                anim::replay(&self.widgets.body, "enter");
            }
            DetailMsg::Reload => {
                if let Some(id) = self.task.as_ref().map(|t| t.id) {
                    if self.notes_dirty {
                        return;
                    }
                    self.load(id);
                    self.fill();
                }
            }
            DetailMsg::Close => {
                self.flush_notes(&sender);
                self.task = None;
                let _ = sender.output(DetailOutput::Closed);
            }
            DetailMsg::Title(title) => {
                let title = title.trim().to_string();
                self.edit(&sender, |t| {
                    if title.is_empty() || t.title == title {
                        return false;
                    }
                    t.title = title;
                    true
                });
            }
            DetailMsg::Schedule(text) => {
                let parsed = parse(&text, now());
                if parsed.due.is_none() && parsed.recurrence.is_none() {
                    self.widgets.when_error.set_visible(!text.trim().is_empty());
                    return;
                }
                self.widgets.when_error.set_visible(false);
                self.widgets.when.set_text("");
                self.edit(&sender, |t| {
                    if parsed.recurrence.is_some() {
                        t.recurrence = parsed.recurrence;
                    }
                    set_due(t, parsed.due);
                    true
                });
            }
            DetailMsg::PickDate(date) => {
                self.edit(&sender, |t| {
                    let time = t.due.and_then(|d| d.time);
                    if t.due.map(|d| d.date) == Some(date) {
                        return false;
                    }
                    set_due(t, Some(Due { date, time }));
                    true
                });
            }
            DetailMsg::ClearSchedule => {
                self.edit(&sender, |t| {
                    t.due = None;
                    t.recurrence = None;
                    t.remind_at = None;
                    true
                });
            }
            DetailMsg::Remind(on) => {
                self.edit(&sender, |t| {
                    t.remind_at = match (on, t.due) {
                        (false, _) | (true, None) => None,
                        (true, Some(due)) => Some(due.date.and_time(due.time.unwrap_or(default_reminder_time()))),
                    };
                    true
                });
            }
            DetailMsg::Priority(i) => {
                self.edit(&sender, |t| {
                    t.priority = Priority::from_db(i as i64);
                    true
                });
            }
            DetailMsg::List(i) => {
                let Some(list) = self.lists.get(i as usize).map(|l| l.id) else { return };
                self.edit(&sender, |t| {
                    if t.list_id == list {
                        return false;
                    }
                    t.list_id = list;
                    true
                });
            }
            DetailMsg::Tags(text) => {
                let mut tags: Vec<String> = text
                    .split([' ', ','])
                    .map(|s| s.trim().trim_start_matches(['@', '#']).to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect();
                tags.sort();
                tags.dedup();
                self.edit(&sender, |t| {
                    if t.tags == tags {
                        return false;
                    }
                    t.tags = tags;
                    true
                });
            }
            DetailMsg::NotesChanged => {
                self.notes_dirty = true;
                self.notes_generation += 1;
                let (s, generation) = (self.input.clone(), self.notes_generation);
                glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
                    s.emit(DetailMsg::SaveNotesAfterPause(generation))
                });
            }
            DetailMsg::SaveNotesAfterPause(generation) => {
                if generation == self.notes_generation {
                    self.flush_notes(&sender);
                }
            }
            DetailMsg::SaveNotes => self.flush_notes(&sender),
            DetailMsg::AddSubtask(text) => {
                let Some(parent) = &self.task else { return };
                let mut new: NewTask = parse(&text, now()).into_new_task();
                new.parent_id = Some(parent.id);
                if new.title.trim().is_empty() {
                    new.title = text.trim().to_string();
                }
                if report(self.store.add_task(new)) {
                    self.reload_subtasks();
                    let _ = sender.output(DetailOutput::Changed);
                }
            }
            DetailMsg::ToggleSubtask(id, done) => {
                let ok = if done { self.store.complete(id, now()).map(drop) } else { self.store.uncomplete(id) };
                if report(ok) {
                    self.reload_subtasks();
                    let _ = sender.output(DetailOutput::Changed);
                }
            }
            DetailMsg::DeleteSubtask(id) => {
                if report(self.store.delete_task(id)) {
                    self.reload_subtasks();
                    let _ = sender.output(DetailOutput::Changed);
                }
            }
            DetailMsg::Delete => {
                let Some(task) = self.task.take() else { return };
                let Ok(snapshot) = self.store.snapshot(task.id) else { return };
                if report(self.store.delete_task(task.id)) {
                    self.notes_dirty = false;
                    let _ = sender.output(DetailOutput::Deleted(snapshot));
                }
            }
        }
    }
}

/// Heure du rappel quand l'échéance n'a pas d'heure.
fn default_reminder_time() -> NaiveTime {
    NaiveTime::from_hms_opt(9, 0, 0).expect("heure valide")
}

/// Change l'échéance ; le rappel suit l'heure d'échéance.
fn set_due(t: &mut Task, due: Option<Due>) {
    let had_reminder = t.remind_at.is_some();
    t.due = due;
    t.remind_at = match due {
        Some(Due { date, time: Some(time) }) => Some(date.and_time(time)),
        Some(Due { date, time: None }) if had_reminder => Some(date.and_time(default_reminder_time())),
        _ => None,
    };
}

fn report<T>(result: omado_core::store::Result<T>) -> bool {
    match result {
        Ok(_) => true,
        Err(e) => {
            eprintln!("{}", tr!("omado: {error}", error = e));
            false
        }
    }
}

impl Detail {
    fn load(&mut self, id: TaskId) {
        self.task = self.store.task(id).ok();
        self.lists = self.store.lists().unwrap_or_default();
        self.notes_dirty = false;
        self.reload_subtasks_data();
    }

    fn reload_subtasks_data(&mut self) {
        self.subtasks = match &self.task {
            Some(t) => self.store.subtasks(t.id).unwrap_or_default(),
            None => Vec::new(),
        };
    }

    fn reload_subtasks(&mut self) {
        self.reload_subtasks_data();
        self.fill_subtasks();
    }

    /// Applique une modification à la tâche et l'enregistre si elle a changé.
    fn edit(&mut self, sender: &ComponentSender<Self>, change: impl FnOnce(&mut Task) -> bool) {
        let Some(task) = &mut self.task else { return };
        if !change(task) {
            return;
        }
        if report(self.store.update_task(task)) {
            let _ = sender.output(DetailOutput::Changed);
        }
        // Relire : l'ancrage des récurrences peut avoir changé.
        let id = task.id;
        let notes_dirty = self.notes_dirty;
        self.task = self.store.task(id).ok();
        self.notes_dirty = notes_dirty;
        self.fill();
    }

    fn flush_notes(&mut self, sender: &ComponentSender<Self>) {
        if !self.notes_dirty {
            return;
        }
        self.notes_dirty = false;
        let buffer = self.widgets.notes.buffer();
        let notes = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
        self.edit(sender, |t| {
            if t.notes == notes {
                return false;
            }
            t.notes = notes;
            true
        });
    }

    fn fill(&self) {
        let Some(task) = &self.task else { return };
        let w = &self.widgets;
        w.filling.set(true);
        let today = now().date();

        if text_of(&w.title) != task.title {
            w.title.buffer().set_text(&task.title);
        }
        match &task.due {
            Some(due) => {
                w.schedule.set_label(&due_label(due, today));
                w.schedule.set_class_active("overdue", due.is_overdue(now()) && !task.is_completed());
                let (y, m, d) = (due.date.year(), due.date.month() as i32, due.date.day() as i32);
                if let Ok(date) = glib::DateTime::from_local(y, m, d, 0, 0, 0.0) {
                    w.calendar.select_day(&date);
                }
            }
            None => {
                w.schedule.set_label(tr!("No due date"));
                w.schedule.remove_css_class("overdue");
            }
        }
        match &task.recurrence {
            Some(r) => {
                w.recurrence.set_label(&format!("↻ {}", r.describe()));
                w.recurrence.set_visible(true);
            }
            None => w.recurrence.set_visible(false),
        }
        w.remind.set_active(task.remind_at.is_some());
        w.remind.set_sensitive(task.due.is_some());
        w.remind_label.set_label(&match task.remind_at {
            Some(at) => tr!("Reminder · {when}", when = datetime_label(at.date(), at.time(), today)),
            None if task.due.is_some() => tr!("Remind me").to_string(),
            None => tr!("Reminder: add a due date first").to_string(),
        });

        w.priority.set_selected(task.priority.to_db() as u32);

        let names: Vec<&str> = self.lists.iter().map(List::display_name).collect();
        w.list.set_model(Some(&gtk::StringList::new(&names)));
        if let Some(i) = self.lists.iter().position(|l| l.id == task.list_id) {
            w.list.set_selected(i as u32);
        }
        w.list_row.set_visible(task.parent_id.is_none());

        let tags = task.tags.join(" ");
        if w.tags.text() != tags {
            w.tags.set_text(&tags);
        }
        let buffer = w.notes.buffer();
        if !self.notes_dirty && buffer.text(&buffer.start_iter(), &buffer.end_iter(), false) != task.notes {
            buffer.set_text(&task.notes);
        }
        w.subtask_entry.set_visible(task.parent_id.is_none());
        w.footnote.set_label(&tr!(
            "Created {created} · modified {modified}",
            created = date_label_inline(task.created_at.date(), today),
            modified = date_label_inline(task.updated_at.date(), today),
        ));
        w.filling.set(false);
        self.fill_subtasks();
    }

    fn fill_subtasks(&self) {
        let input = &self.input;
        let container = &self.widgets.subtasks;
        while let Some(child) = container.first_child() {
            container.remove(&child);
        }
        for sub in &self.subtasks {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            // Libellé qui passe à la ligne : une longue sous-tâche n'élargit pas le panneau.
            let label = gtk::Label::builder()
                .label(&sub.title)
                .wrap(true)
                .xalign(0.0)
                .max_width_chars(20)
                .hexpand(true)
                .build();
            label.set_wrap_mode(pango::WrapMode::WordChar);
            let check = gtk::CheckButton::new();
            check.set_child(Some(&label));
            check.set_active(sub.is_completed());
            check.set_hexpand(true);
            check.add_css_class("task-check");
            let (id, send) = (sub.id, input.clone());
            check.connect_toggled(move |c| send.emit(DetailMsg::ToggleSubtask(id, c.is_active())));
            row.append(&check);
            if let Some(due) = &sub.due {
                let l = gtk::Label::new(Some(&due_label(due, now().date())));
                l.add_css_class("meta");
                row.append(&l);
            }
            let remove = gtk::Button::from_icon_name("window-close-symbolic");
            remove.add_css_class("flat");
            remove.set_tooltip_text(Some(tr!("Delete the subtask")));
            let send = input.clone();
            remove.connect_clicked(move |_| send.emit(DetailMsg::DeleteSubtask(id)));
            row.append(&remove);
            container.append(&row);
        }
    }
}
