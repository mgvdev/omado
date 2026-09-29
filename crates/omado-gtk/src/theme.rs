//! Apparence calquée sur le thème Omarchy actif, rechargée à chaud.
//!
//! Sources, toutes facultatives (hors Omarchy, un thème sombre neutre sert) :
//! - `~/.local/state/omarchy/current/theme/colors.toml` : la palette ;
//! - `…/theme/shell.toml`, section `[controls]` : les opacités des états
//!   (repos, survol, sélection), pour ressembler au shell Omarchy ;
//! - `hyprctl getoption decoration:rounding` : l'arrondi des fenêtres.
//!
//! `omarchy theme set` remplace le dossier du thème : on surveille son parent.

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use omado_core::SpanKind;
use relm4::gtk::{self, gdk, gio, glib, prelude::*};

pub struct Palette {
    dark: bool,
    accent: Rgb,
    background: Rgb,
    dark_background: Rgb,
    lighter_background: Rgb,
    foreground: Rgb,
    muted: Rgb,
    red: Rgb,
    orange: Rgb,
    yellow: Rgb,
    green: Rgb,
    cyan: Rgb,
    blue: Rgb,
    magenta: Rgb,
    normal_fill: f32,
    hover_fill: f32,
    selected_fill: f32,
    border_alpha: f32,
    radius: u32,
}

#[derive(Clone, Copy)]
struct Rgb(u8, u8, u8);

impl Rgb {
    fn parse(s: &str) -> Option<Rgb> {
        let h = s.trim().strip_prefix('#')?;
        let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
        match h.len() {
            6 | 8 => Some(Rgb(byte(0)?, byte(2)?, byte(4)?)),
            _ => None,
        }
    }

    fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    fn a(self, alpha: f32) -> String {
        format!("rgba({},{},{},{:.3})", self.0, self.1, self.2, alpha)
    }

    fn luminance(self) -> f32 {
        (0.299 * self.0 as f32 + 0.587 * self.1 as f32 + 0.114 * self.2 as f32) / 255.0
    }
}

fn theme_dir() -> PathBuf {
    state_dir().join("theme")
}

fn state_dir() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| PathBuf::from("~/.local/state")).join("omarchy").join("current")
}

impl Palette {
    pub fn load() -> Palette {
        let read = |name: &str| {
            std::fs::read_to_string(theme_dir().join(name)).ok().and_then(|s| s.parse::<toml::Table>().ok())
        };
        let colors = read("colors.toml").unwrap_or_default();
        let controls = read("shell.toml").and_then(|t| t.get("controls")?.as_table().cloned()).unwrap_or_default();

        let color = |key: &str, fallback: &str| {
            colors.get(key).and_then(|v| v.as_str()).and_then(Rgb::parse).or_else(|| Rgb::parse(fallback)).unwrap()
        };
        let alpha = |key: &str, fallback: f32| {
            controls
                .get(key)
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                .map_or(fallback, |v| v.clamp(0.0, 1.0) as f32)
        };

        let background = color("background", "#121212");
        let foreground = color("foreground", "#e0e0e0");
        let dark = match colors.get("mode").and_then(|v| v.as_str()) {
            Some("light") => false,
            Some("dark") => true,
            _ => background.luminance() < 0.5,
        };
        Palette {
            dark,
            accent: color("accent", "#7dabe8"),
            background,
            dark_background: color("dark_background", &background.hex()),
            lighter_background: color("lighter_background", &background.hex()),
            foreground,
            muted: color("muted", "#9e9e9e"),
            red: color("red", "#e06c75"),
            orange: color("orange", "#d19a66"),
            yellow: color("yellow", "#e5c07b"),
            green: color("green", "#98c379"),
            cyan: color("cyan", "#56b6c2"),
            blue: color("blue", "#61afef"),
            magenta: color("magenta", "#c678dd"),
            normal_fill: alpha("normal-fill-alpha", 0.04),
            hover_fill: alpha("hover-cursor-fill-alpha", 0.08),
            selected_fill: alpha("selected-fill-alpha", 0.18),
            border_alpha: alpha("normal-border-alpha", 0.4),
            radius: hyprland_rounding().unwrap_or(8),
        }
    }

