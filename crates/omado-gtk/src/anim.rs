//! Micro-interactions : durées communes et relance des animations CSS.
//!
//! GTK ne rejoue une animation CSS que si son nom change : chaque animation
//! rejouable existe donc en deux exemplaires identiques (`nom-a`, `nom-b`),
//! et [`replay`] alterne entre les deux classes. Si l'utilisateur a coupé les
//! animations du système (`gtk-enable-animations`), GTK les ignore de lui-même.

use relm4::gtk::prelude::*;

/// Dépliage / repliage d'une ligne de tâche.
pub const ROW_MS: u32 = 200;

/// Relance l'animation `name` (classes CSS `name-a` / `name-b`) sur `widget`.
pub fn replay(widget: &impl IsA<relm4::gtk::Widget>, name: &str) {
    let (a, b) = (format!("{name}-a"), format!("{name}-b"));
    if widget.has_css_class(&a) {
        widget.remove_css_class(&a);
        widget.add_css_class(&b);
    } else {
        widget.remove_css_class(&b);
        widget.add_css_class(&a);
    }
}
