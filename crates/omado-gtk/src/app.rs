//! Fenêtre principale : barre latérale, liste des tâches, panneau de détail.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use chrono::{NaiveDateTime, Timelike};
use omado_core::human::{date_label, day_heading, due_label};
use omado_core::{Completion, Counts, Due, INBOX_ID, List, ListId, Store, Task, TaskId, View, now, parse, tr};
use relm4::factory::FactoryVecDeque;
use relm4::gtk::{self, gdk, glib, prelude::*};
use relm4::prelude::*;

use crate::anim;
use crate::detail::{Detail, DetailMsg, DetailOutput};
use crate::quick::{QuickAdd, QuickOutput};
use crate::rows::{
    COMPLETE_PAUSE, ListItem, TaskInit, TaskItem, TaskMsg, TaskOutput, after_collapse, fill_chips, highlight_entry,
};

pub static BROKER: relm4::MessageBroker<AppMsg> = relm4::MessageBroker::new();

const LIST_COLORS: [&str; 8] = ["accent", "red", "orange", "yellow", "green", "cyan", "blue", "magenta"];

#[derive(Debug)]
pub enum AppMsg {
    ShowMain,
    ShowQuick,
    QuickClosed,
    Refresh,
    Tick,
    Select(View),
    SelectList(i32),
    SelectTag(i32),
    Search(String),
    QuickChanged(String),
    QuickAdd(String),
    RowSelected(Option<i32>),
    Toggle(TaskId, bool),
    /// Replie la ligne d'une tâche qui quitte la vue, puis rafraîchit.
    Collapse(TaskId),
    Open(i32),
    ToggleSelected,
    DeleteSelected,
    OpenSelected,
    MoveSelected(i32),
    SelectNext(i32),
    Detail(DetailOutput),
    Undo,
    DismissUndo,
    /// Fin du délai d'annulation ; ignoré si une annulation plus récente a été proposée.
    ExpireUndo(u64),
    NewList(String),
    RenameList(String),
    SetListColor(&'static str),
    DeleteList,
    FocusQuick,
    FocusSearch,
    Escape,
}

struct Undo {
    message: String,
    snapshot: Vec<Task>,
}

/// Un bouton de vue intelligente (Aujourd'hui, Planifié…).
struct Tile {
    view: View,
    button: gtk::Button,
    count: gtk::Label,
}

pub struct App {
    store: Rc<Store>,
    view: View,
    /// Vue à retrouver quand on vide la recherche.
    before_search: View,
    lists: Vec<List>,
    counts: Counts,
    tags: Vec<(String, usize)>,
    tasks: FactoryVecDeque<TaskItem>,
    sidebar_lists: FactoryVecDeque<ListItem>,
    tiles: Vec<Tile>,
    tags_box: gtk::ListBox,
    title_label: gtk::Label,
    chips: gtk::Box,
    quick_entry: gtk::Entry,
    search_entry: gtk::SearchEntry,
    rename_entry: gtk::Entry,
    sections: Rc<RefCell<Vec<Option<String>>>>,
    empty_box: gtk::Box,
    empty_icon: gtk::Image,
    undo_timer: gtk::Box,
    /// Tâches affichées au dernier rafraîchissement, pour repérer les nouvelles.
    shown_ids: HashSet<TaskId>,
    shown_view: Option<View>,
    /// À faire apparaître (dépliée, éclairée) au prochain rafraîchissement.
    fresh: HashSet<TaskId>,
    /// À éclairer sur place au prochain rafraîchissement.
    flash: HashSet<TaskId>,
    selected: Option<TaskId>,
    detail: Controller<Detail>,
    detail_open: bool,
    undo: Option<Undo>,
    undo_generation: u64,
    quick: Option<Controller<QuickAdd>>,
    data_version: i64,
    last_minute: u32,
    // Affichage
    title: String,
    count_text: String,
    empty: bool,
    empty_title: &'static str,
    empty_sub: &'static str,
    list_menu: bool,
}

#[relm4::component(pub)]
impl Component for App {
    type Init = Rc<Store>;
    type Input = AppMsg;
    type Output = ();
    type CommandOutput = ();