    /// Couleur d'une liste : nom de couleur du thème, sinon l'accent.
    fn named(&self, name: &str) -> String {
        match name {
            "red" => self.red,
            "orange" => self.orange,
            "yellow" => self.yellow,
            "green" => self.green,
            "cyan" => self.cyan,
            "blue" => self.blue,
            "magenta" => self.magenta,
            _ => self.accent,
        }
        .hex()
    }

    pub fn css(&self) -> String {
        let fg_rgb = self.foreground;
        let fg = fg_rgb.hex();
        let bg = self.background.hex();
        let side = self.dark_background.hex();
        let raised = self.lighter_background.hex();
        let accent = self.accent.hex();
        let muted = self.muted.hex();
        let red = self.red.hex();
        let orange = self.orange.hex();
        let blue = self.blue.hex();
        let cyan = self.cyan.hex();
        let green = self.green.hex();
        let magenta = self.magenta.hex();
        let yellow = self.yellow.hex();
        let fill = fg_rgb.a(self.normal_fill);
        let hover = fg_rgb.a(self.hover_fill);
        let selected = fg_rgb.a(self.selected_fill);
        let border = fg_rgb.a(self.border_alpha);
        let hairline = fg_rgb.a(0.08);
        let accent_soft = self.accent.a(0.18);
        let accent_sel = self.accent.a(0.35);
        // Départ vif, arrivée douce : la courbe de toutes les micro-interactions.
        let ease = "cubic-bezier(0.2, 0.8, 0.2, 1)";
        let r = self.radius.min(14);
        let rs = (self.radius / 2).min(8);
        let list_colors: String = ["red", "orange", "yellow", "green", "cyan", "blue", "magenta"]
            .iter()
            .map(|c| {
                format!(
                    ".dot.{c} {{ background: {}; }}\n.list-title.{c} {{ color: {}; }}\n",
                    self.named(c),
                    self.named(c)
                )
            })
            .collect();

        format!(
            r#"
window.omado, window.omado-quick {{
    background: {bg};
    color: {fg};
    font-family: monospace;
}}
selection, text selection {{ background: {accent_sel}; color: {fg}; }}
label.dim, .dim {{ color: {muted}; }}

/* Barre latérale */
.sidebar {{ background: {side}; border-right: 1px solid {hairline}; }}
.sidebar-title {{ font-size: 0.8em; font-weight: bold; color: {muted}; margin: 14px 14px 4px 16px; letter-spacing: 1px; }}
.tile {{
    background: {fill};
    border: 1px solid {hairline};
    border-radius: {r}px;
    padding: 8px 10px;
    min-height: 0;
    box-shadow: none;
    color: {fg};
}}
.tile {{ transition: background-color 150ms ease-out, border-color 150ms ease-out, transform 160ms {ease}; }}
.tile:hover {{ background: {hover}; transform: translateY(-1px); }}
.tile:active {{ transform: scale(0.97); }}
.tile.selected {{ background: {accent_soft}; border-color: {accent}; }}
.tile .count {{ font-size: 1.5em; font-weight: bold; }}
.tile .tile-label {{ font-weight: bold; color: {muted}; }}
.tile.selected .tile-label {{ color: {fg}; }}
.tile image {{ -gtk-icon-size: 18px; }}
.tile.today image {{ color: {accent}; }}
.tile.upcoming image {{ color: {red}; }}
.tile.all image {{ color: {fg}; }}
.tile.completed image {{ color: {green}; }}

list.nav {{ background: transparent; }}
list.nav > row {{ border-radius: {rs}px; margin: 1px 8px; padding: 6px 8px; transition: background-color 150ms ease-out; }}
list.nav > row:hover {{ background: {hover}; }}
list.nav > row:selected {{ background: {selected}; color: {fg}; }}
.dot {{ min-width: 10px; min-height: 10px; border-radius: 999px; background: {accent}; }}
.dot.inbox {{ background: {muted}; }}
{list_colors}
entry.search {{ margin: 12px 10px 6px 10px; }}

/* Contrôles façon shell Omarchy */
entry, dropdown > button, button.flat-control, spinbutton {{
    background: {fill};
    border: 1px solid {border};
    border-radius: {rs}px;
    box-shadow: none;
    color: {fg};
    outline: none;
}}
entry {{ transition: border-color 150ms ease-out, background-color 150ms ease-out; }}
entry:focus-within {{ border-color: {accent}; }}
button {{ border-radius: {rs}px; transition: background-color 150ms ease-out, color 150ms ease-out, transform 120ms {ease}; }}
button:active {{ transform: scale(0.96); }}
button.flat, menubutton.flat > button {{ background: none; border: none; box-shadow: none; }}
button.flat:hover, menubutton.flat > button:hover, button.flat-control:hover {{ background: {hover}; }}
button.accent {{ background: {accent}; color: {bg}; border: none; }}
button.danger {{ color: {red}; }}
popover > contents {{ background: {raised}; border: 1px solid {border}; border-radius: {r}px; color: {fg}; }}
popover > arrow {{ background: {raised}; border: 1px solid {border}; }}
scrollbar {{ background: transparent; }}
scrollbar slider {{ background: {selected}; border-radius: 999px; min-width: 4px; }}
calendar {{ background: transparent; border: none; color: {fg}; }}
calendar > grid > label.day-number:selected {{ background: {accent}; color: {bg}; border-radius: {rs}px; }}
calendar > grid > label.today {{ color: {accent}; font-weight: bold; }}

/* Zone principale */
.view-title {{ font-size: 1.9em; font-weight: bold; color: {accent}; }}
.view-count {{ font-size: 1.9em; font-weight: bold; color: {muted}; }}
entry.quick-add {{ padding: 6px 10px; min-height: 34px; font-size: 1.05em; }}
.quick-hint {{ font-size: 0.85em; color: {muted}; }}

.chip {{ border-radius: 999px; padding: 1px 9px; font-size: 0.85em; background: {fill}; border: 1px solid {hairline}; }}
.chip.date {{ color: {accent}; border-color: {accent}; }}
.chip.recurrence {{ color: {cyan}; border-color: {cyan}; }}
.chip.list {{ color: {blue}; border-color: {blue}; }}
.chip.tag {{ color: {magenta}; border-color: {magenta}; }}
.chip.priority {{ color: {orange}; border-color: {orange}; }}
.chip.new {{ border-style: dashed; }}

list.tasks {{ background: transparent; }}
list.tasks > row {{ padding: 0; border-radius: {rs}px; margin: 0 6px; transition: background-color 150ms ease-out; }}
list.tasks > row:hover {{ background: {fill}; }}
list.tasks > row:selected {{ background: {selected}; }}
.task-row {{ padding: 7px 10px; border-bottom: 1px solid {hairline}; border-radius: {rs}px; transition: opacity 220ms ease-out; }}
list.tasks > row.subtask .task-row {{ padding-left: 38px; }}
.task-row.completing {{ opacity: 0.45; }}
.task-row.completing .task-title {{ color: {muted}; }}
.task-row.fresh {{ animation: flash 1400ms ease-out; }}
.section-header {{ font-weight: bold; color: {muted}; padding: 14px 16px 4px 16px; }}
.section-header.overdue {{ color: {red}; }}
.task-title.done {{ color: {muted}; }}
.meta {{ font-size: 0.85em; color: {muted}; }}
.meta.overdue {{ color: {red}; }}
.meta.today {{ color: {accent}; }}
.meta.list {{ color: {blue}; }}
.meta.tag {{ color: {magenta}; }}
.prio-mark {{ font-weight: bold; }}
.prio-mark.p1 {{ color: {red}; }}
.prio-mark.p2 {{ color: {orange}; }}
.prio-mark.p3 {{ color: {blue}; }}

checkbutton.task-check check {{
    min-width: 16px; min-height: 16px;
    border-radius: 999px;
    border: 2px solid {muted};
    background: transparent;
    box-shadow: none;
    -gtk-icon-source: none;
}}
checkbutton.task-check.p1 check {{ border-color: {red}; background: {}; }}
checkbutton.task-check.p2 check {{ border-color: {orange}; background: {}; }}
checkbutton.task-check.p3 check {{ border-color: {blue}; background: {}; }}
checkbutton.task-check check {{ transition: background-color 180ms ease-out, border-color 180ms ease-out, color 180ms ease-out; }}
checkbutton.task-check check:checked {{ background: {accent}; border-color: {accent}; -gtk-icon-source: -gtk-icontheme("object-select-symbolic"); color: {bg}; animation: pop 280ms {ease}; }}
checkbutton.task-check:hover check {{ background: {hover}; }}
/* Aperçu de la coche au survol, façon Todoist. */
checkbutton.task-check:hover check:not(:checked) {{ -gtk-icon-source: -gtk-icontheme("object-select-symbolic"); color: {muted}; }}

.empty-icon {{ -gtk-icon-size: 44px; color: {muted}; margin-bottom: 6px; }}
.empty-icon.done {{ color: {green}; }}
.empty-title {{ font-size: 1.3em; font-weight: bold; color: {muted}; }}
.empty-sub {{ color: {muted}; }}

/* Détail */
.detail {{ background: {side}; border-left: 1px solid {hairline}; }}
entry.detail-title {{ font-size: 1.25em; font-weight: bold; background: transparent; border-color: transparent; }}
entry.detail-title:focus-within {{ border-color: {accent}; }}
.field-label {{ font-size: 0.8em; font-weight: bold; color: {muted}; letter-spacing: 1px; }}
.schedule {{ color: {accent}; font-weight: bold; }}
.schedule.overdue {{ color: {red}; }}
.recurrence-label {{ color: {cyan}; }}
.parse-error {{ color: {yellow}; font-size: 0.85em; }}
textview.notes {{ background: {fill}; color: {fg}; border: 1px solid {border}; border-radius: {rs}px; padding: 6px; }}
textview.notes text {{ background: transparent; color: {fg}; }}
.footnote {{ font-size: 0.8em; color: {muted}; }}

/* Barre d'annulation, avec son compte à rebours */
.undo-bar {{ background: {raised}; border: 1px solid {border}; border-radius: {r}px; padding: 6px 6px 4px 14px; margin: 12px; }}
.undo-timer {{ min-height: 2px; background: {accent}; border-radius: 2px; margin: 2px 8px 0 0; transform-origin: 0 50%; opacity: 0.8; }}
.undo-timer.countdown-a {{ animation: countdown-a 7s linear; }}
.undo-timer.countdown-b {{ animation: countdown-b 7s linear; }}

/* Saisie rapide */
entry.quick-add.success {{ border-color: {green}; }}
.quick-hint.success {{ color: {green}; }}
.quick-body {{ animation: enter-a 200ms {ease}; }}

/* Mouvement : rejouables en alternant les classes -a / -b (voir anim.rs) */
@keyframes pop {{ 0% {{ transform: scale(0.55); }} 60% {{ transform: scale(1.2); }} 100% {{ transform: scale(1); }} }}
@keyframes flash {{ from {{ background-color: {accent_soft}; }} to {{ background-color: transparent; }} }}
@keyframes chip-in {{ from {{ opacity: 0; transform: scale(0.7); }} to {{ opacity: 1; transform: scale(1); }} }}
@keyframes enter-a {{ from {{ opacity: 0; transform: translateY(8px); }} to {{ opacity: 1; transform: translateY(0); }} }}
@keyframes enter-b {{ from {{ opacity: 0; transform: translateY(8px); }} to {{ opacity: 1; transform: translateY(0); }} }}
@keyframes bump-a {{ 0% {{ transform: scale(1); }} 40% {{ transform: scale(1.35); }} 100% {{ transform: scale(1); }} }}
@keyframes bump-b {{ 0% {{ transform: scale(1); }} 40% {{ transform: scale(1.35); }} 100% {{ transform: scale(1); }} }}
@keyframes appear-a {{ from {{ opacity: 0; transform: translateY(12px) scale(0.96); }} to {{ opacity: 1; transform: translateY(0) scale(1); }} }}
@keyframes appear-b {{ from {{ opacity: 0; transform: translateY(12px) scale(0.96); }} to {{ opacity: 1; transform: translateY(0) scale(1); }} }}
@keyframes countdown-a {{ from {{ transform: scaleX(1); }} to {{ transform: scaleX(0); }} }}
@keyframes countdown-b {{ from {{ transform: scaleX(1); }} to {{ transform: scaleX(0); }} }}
.enter-a {{ animation: enter-a 220ms {ease}; }}
.enter-b {{ animation: enter-b 220ms {ease}; }}
.bump-a {{ animation: bump-a 340ms {ease}; }}
.bump-b {{ animation: bump-b 340ms {ease}; }}
.appear-a {{ animation: appear-a 360ms {ease}; }}
.appear-b {{ animation: appear-b 360ms {ease}; }}
.chip.pop {{ animation: chip-in 200ms {ease}; }}
"#,
            self.red.a(0.08),
            self.orange.a(0.08),
            self.blue.a(0.08),
        )
    }
}

