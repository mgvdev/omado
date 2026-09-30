//! Compile les traductions `po/*.po` en catalogues `.mo`, que `src/i18n.rs`
//! embarque dans les binaires : rien à installer, `cargo run` est traduit.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let po_dir = manifest.join("../../po");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    println!("cargo::rerun-if-changed={}", po_dir.display());

    let mut po_files: Vec<PathBuf> = fs::read_dir(&po_dir)
        .expect("dossier po/ introuvable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "po"))
        .collect();
    po_files.sort();

    let mut entries = Vec::new();
    for po in &po_files {
        let code = po.file_stem().and_then(|s| s.to_str()).expect("nom de fichier .po en UTF-8");
        let mo = out.join(format!("{code}.mo"));
        compile(po, &mo);
        entries.push(format!("({code:?}, include_bytes!({:?})),", mo.display().to_string()));
    }
    let source = format!("pub static CATALOGS: &[(&str, &[u8])] = &[\n{}\n];\n", entries.join("\n"));
    fs::write(out.join("catalogs.rs"), source).expect("écriture de catalogs.rs");
}

fn compile(po: &Path, mo: &Path) {
    let status = Command::new("msgfmt")
        .arg("--check")
        .arg("--output-file")
        .arg(mo)
        .arg(po)
        .status()
        .unwrap_or_else(|e| panic!("msgfmt introuvable ({e}) : installez gettext (pacman -S gettext)"));
    assert!(status.success(), "msgfmt a refusé {}", po.display());
}