    view! {
        #[root]
        window = gtk::ApplicationWindow {
            set_title: Some("Omado"),
            set_default_size: (1120, 740),
            add_css_class: "omado",
            // Hyprland dessine bordures et arrondis : pas de barre de titre GTK.
            set_decorated: false,
            set_visible: false,

            gtk::Paned {
                set_position: 270,
                set_shrink_start_child: false,
                set_resize_start_child: false,

                #[wrap(Some)]
                set_start_child = &gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    add_css_class: "sidebar",
                    set_width_request: 230,

                    #[local_ref]
                    search_entry -> gtk::SearchEntry {
                        add_css_class: "search",
                        set_placeholder_text: Some(tr!("Search (/)")),
                        connect_search_changed[sender] => move |e| sender.input(AppMsg::Search(e.text().into())),
                        connect_stop_search[sender] => move |e| {
                            e.set_text("");
                            sender.input(AppMsg::Escape);
                        },
                    },

                    #[local_ref]
                    tiles_grid -> gtk::Grid {
                        set_column_homogeneous: true,
                        set_row_spacing: 8,
                        set_column_spacing: 8,
                        set_margin_start: 10,
                        set_margin_end: 10,
                        set_margin_top: 6,
                    },

                    gtk::ScrolledWindow {
                        set_hscrollbar_policy: gtk::PolicyType::Never,
                        set_vexpand: true,

                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,

                            gtk::Box {
                                set_margin_end: 8,
                                gtk::Label {
                                    set_label: &tr!("Lists").to_uppercase(),
                                    add_css_class: "sidebar-title",
                                    set_hexpand: true,
                                    set_xalign: 0.0,
                                },
                                gtk::MenuButton {
                                    set_icon_name: "list-add-symbolic",
                                    add_css_class: "flat",
                                    set_valign: gtk::Align::End,
                                    set_tooltip_text: Some(tr!("New list")),
                                    #[wrap(Some)]
                                    set_popover = &gtk::Popover {
                                        gtk::Entry {
                                            set_placeholder_text: Some(tr!("List name")),
                                            set_width_chars: 22,
                                            connect_activate[sender] => move |e| {
                                                sender.input(AppMsg::NewList(e.text().into()));
                                                e.set_text("");
                                                close_popover(e);
                                            },
                                        },
                                    },
                                },
                            },

                            #[local_ref]
                            lists_box -> gtk::ListBox {
                                add_css_class: "nav",
                                set_selection_mode: gtk::SelectionMode::Single,
                                connect_row_activated[sender] => move |_, row| sender.input(AppMsg::SelectList(row.index())),
                            },

                            gtk::Label {
                                set_label: &tr!("Tags").to_uppercase(),
                                add_css_class: "sidebar-title",
                                set_xalign: 0.0,
                                #[watch]
                                set_visible: !model.tags.is_empty(),
                            },

                            #[local_ref]
                            tags_box -> gtk::ListBox {
                                add_css_class: "nav",
                                set_selection_mode: gtk::SelectionMode::Single,
                                connect_row_activated[sender] => move |_, row| sender.input(AppMsg::SelectTag(row.index())),
                            },
                        },
                    },
                },

                #[wrap(Some)]
                set_end_child = &gtk::Overlay {
                    #[wrap(Some)]
                    set_child = &gtk::Box {
                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_hexpand: true,
                            set_spacing: 8,
                            set_margin_top: 18,
                            set_margin_start: 22,
                            set_margin_end: 22,

                            gtk::Box {
                                set_spacing: 12,
                                #[local_ref]
                                title_label -> gtk::Label {
                                    add_css_class: "view-title",
                                    set_xalign: 0.0,
                                    set_ellipsize: gtk::pango::EllipsizeMode::End,
                                    #[watch]
                                    set_label: &model.title,
                                },
                                gtk::Label {
                                    add_css_class: "view-count",
                                    set_hexpand: true,
                                    set_xalign: 1.0,
                                    #[watch]
                                    set_label: &model.count_text,
                                },
                                gtk::MenuButton {
                                    set_icon_name: "view-more-symbolic",
                                    add_css_class: "flat",
                                    set_valign: gtk::Align::Center,
                                    set_tooltip_text: Some(tr!("List options")),
                                    #[watch]
                                    set_visible: model.list_menu,
                                    #[wrap(Some)]
                                    set_popover = &gtk::Popover {
                                        gtk::Box {
                                            set_orientation: gtk::Orientation::Vertical,
                                            set_spacing: 10,
                                            set_margin_all: 6,

                                            gtk::Label { set_label: &tr!("Rename").to_uppercase(), add_css_class: "field-label", set_xalign: 0.0 },
                                            #[local_ref]
                                            rename_entry -> gtk::Entry {
                                                set_width_chars: 24,
                                                connect_activate[sender] => move |e| {
                                                    sender.input(AppMsg::RenameList(e.text().into()));
                                                    close_popover(e);
                                                },
                                            },
                                            gtk::Label { set_label: &tr!("Color").to_uppercase(), add_css_class: "field-label", set_xalign: 0.0 },
                                            #[local_ref]
                                            colors_box -> gtk::Box { set_spacing: 6 },
                                            #[local_ref]
                                            delete_list_button -> gtk::Button {
                                                add_css_class: "flat",
                                                add_css_class: "danger",
                                            },
                                        },
                                    },
                                },
                            },

                            #[local_ref]
                            quick_entry -> gtk::Entry {
                                add_css_class: "quick-add",
                                // Translators: quick entry understands English and French only: keep the example in English.
                                set_placeholder_text: Some(tr!("New task… e.g. “Call Paul tomorrow 9am #personal @phone !1”  (n)")),
                                connect_changed[sender] => move |e| sender.input(AppMsg::QuickChanged(e.text().into())),
                                connect_activate[sender] => move |e| sender.input(AppMsg::QuickAdd(e.text().into())),
                            },

                            #[local_ref]
                            chips -> gtk::Box {
                                set_spacing: 6,
                                set_visible: false,
                            },

                            gtk::ScrolledWindow {
                                set_hscrollbar_policy: gtk::PolicyType::Never,
                                set_vexpand: true,
                                set_margin_top: 6,
                                #[watch]
                                set_visible: !model.empty,

                                #[local_ref]
                                tasks_box -> gtk::ListBox {
                                    add_css_class: "tasks",
                                    set_selection_mode: gtk::SelectionMode::Single,
                                    set_activate_on_single_click: false,
                                    connect_row_activated[sender] => move |_, row| sender.input(AppMsg::Open(row.index())),
                                    connect_row_selected[sender] => move |_, row| {
                                        sender.input(AppMsg::RowSelected(row.map(|r| r.index())));
                                    },
                                },
                            },

                            #[local_ref]
                            empty_box -> gtk::Box {
                                set_vexpand: true,
                                set_valign: gtk::Align::Center,
                                #[watch]
                                set_visible: model.empty,
                                #[local_ref]
                                empty_icon -> gtk::Image {
                                    add_css_class: "empty-icon",
                                },
                                gtk::Label {
                                    add_css_class: "empty-title",
                                    #[watch]
                                    set_label: model.empty_title,
                                },
                                gtk::Label {
                                    add_css_class: "empty-sub",
                                    #[watch]
                                    set_label: model.empty_sub,
                                },
                            },
                        },

                        gtk::Revealer {
                            set_transition_type: gtk::RevealerTransitionType::SlideLeft,
                            set_transition_duration: 240,
                            // Sinon, replié, il réclame quand même la moitié de la largeur.
                            set_hexpand: false,
                            #[watch]
                            set_reveal_child: model.detail_open,
                            #[local_ref]
                            detail_widget -> gtk::Box {},
                        },
                    },

                    add_overlay = &gtk::Revealer {
                        set_valign: gtk::Align::End,
                        set_halign: gtk::Align::Center,
                        set_transition_type: gtk::RevealerTransitionType::SlideUp,
                        #[watch]
                        set_reveal_child: model.undo.is_some(),

                        gtk::Box {
                            add_css_class: "undo-bar",
                            set_orientation: gtk::Orientation::Vertical,
                            gtk::Box {
                                set_spacing: 10,
                                gtk::Label {
                                    #[watch]
                                    set_label: model.undo.as_ref().map_or("", |u| u.message.as_str()),
                                },
                                gtk::Button {
                                    set_label: tr!("Undo (u)"),
                                    add_css_class: "flat",
                                    connect_clicked => AppMsg::Undo,
                                },
                                gtk::Button {
                                    set_icon_name: "window-close-symbolic",
                                    add_css_class: "flat",
                                    connect_clicked => AppMsg::DismissUndo,
                                },
                            },
                            #[local_ref]
                            undo_timer -> gtk::Box {
                                add_css_class: "undo-timer",
                            },
                        },
                    },
                },
            },
        }
    }

    fn init(store: Rc<Store>, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let tasks =
            FactoryVecDeque::builder().launch(gtk::ListBox::default()).forward(sender.input_sender(), |o| match o {
                TaskOutput::Toggle(id, done) => AppMsg::Toggle(id, done),
            });
        let sidebar_lists = FactoryVecDeque::builder().launch(gtk::ListBox::default()).detach();
        let detail = Detail::builder().launch(store.clone()).forward(sender.input_sender(), AppMsg::Detail);

        let search_entry = gtk::SearchEntry::new();
        let tiles_grid = gtk::Grid::new();
        let tiles = build_tiles(&tiles_grid, &sender);
        let tags_box = gtk::ListBox::new();
        let title_label = gtk::Label::new(None);
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let quick_entry = gtk::Entry::new();
        let rename_entry = gtk::Entry::new();
        let empty_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let empty_icon = gtk::Image::new();
        let undo_timer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let colors_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for color in LIST_COLORS {
            let b = gtk::Button::new();
            b.add_css_class("flat");
            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.add_css_class("dot");
            dot.add_css_class(color);
            dot.set_size_request(16, 16);
            b.set_child(Some(&dot));
            let s = sender.input_sender().clone();
            b.connect_clicked(move |_| s.emit(AppMsg::SetListColor(color)));
            colors_box.append(&b);
        }
        let delete_list_button = gtk::Button::with_label(tr!("Delete list…"));
        {
            // Deux clics : la suppression emporte les tâches de la liste.
            let s = sender.input_sender().clone();
            delete_list_button.connect_clicked(move |b| {
                let confirm = tr!("Confirm: delete with its tasks");
                if b.label().is_some_and(|l| l == confirm) {
                    b.set_label(tr!("Delete list…"));
                    s.emit(AppMsg::DeleteList);
                } else {
                    b.set_label(confirm);
                }
            });
        }

        let data_version = store.data_version().unwrap_or(0);
        let mut model = App {
            store,
            view: View::Today,
            before_search: View::Today,
            lists: Vec::new(),
            counts: Counts::default(),
            tags: Vec::new(),
            tasks,
            sidebar_lists,
            tiles,
            tags_box: tags_box.clone(),
            title_label: title_label.clone(),
            chips: chips.clone(),
            quick_entry: quick_entry.clone(),
            search_entry: search_entry.clone(),
            rename_entry: rename_entry.clone(),
            sections: Rc::default(),
            empty_box: empty_box.clone(),
            empty_icon: empty_icon.clone(),
            undo_timer: undo_timer.clone(),
            shown_ids: HashSet::new(),
            shown_view: None,
            fresh: HashSet::new(),
            flash: HashSet::new(),
            selected: None,
            detail,
            detail_open: false,
            undo: None,
            undo_generation: 0,
            quick: None,
            data_version,
            last_minute: now().minute(),
            title: String::new(),
            count_text: String::new(),
            empty: false,
            empty_title: "",
            empty_sub: "",
            list_menu: false,
        };

        let tasks_box = model.tasks.widget().clone();
        let lists_box = model.sidebar_lists.widget().clone();
        let detail_widget = model.detail.widget().clone();
        let sections = model.sections.clone();
        tasks_box.set_header_func(move |row, before| section_header(&sections.borrow(), row, before));

        model.refresh();
        let widgets = view_output!();
        install_shortcuts(&root, &sender);
        {
            let s = sender.input_sender().clone();
            glib::timeout_add_seconds_local(2, move || {
                s.emit(AppMsg::Tick);
                glib::ControlFlow::Continue
            });
        }
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: AppMsg, sender: ComponentSender<Self>, window: &Self::Root) {
        match msg {
            AppMsg::ShowMain => {
                window.present();
                // Sinon le champ de recherche prend le focus et avale les raccourcis à une lettre.
                self.focus_selected_row();
            }
            AppMsg::ShowQuick => match &self.quick {
                Some(q) => {
                    q.widget().present();
                }
                None => {
                    let q =
                        QuickAdd::builder().launch(self.store.clone()).forward(sender.input_sender(), |o| match o {
                            QuickOutput::Added => AppMsg::Refresh,
                            QuickOutput::Closed => AppMsg::QuickClosed,
                        });
                    if let Some(app) = window.application() {
                        app.add_window(q.widget());
                    }
                    self.quick = Some(q);
                }
            },
            AppMsg::QuickClosed => {
                if let Some(q) = self.quick.take() {
                    q.widget().destroy();
                }
                // Lancée juste pour la saisie rapide : on s'en va.
                if !window.is_visible()
                    && let Some(app) = window.application()
                {
                    app.quit();
                }
            }
            AppMsg::Refresh => self.refresh(),
            AppMsg::Tick => {
                let version = self.store.data_version().unwrap_or(self.data_version);
                let minute = now().minute();
                if version != self.data_version {
                    self.data_version = version;
                    self.refresh();
                    self.detail.emit(DetailMsg::Reload);
                } else if minute != self.last_minute {
                    // Les retards et « Aujourd'hui » dépendent de l'heure.
                    self.last_minute = minute;
                    self.refresh();
                }
            }
            AppMsg::Select(view) => self.select(view),
            AppMsg::SelectList(i) => {
                if let Some(list) = self.lists.get(i as usize) {
                    self.select(View::List(list.id));
                }
            }
            AppMsg::SelectTag(i) => {
                if let Some((tag, _)) = self.tags.get(i as usize) {
                    self.select(View::Tag(tag.clone()));
                }
            }
            AppMsg::Search(q) => {
                if q.trim().is_empty() {
                    if matches!(self.view, View::Search(_)) {
                        let back = self.before_search.clone();
                        self.select(back);
                    }
                } else {
                    if !matches!(self.view, View::Search(_)) {
                        self.before_search = self.view.clone();
                    }
                    self.view = View::Search(q);
                    self.refresh();
                }
            }
            AppMsg::QuickChanged(text) => {
                let parsed = self.contextual_parse(&text);
                let store = &self.store;
                fill_chips(&self.chips, &parsed, |n| store.find_list(n).ok().flatten().is_some(), now().date());
                highlight_entry(&self.quick_entry, &parse(&text, now()));
                if text.trim().is_empty() {
                    self.chips.set_visible(false);
                }
            }
            AppMsg::QuickAdd(text) => {
                let parsed = self.contextual_parse(&text);
                let default_list = match self.view {
                    View::List(id) => Some(id),
                    _ => None,
                };
                match self.store.add_parsed(parsed, default_list) {
                    Ok(task) => {
                        self.quick_entry.set_text("");
                        self.selected = Some(task.id);
                        self.refresh();
                    }
                    Err(omado_core::Error::EmptyTitle) => {}
                    Err(e) => eprintln!("{}", tr!("omado: {error}", error = e)),
                }
            }
            AppMsg::RowSelected(index) => {
                self.selected = index.and_then(|i| self.task_at(i)).map(|t| t.id);
            }
            AppMsg::Toggle(id, done) => self.toggle(id, done, &sender),
            AppMsg::Collapse(id) => {
                if let Some(i) = self.tasks.iter().position(|item| item.task.id == id) {
                    self.tasks.send(i, TaskMsg::Collapse);
                }
                let s = sender.input_sender().clone();
                after_collapse(move || s.emit(AppMsg::Refresh));
            }
            AppMsg::ToggleSelected => {
                // En passant par la case, `x` joue la même animation qu'un clic.
                if let Some(i) = self.selected_index() {
                    self.tasks.send(i, TaskMsg::Check);
                }
            }
            AppMsg::Open(index) => {
                if let Some(id) = self.task_at(index).map(|t| t.id) {
                    self.selected = Some(id);
                    self.detail.emit(DetailMsg::Show(id));
                    self.detail_open = true;
                }
            }
            AppMsg::OpenSelected => {
                if let Some(id) = self.selected {
                    self.detail.emit(DetailMsg::Show(id));
                    self.detail_open = true;
                }
            }
            AppMsg::DeleteSelected => {
                let Some(task) = self.selected_task().cloned() else { return };
                let Ok(snapshot) = self.store.snapshot(task.id) else { return };
                if self.store.delete_task(task.id).is_ok() {
                    if self.detail_open {
                        self.detail.emit(DetailMsg::Close);
                    }
                    self.offer_undo(tr!("“{title}” deleted", title = task.title), snapshot, &sender);
                    self.select_neighbour();
                    sender.input(AppMsg::Collapse(task.id));
                }
            }
            AppMsg::MoveSelected(delta) => self.move_selected(delta),
            AppMsg::SelectNext(delta) => self.select_next(delta),
            AppMsg::Detail(out) => match out {
                DetailOutput::Changed => self.refresh(),
                DetailOutput::Closed => {
                    self.detail_open = false;
                    self.focus_selected_row();
                }
                DetailOutput::Deleted(snapshot) => {
                    self.detail_open = false;
                    let Some((id, title)) = snapshot.first().map(|t| (t.id, t.title.clone())) else { return };
                    self.offer_undo(tr!("“{title}” deleted", title = title), snapshot, &sender);
                    sender.input(AppMsg::Collapse(id));
                }
            },
            AppMsg::Undo => {
                if let Some(undo) = self.undo.take() {
                    if let Err(e) = self.store.restore(&undo.snapshot) {
                        eprintln!("{}", tr!("omado: couldn't undo: {error}", error = e));
                    }
                    self.selected = undo.snapshot.first().map(|t| t.id);
                    self.fresh.extend(undo.snapshot.iter().map(|t| t.id));
                    self.refresh();
                    self.detail.emit(DetailMsg::Reload);
                }
            }
            AppMsg::DismissUndo => self.undo = None,
            AppMsg::ExpireUndo(generation) => {
                if generation == self.undo_generation {
                    self.undo = None;
                }
            }
            AppMsg::NewList(name) => match self.store.create_list(&name) {
                Ok(list) => self.select(View::List(list.id)),
                Err(e) => eprintln!("{}", tr!("omado: {error}", error = e)),
            },
            AppMsg::RenameList(name) => {
                if let View::List(id) = self.view {
                    let _ = self.store.rename_list(id, &name);
                    self.refresh();
                }
            }
            AppMsg::SetListColor(color) => {
                if let View::List(id) = self.view {
                    let _ = self.store.set_list_color(id, (color != "accent").then_some(color));
                    self.refresh();
                }
            }
            AppMsg::DeleteList => {
                if let View::List(id) = self.view
                    && self.store.delete_list(id).is_ok()
                {
                    self.select(View::Today);
                }
            }
            AppMsg::FocusQuick => {
                self.quick_entry.grab_focus();
            }
            AppMsg::FocusSearch => {
                self.search_entry.grab_focus();
            }
            AppMsg::Escape => {
                if self.detail_open {
                    self.detail.emit(DetailMsg::Close);
                } else if matches!(self.view, View::Search(_)) {
                    self.search_entry.set_text("");
                } else {
                    self.focus_selected_row();
                }
            }
        }
    }
}