fn hyprland_rounding() -> Option<u32> {
    let out = Command::new("hyprctl").args(["getoption", "decoration:rounding", "-j"]).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let after = text.split("\"int\"").nth(1)?;
    let digits: String = after.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Couleur (thème courant) d'un élément reconnu par la saisie rapide.
pub fn span_rgb(kind: SpanKind) -> Option<(u8, u8, u8)> {
    PALETTE.with_borrow(|p| {
        let p = p.as_ref()?;
        let c = match kind {
            SpanKind::Date | SpanKind::Time => p.accent,
            SpanKind::Recurrence => p.cyan,
            SpanKind::List => p.blue,
            SpanKind::Tag => p.magenta,
            SpanKind::Priority => p.orange,
        };
        Some((c.0, c.1, c.2))
    })
}

thread_local! {
    static PALETTE: RefCell<Option<Palette>> = const { RefCell::new(None) };
    static PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
}

/// Installe le style et surveille les changements de thème Omarchy.
pub fn install() {
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    PROVIDER.set(Some(provider));
    apply();

    let dir = gio::File::for_path(state_dir());
    let Ok(monitor) = dir.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else {
        return;
    };
    let pending: std::rc::Rc<RefCell<Option<glib::SourceId>>> = Default::default();
    monitor.connect_changed(move |_, _, _, _| {
        // Un changement de thème touche beaucoup de fichiers : on attend que ça se calme.
        if let Some(id) = pending.borrow_mut().take() {
            id.remove();
        }
        let slot = pending.clone();
        let id = glib::timeout_add_local_once(Duration::from_millis(400), move || {
            slot.borrow_mut().take();
            apply();
        });
        *pending.borrow_mut() = Some(id);
    });
    MONITOR.set(Some(monitor));
}

fn apply() {
    let palette = Palette::load();
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(palette.dark);
        // Depuis GTK 4.16, le texte est placé au sous-pixel près : sur un écran à l'échelle 1,
        // les traits horizontaux fins (barre du T, du f…) s'étalent sur deux rangées et
        // semblent coupés. On revient au calage sur la grille, selon le hinting du système.
        settings.set_gtk_font_rendering(gtk::FontRendering::Manual);
        settings.set_gtk_hint_font_metrics(true);
    }
    let css = palette.css();
    PROVIDER.with_borrow(|p| {
        if let Some(p) = p {
            p.load_from_string(&css);
        }
    });
    PALETTE.set(Some(palette));
}
