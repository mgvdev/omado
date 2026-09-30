//! `omado-gtk` : l'application graphique d'Omado.
//!
//! Une seule instance tourne : `omado-gtk --quick` lancé alors que l'app est
//! ouverte ouvre la saisie rapide dans l'instance existante (D-Bus, via GApplication).

mod anim;
mod app;
mod detail;
mod quick;
mod rows;
mod theme;

use std::rc::Rc;

use omado_core::{Store, tr};
use relm4::RelmApp;
use relm4::gtk::{self, gio, glib, prelude::*};

use app::{App, AppMsg, BROKER};

pub const APP_ID: &str = "dev.omado.Omado";

fn main() -> glib::ExitCode {
    let app =
        gtk::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE).build();
    app.add_main_option(
        "quick",
        glib::Char::from(b'q'),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        tr!("Open quick entry"),
        None,
    );

    // Instance déjà ouverte : GApplication lui passe la ligne de commande et rend la main.
    // On évite RelmApp ici : son `run` finit par une itération bloquante qui ne rendrait jamais la main.
    if instance_running() {
        return app.run();
    }

    let store = match Store::open_default() {
        Ok(s) => Rc::new(s),
        Err(e) => {
            eprintln!("{}", tr!("omado: couldn't open the database: {error}", error = e));
            return glib::ExitCode::FAILURE;
        }
    };
    app.connect_startup(|_| theme::install());
    app.connect_command_line(|_, cmd| {
        let quick = cmd.options_dict().contains("quick");
        BROKER.send(if quick { AppMsg::ShowQuick } else { AppMsg::ShowMain });
        glib::ExitCode::SUCCESS
    });

    let relm = RelmApp::from_app(app).with_broker(&BROKER).visible_on_activate(false);
    relm.run::<App>(store);
    glib::ExitCode::SUCCESS
}

/// Une instance d'Omado possède-t-elle déjà son nom sur le bus de session ?
fn instance_running() -> bool {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else { return false };
    bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        Some(&(APP_ID,).to_variant()),
        glib::VariantTy::new("(b)").ok(),
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
    )
    .ok()
    .and_then(|v| v.get::<(bool,)>())
    .is_some_and(|(owned,)| owned)
}