impl App {
    fn select(&mut self, view: View) {
        if !matches!(view, View::Search(_)) && !self.search_entry.text().is_empty() {
            self.search_entry.set_text("");
        }
        self.view = view;
        self.selected = None;
        if self.detail_open {
            self.detail.emit(DetailMsg::Close);
        }
        self.refresh();
    }

    /// Saisie rapide complétée par le contexte : date du jour dans « Aujourd'hui »,
    /// étiquette courante dans une vue d'étiquette.
    fn contextual_parse(&self, text: &str) -> omado_core::Parsed {
        let mut parsed = parse(text, now());
        match &self.view {
            View::Today if parsed.due.is_none() => parsed.due = Some(Due::on(now().date())),
            View::Tag(tag) if !parsed.tags.contains(tag) => parsed.tags.push(tag.clone()),
            _ => {}
        }
        parsed
    }

    fn task_at(&self, index: i32) -> Option<&Task> {
        self.tasks.get(index.try_into().ok()?).map(|item| &item.task)
    }

    fn selected_task(&self) -> Option<&Task> {
        let id = self.selected?;
        self.tasks.iter().map(|item| &item.task).find(|t| t.id == id)
    }

    fn selected_index(&self) -> Option<usize> {
        let id = self.selected?;
        self.tasks.iter().position(|item| item.task.id == id)
    }

