//! Fenêtre de saisie rapide (`omado quick`), à lier à un raccourci Hyprland.
//!
//! Entrée ajoute et ferme, Maj+Entrée ajoute et garde la fenêtre ouverte,
//! Échap ferme. Le titre, « Omado — … » quelle que soit la langue, sert aux
//! règles de fenêtre Hyprland.

use std::rc::Rc;
use std::time::Duration;

use omado_core::human::due_label;
use omado_core::{Store, now, parse, tr};
use relm4::gtk::{self, gdk, glib, prelude::*};
use relm4::{ComponentParts, ComponentSender, SimpleComponent};

use crate::anim;
use crate::rows::{fill_chips, highlight_entry};

pub struct QuickAdd {
    store: Rc<Store>,
    window: gtk::Window,
    entry: gtk::Entry,
    chips: gtk::Box,
    status: gtk::Label,
}

#[derive(Debug)]
pub enum QuickMsg {
    Changed(String),
    Submit { keep_open: bool },
    Close,
}

#[derive(Debug)]
pub enum QuickOutput {
    Added,
    Closed,
}

impl SimpleComponent for QuickAdd {
    type Init = Rc<Store>;
    type Input = QuickMsg;
    type Output = QuickOutput;
    type Root = gtk::Window;
    type Widgets = ();

    fn init_root() -> gtk::Window {
        let title = format!("Omado — {}", tr!("Quick entry"));
        let window = gtk::Window::builder().title(title).default_width(640).resizable(false).build();
        window.add_css_class("omado-quick");
        window.set_decorated(false);
        window
    }

    fn init(store: Rc<Store>, window: gtk::Window, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
        body.add_css_class("quick-body");
        body.set_margin_top(14);
        body.set_margin_bottom(12);
        body.set_margin_start(14);
        body.set_margin_end(14);

        // Translators: quick entry understands your language: translate the example so that it keeps a date, a time, a #list, a @tag and a !priority it recognizes (a test checks it).
        let example = tr!("Call Paul tomorrow 9am #personal @phone !1");
        let entry = gtk::Entry::builder().placeholder_text(example).hexpand(true).build();
        entry.add_css_class("quick-add");
        {
            let s = sender.input_sender().clone();
            entry.connect_changed(move |e| s.emit(QuickMsg::Changed(e.text().into())));
        }
        {
            let s = sender.input_sender().clone();
            entry.connect_activate(move |_| s.emit(QuickMsg::Submit { keep_open: false }));
        }
        body.append(&entry);

        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        chips.set_visible(false);
        body.append(&chips);

        let status = gtk::Label::builder().label(tr!("↵ add · ⇧↵ add and continue · Esc close")).xalign(0.0).build();
        status.add_css_class("quick-hint");
        body.append(&status);
        window.set_child(Some(&body));

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let s = sender.input_sender().clone();
            keys.connect_key_pressed(move |_, key, _, state| match key {
                gdk::Key::Escape => {
                    s.emit(QuickMsg::Close);
                    glib::Propagation::Stop
                }
                gdk::Key::Return | gdk::Key::KP_Enter if state.contains(gdk::ModifierType::SHIFT_MASK) => {
                    s.emit(QuickMsg::Submit { keep_open: true });
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            });
        }
        window.add_controller(keys);
        {
            let s = sender.input_sender().clone();
            window.connect_close_request(move |_| {
                s.emit(QuickMsg::Close);
                glib::Propagation::Stop
            });
        }

        let model = QuickAdd { store, window: window.clone(), entry: entry.clone(), chips, status };
        window.present();
        entry.grab_focus();
        ComponentParts { model, widgets: () }
    }

    fn update(&mut self, msg: QuickMsg, sender: ComponentSender<Self>) {
        match msg {
            QuickMsg::Changed(text) => {
                let parsed = parse(&text, now());
                let store = &self.store;
                fill_chips(&self.chips, &parsed, |name| store.find_list(name).ok().flatten().is_some(), now().date());
                highlight_entry(&self.entry, &parsed);
                if !text.is_empty() {
                    self.entry.remove_css_class("success");
                    self.status.remove_css_class("success");
                }
            }
            QuickMsg::Submit { keep_open } => {
                let text = self.entry.text().to_string();
                match self.store.add_quick(&text, None, now()) {
                    Ok(task) => {
                        let _ = sender.output(QuickOutput::Added);
                        let mut status = tr!("✓ Added: {title}", title = task.title);
                        if let Some(due) = task.due {
                            status = format!("{status} · {}", due_label(&due, now().date()));
                        }
                        self.status.set_label(&status);
                        self.entry.set_text("");
                        self.entry.add_css_class("success");
                        self.status.add_css_class("success");
                        anim::replay(&self.status, "enter");
                        if !keep_open {
                            let s = sender.input_sender().clone();
                            glib::timeout_add_local_once(Duration::from_millis(550), move || s.emit(QuickMsg::Close));
                        }
                    }
                    Err(omado_core::Error::EmptyTitle) => self.status.set_label(tr!("Give the task a title.")),
                    Err(e) => self.status.set_label(&tr!("Error: {error}", error = e)),
                }
            }
            QuickMsg::Close => {
                self.window.set_visible(false);
                let _ = sender.output(QuickOutput::Closed);
            }
        }
    }
}
