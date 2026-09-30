#!/usr/bin/env bash
# Installe Omado pour l'utilisateur courant (par défaut dans ~/.local).
#
#   ./install.sh              compile et installe, active le démon de rappels
#   ./install.sh --uninstall  retire tout (la base de tâches est conservée)
#
# PREFIX=/chemin ./install.sh pour installer ailleurs.
set -euo pipefail

PREFIX=${PREFIX:-$HOME/.local}
BINDIR=$PREFIX/bin
DATADIR=$PREFIX/share
UNITDIR=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user
BINS=(omado omado-gtk omado-daemon)
cd "$(dirname "$0")"

if [[ ${1:-} == --uninstall ]]; then
  systemctl --user disable --now omado-daemon.service 2>/dev/null || true
  for b in "${BINS[@]}"; do rm -f "$BINDIR/$b"; done
  rm -f "$DATADIR/applications/dev.omado.Omado.desktop" \
        "$DATADIR/icons/hicolor/scalable/apps/dev.omado.Omado.svg" \
        "$UNITDIR/omado-daemon.service"
  rm -rf "$DATADIR/omado"
  systemctl --user daemon-reload
  echo "Omado désinstallé. Vos tâches restent dans ${XDG_DATA_HOME:-$HOME/.local/share}/omado/."
  exit 0
fi

cargo build --release --locked

install -Dm755 -t "$BINDIR" "${BINS[@]/#/target/release/}"
# Le .desktop reçoit ses traductions (Name[fr]=…) depuis po/.
mkdir -p "$DATADIR/applications"
msgfmt --desktop --template=packaging/dev.omado.Omado.desktop.in -d po \
  -o "$DATADIR/applications/dev.omado.Omado.desktop"
install -Dm644 packaging/icons/dev.omado.Omado.svg "$DATADIR/icons/hicolor/scalable/apps/dev.omado.Omado.svg"
install -Dm644 packaging/hypr/omado.lua "$DATADIR/omado/hypr/omado.lua"
mkdir -p "$UNITDIR"
sed "s|@BINDIR@|$BINDIR|" packaging/systemd/omado-daemon.service > "$UNITDIR/omado-daemon.service"

gtk-update-icon-cache -q "$DATADIR/icons/hicolor" 2>/dev/null || true
update-desktop-database -q "$DATADIR/applications" 2>/dev/null || true
systemctl --user daemon-reload
systemctl --user enable --now omado-daemon.service
systemctl --user restart omado-daemon.service

cat <<MSG

Omado est installé dans $BINDIR, et le démon de rappels tourne.

Raccourcis Hyprland (Super+R : saisie rapide) : ajoutez cette ligne
à la fin de ~/.config/hypr/hyprland.lua, puis rechargez Hyprland :

  dofile("$DATADIR/omado/hypr/omado.lua")
MSG