    fn toggle(&mut self, id: TaskId, done: bool, sender: &ComponentSender<Self>) {
        let Ok(snapshot) = self.store.snapshot(id) else { return };
        let title = snapshot[0].title.clone();
        let result = if done {
            self.store.complete(id, now())
        } else {
            self.store.uncomplete(id).map(|_| Completion::Completed)
        };
        match result {
            Ok(Completion::Completed) if done => {
                self.offer_undo(tr!("“{title}” completed", title = title), snapshot, sender)
            }
            Ok(Completion::Rescheduled(due)) => {
                let when = due_label(&due, now().date());
                self.offer_undo(tr!("“{title}” → {when}", title = title, when = when), snapshot, sender)
            }
            Ok(_) => {}
            Err(e) => eprintln!("{}", tr!("omado: {error}", error = e)),
        }
        self.detail.emit(DetailMsg::Reload);

        // Laisse voir la case se cocher, puis replie la ligne si elle quitte la vue ;
        // sinon (sous-tâche, vue « Terminé »…) elle se met à jour en s'éclairant.
        let leaves = !self.store.tasks(&self.view, now()).is_ok_and(|ts| ts.iter().any(|t| t.id == id));
        let index = self.tasks.iter().position(|item| item.task.id == id);
        if done && let Some(i) = index {
            self.tasks.send(i, TaskMsg::Completing);
        }
        let s = sender.input_sender().clone();
        let pause = if done { COMPLETE_PAUSE } else { Duration::ZERO };
        if leaves && index.is_some() {
            glib::timeout_add_local_once(pause, move || s.emit(AppMsg::Collapse(id)));
        } else {
            self.flash.insert(id);
            glib::timeout_add_local_once(pause, move || s.emit(AppMsg::Refresh));
        }
    }

    fn offer_undo(&mut self, message: String, snapshot: Vec<Task>, sender: &ComponentSender<Self>) {
        self.undo = Some(Undo { message, snapshot });
        self.undo_generation += 1;
        anim::replay(&self.undo_timer, "countdown");
        let (s, generation) = (sender.input_sender().clone(), self.undo_generation);
        glib::timeout_add_local_once(Duration::from_secs(7), move || s.emit(AppMsg::ExpireUndo(generation)));
    }

    fn select_neighbour(&mut self) {
        let Some(i) = self.selected_index() else { return };
        let next = self.tasks.get(i + 1).or_else(|| i.checked_sub(1).and_then(|p| self.tasks.get(p)));
        self.selected = next.map(|item| item.task.id);
    }

    fn select_next(&mut self, delta: i32) {
        let len = self.tasks.len() as i32;
        if len == 0 {
            return;
        }
        let current = self.selected_index().map_or(if delta > 0 { -1 } else { len }, |i| i as i32);
        let next = (current + delta).clamp(0, len - 1);
        self.selected = self.task_at(next).map(|t| t.id);
        self.focus_selected_row();
    }

    fn focus_selected_row(&self) {
        let list = self.tasks.widget();
        match self.selected_index().and_then(|i| list.row_at_index(i as i32)) {
            Some(row) => {
                list.select_row(Some(&row));
                row.grab_focus();
            }
            None => match list.row_at_index(0) {
                Some(row) => {
                    row.grab_focus();
                }
                // Liste vide : focus sur la fenêtre, pour que les raccourcis restent actifs.
                None => {
                    if let Some(window) = list.root().and_downcast::<gtk::Window>() {
                        GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
                    }
                }
            },
        }
    }

    /// Alt+↑/↓ dans une liste : déplace la tâche parmi ses sœurs.
    fn move_selected(&mut self, delta: i32) {
        let View::List(_) = self.view else { return };
        let Some(task) = self.selected_task().cloned() else { return };
        let mut siblings: Vec<TaskId> = self
            .tasks
            .iter()
            .map(|item| &item.task)
            .filter(|t| t.parent_id == task.parent_id && !t.is_completed())
            .map(|t| t.id)
            .collect();
        let Some(pos) = siblings.iter().position(|id| *id == task.id) else { return };
        let target = pos as i32 + delta;
        if target < 0 || target >= siblings.len() as i32 {
            return;
        }
        siblings.swap(pos, target as usize);
        if self.store.reorder(&siblings).is_ok() {
            self.refresh();
            self.focus_selected_row();
        }
    }

    fn refresh(&mut self) {
        let now = now();
        self.lists = self.store.lists().unwrap_or_default();
        self.counts = self.store.counts(now).unwrap_or_default();
        self.tags = self.store.tags().unwrap_or_default();
        // Une liste ou une étiquette a pu disparaître (supprimée ailleurs).
        let gone = match &self.view {
            View::List(id) => !self.lists.iter().any(|l| l.id == *id),
            View::Tag(tag) => !self.tags.iter().any(|(t, _)| t == tag),
            _ => false,
        };
        if gone {
            self.view = View::Today;
        }

        self.refresh_sidebar();
        self.refresh_tasks(now);
        self.refresh_header();
    }

    fn refresh_sidebar(&mut self) {
        {
            let mut guard = self.sidebar_lists.guard();
            guard.clear();
            for list in &self.lists {
                let count = self.counts.per_list.get(&list.id).copied().unwrap_or(0);
                guard.push_back((list.clone(), count));
            }
        }
        let lists_box = self.sidebar_lists.widget();
        match self.view {
            View::List(id) => {
                let row = self.lists.iter().position(|l| l.id == id).and_then(|i| lists_box.row_at_index(i as i32));
                lists_box.select_row(row.as_ref());
            }
            _ => lists_box.unselect_all(),
        }

        while let Some(child) = self.tags_box.first_child() {
            self.tags_box.remove(&child);
        }
        for (i, (tag, count)) in self.tags.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let name = gtk::Label::builder().label(format!("@{tag}")).xalign(0.0).hexpand(true).build();
            name.add_css_class("meta");
            name.add_css_class("tag");
            row.append(&name);
            let n = gtk::Label::new(Some(&count.to_string()));
            n.add_css_class("dim");
            row.append(&n);
            self.tags_box.append(&row);
            if self.view == View::Tag(tag.clone()) {
                self.tags_box.select_row(self.tags_box.row_at_index(i as i32).as_ref());
            }
        }

        for tile in &self.tiles {
            tile.button.set_class_active("selected", tile.view == self.view);
            let n = match tile.view {
                View::Today => self.counts.today,
                View::Upcoming => self.counts.upcoming,
                View::All => self.counts.all,
                _ => 0,
            };
            let label = if tile.view == View::Completed { String::new() } else { n.to_string() };
            if tile.count.label() != label {
                // Pas de rebond au tout premier affichage.
                if self.shown_view.is_some() {
                    anim::replay(&tile.count, "bump");
                }
                tile.count.set_label(&label);
            }
        }
    }

    fn refresh_tasks(&mut self, now: NaiveDateTime) {
        let tasks = self.store.tasks(&self.view, now).unwrap_or_default();
        let list_names: HashMap<ListId, &str> = self.lists.iter().map(|l| (l.id, l.display_name())).collect();
        let nest = matches!(self.view, View::List(_));
        let ordered = if nest { nest_subtasks(tasks) } else { tasks.into_iter().map(|t| (t, false)).collect() };
        // Nouvelles depuis le dernier affichage de cette vue (ajout ici, CLI, annulation…).
        let same_view = self.shown_view.as_ref() == Some(&self.view);
        let mut fresh = std::mem::take(&mut self.fresh);
        let flash = std::mem::take(&mut self.flash);
        if same_view {
            fresh.extend(ordered.iter().map(|(t, _)| t.id).filter(|id| !self.shown_ids.contains(id)));
        }
        let was_empty = self.empty;
        self.shown_ids = ordered.iter().map(|(t, _)| t.id).collect();
        let mut sections = Vec::with_capacity(ordered.len());
        {
            let mut guard = self.tasks.guard();
            guard.clear();
            for (task, nested) in ordered {
                let is_fresh = same_view && fresh.contains(&task.id);
                let section = section_for(&self.view, &task, now);
                let list_name = (!nest && task.list_id != INBOX_ID)
                    .then(|| list_names.get(&task.list_id).map(|n| n.to_string()))
                    .flatten();
                let progress = if task.parent_id.is_none() {
                    let subs = self.store.subtasks(task.id).unwrap_or_default();
                    (!subs.is_empty()).then(|| (subs.iter().filter(|s| s.is_completed()).count(), subs.len()))
                } else {
                    None
                };
                sections.push(section);
                let flash = !is_fresh && flash.contains(&task.id);
                guard.push_back(TaskInit { task, nested, list_name, progress, fresh: is_fresh, flash, now });
            }
        }
        *self.sections.borrow_mut() = sections;
        let list = self.tasks.widget();
        list.invalidate_headers();

        self.empty = self.tasks.is_empty();
        if !same_view && self.shown_view.is_some() {
            anim::replay(list, "enter");
            anim::replay(&self.title_label, "enter");
        }
        if self.empty && (!was_empty || !same_view) {
            anim::replay(&self.empty_box, "appear");
        }
        let (icon, done) = match &self.view {
            View::Today | View::All => ("object-select-symbolic", true),
            View::Upcoming => ("x-office-calendar-symbolic", false),
            View::Search(_) => ("edit-find-symbolic", false),
            _ => ("view-list-bullet-symbolic", false),
        };
        self.empty_icon.set_icon_name(Some(icon));
        self.empty_icon.set_class_active("done", done);
        self.shown_view = Some(self.view.clone());
        // Translators: dates are understood in English and French only: keep the examples in English.
        let add_date = tr!("Add a date: “friday 2pm”, “Oct 15”…");
        (self.empty_title, self.empty_sub) = match &self.view {
            View::Today => (tr!("Nothing for today"), tr!("Enjoy it, or add a task above.")),
            View::Upcoming => (tr!("Nothing scheduled"), add_date),
            View::All => (tr!("All done"), tr!("There's nothing left to do.")),
            View::Completed => (tr!("Nothing completed yet"), tr!("Checked tasks will show up here.")),
            View::List(_) => (tr!("Empty list"), tr!("Add a task above.")),
            View::Tag(_) => (tr!("No tasks with this tag"), ""),
            View::Search(_) => (tr!("No results"), tr!("Try another word.")),
        };
        self.count_text = match self.view {
            View::Completed | View::Search(_) => String::new(),
            _ => {
                let open = self.tasks.iter().filter(|i| !i.task.is_completed() && i.task.parent_id.is_none()).count();
                if open == 0 { String::new() } else { open.to_string() }
            }
        };

        if let Some(row) = self.selected_index().and_then(|i| list.row_at_index(i as i32)) {
            list.select_row(Some(&row));
        }
    }

    fn refresh_header(&mut self) {
        let current_list = match self.view {
            View::List(id) => self.lists.iter().find(|l| l.id == id),
            _ => None,
        };
        self.title = match &self.view {
            View::Today => tr!("Today").into(),
            View::Upcoming => tr!("Scheduled").into(),
            View::All => tr!("All").into(),
            View::Completed => tr!("Completed").into(),
            View::List(_) => current_list.map(|l| l.display_name().to_string()).unwrap_or_default(),
            View::Tag(tag) => format!("@{tag}"),
            View::Search(q) => tr!("“{query}”", query = q.trim()),
        };
        for color in LIST_COLORS {
            self.title_label.remove_css_class(color);
        }
        self.title_label.remove_css_class("list-title");
        if let Some(color) = current_list.and_then(|l| l.color.as_deref()) {
            self.title_label.add_css_class("list-title");
            self.title_label.add_css_class(color);
        }
        self.list_menu = current_list.is_some_and(|l| !l.is_inbox());
        if let Some(list) = current_list
            && self.rename_entry.text() != list.name
        {
            self.rename_entry.set_text(&list.name);
        }
    }
}

fn close_popover(widget: &impl IsA<gtk::Widget>) {
    if let Some(p) = widget.ancestor(gtk::Popover::static_type()).and_downcast::<gtk::Popover>() {
        p.popdown();
    }
}

/// Parents dans l'ordre, chacun suivi de ses sous-tâches.
fn nest_subtasks(tasks: Vec<Task>) -> Vec<(Task, bool)> {
    let ids: Vec<TaskId> = tasks.iter().map(|t| t.id).collect();
    let (children, roots): (Vec<Task>, Vec<Task>) =
        tasks.into_iter().partition(|t| t.parent_id.is_some_and(|p| ids.contains(&p)));
    let mut out = Vec::new();
    for root in roots {
        let id = root.id;
        let done = root.is_completed();
        out.push((root, false));
        if !done {
            out.extend(children.iter().filter(|c| c.parent_id == Some(id)).cloned().map(|c| (c, true)));
        }
    }
    out
}

fn section_for(view: &View, task: &Task, now: NaiveDateTime) -> Option<String> {
    let today = now.date();
    match view {
        View::Today => Some(match task.due {
            Some(due) if due.date < today => tr!("Overdue").into(),
            _ => tr!("Today").into(),
        }),
        View::Upcoming => task.due.map(|d| day_heading(d.date, today)),
        View::All => Some(match task.due {
            Some(due) if due.date < today => tr!("Overdue").into(),
            Some(due) if due.date == today => tr!("Today").into(),
            Some(_) => tr!("Later").into(),
            None => tr!("No date").into(),
        }),
        View::Completed => task.completed_at.map(|at| date_label(at.date(), today)),
        _ => None,
    }
}

fn section_header(sections: &[Option<String>], row: &gtk::ListBoxRow, before: Option<&gtk::ListBoxRow>) {
    let current = sections.get(row.index() as usize).cloned().flatten();
    let previous = before.and_then(|b| sections.get(b.index() as usize).cloned().flatten());
    match current {
        Some(title) if Some(&title) != previous.as_ref() => {
            let label = gtk::Label::builder().label(&title).xalign(0.0).build();
            label.add_css_class("section-header");
            if title == tr!("Overdue") {
                label.add_css_class("overdue");
            }
            row.set_header(Some(&label));
        }
        _ => row.set_header(None::<&gtk::Widget>),
    }
}

fn build_tiles(grid: &gtk::Grid, sender: &ComponentSender<App>) -> Vec<Tile> {
    let specs = [
        (View::Today, tr!("Today"), "x-office-calendar-symbolic", "today", "Ctrl+1"),
        (View::Upcoming, tr!("Scheduled"), "appointment-soon-symbolic", "upcoming", "Ctrl+2"),
        (View::All, tr!("All"), "view-list-bullet-symbolic", "all", "Ctrl+3"),
        (View::Completed, tr!("Completed"), "object-select-symbolic", "completed", "Ctrl+4"),
    ];
    specs
        .into_iter()
        .enumerate()
        .map(|(i, (view, label, icon, class, key))| {
            let button = gtk::Button::new();
            button.add_css_class("tile");
            button.add_css_class(class);
            button.set_tooltip_text(Some(key));
            let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
            let top = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            top.append(&gtk::Image::from_icon_name(icon));
            let count = gtk::Label::builder().hexpand(true).xalign(1.0).build();
            count.add_css_class("count");
            top.append(&count);
            body.append(&top);
            let name = gtk::Label::builder().label(label).xalign(0.0).build();
            name.add_css_class("tile-label");
            body.append(&name);
            button.set_child(Some(&body));
            let (s, v) = (sender.input_sender().clone(), view.clone());
            button.connect_clicked(move |_| s.emit(AppMsg::Select(v.clone())));
            grid.attach(&button, (i % 2) as i32, (i / 2) as i32, 1, 1);
            Tile { view, button, count }
        })
        .collect()
}

/// Raccourcis clavier. Les lettres seules ne sont vues que si aucun champ
/// de saisie ne les a consommées (phase de remontée).
fn install_shortcuts(window: &gtk::ApplicationWindow, sender: &ComponentSender<App>) {
    let keys = gtk::EventControllerKey::new();
    let s = sender.input_sender().clone();
    let win = window.clone();
    keys.connect_key_pressed(move |_, key, _, state| {
        use gdk::Key;
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = state.contains(gdk::ModifierType::ALT_MASK);
        let typing = gtk::prelude::GtkWindowExt::focus(&win)
            .is_some_and(|w| w.is::<gtk::Text>() || w.is::<gtk::TextView>() || w.is::<gtk::Entry>());
        let msg = match key {
            Key::Escape => Some(AppMsg::Escape),
            Key::n if ctrl => Some(AppMsg::FocusQuick),
            Key::f if ctrl => Some(AppMsg::FocusSearch),
            Key::z if ctrl => Some(AppMsg::Undo),
            Key::_1 if ctrl => Some(AppMsg::Select(View::Today)),
            Key::_2 if ctrl => Some(AppMsg::Select(View::Upcoming)),
            Key::_3 if ctrl => Some(AppMsg::Select(View::All)),
            Key::_4 if ctrl => Some(AppMsg::Select(View::Completed)),
            Key::q | Key::w if ctrl => {
                win.close();
                None
            }
            Key::Up if alt => Some(AppMsg::MoveSelected(-1)),
            Key::Down if alt => Some(AppMsg::MoveSelected(1)),
            _ if ctrl || alt || typing => None,
            Key::n | Key::a => Some(AppMsg::FocusQuick),
            Key::slash => Some(AppMsg::FocusSearch),
            Key::j => Some(AppMsg::SelectNext(1)),
            Key::k => Some(AppMsg::SelectNext(-1)),
            Key::x => Some(AppMsg::ToggleSelected),
            Key::e => Some(AppMsg::OpenSelected),
            Key::d | Key::Delete => Some(AppMsg::DeleteSelected),
            Key::u => Some(AppMsg::Undo),
            _ => None,
        };
        match msg {
            Some(m) => {
                s.emit(m);
                glib::Propagation::Stop
            }
            None => glib::Propagation::Proceed,
        }
    });
    window.add_controller(keys);
}
