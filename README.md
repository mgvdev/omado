# Omado

Une application de tâches pour [Omarchy](https://omarchy.org), dans l'esprit
d'Apple Rappels et de Todoist : native (GTK4), au clavier d'abord, aux couleurs
du thème Omarchy actif.

## Installation

```sh
./install.sh             # compile, installe dans ~/.local, active le démon de rappels
./install.sh --uninstall # retire tout, sauf vos tâches
```

Il faut Rust et GTK 4.16 ou plus (`pacman -S rust gtk4`). Pour les raccourcis
Hyprland, ajoutez à la fin de `~/.config/hypr/hyprland.lua` :

```lua
dofile(os.getenv("HOME") .. "/.local/share/omado/hypr/omado.lua")
```

Vous aurez alors **Super+R** pour la saisie rapide (fenêtre flottante, où que vous
soyez) et **Super+Maj+R** pour ouvrir Omado.

## Saisie rapide

Tout se tape dans le titre, en français ou en anglais :

```
Appeler Paul demain 9h #perso @tel !1
Sortir les poubelles tous les lundis et jeudis à 20h #maison
Payer le loyer tous les mois le 5
Réserver le train le 15 octobre
Relancer le client dans 3 jours
Pause dans 30 min
```

| Élément | Exemples |
|---|---|
| Date | `aujourd'hui`, `demain`, `après-demain`, `vendredi`, `lundi prochain`, `ce week-end`, `la semaine prochaine`, `le 15`, `15/10`, `15 octobre`, `2026-12-01`, `dans 3 jours`, `tomorrow`, `next friday`, `in 2 weeks` |
| Heure | `9h`, `14h30`, `14:30`, `9pm`, `midi`, `ce soir`, `dans 2h`, `dans 30 min` |
| Récurrence | `tous les jours`, `tous les 3 jours`, `chaque semaine`, `toutes les deux semaines`, `tous les lundis et jeudis`, `en semaine`, `tous les mois`, `tous les ans`, `every monday` |
| Liste | `#courses` (créée si elle n'existe pas ; `#courses-maison` retrouve « Courses maison ») |
| Étiquette | `@tel`, `@maison` |
| Priorité | `!1` (haute), `!2`, `!3`, ou `p1`… `p3` |

Une heure fixe crée un rappel. Un mois seul (« mars ») n'est pas pris pour une
date, et `#123` ou `a@b.fr` restent dans le titre.
`omado parse "…"` montre ce qui serait compris, sans rien créer.

## Raccourcis de l'application

| Touche | Action |
|---|---|
| `n` ou `a`, `Ctrl+N` | Nouvelle tâche |
| `/`, `Ctrl+F` | Rechercher |
| `j` / `k` | Tâche suivante / précédente |
| `x` | Terminer / rouvrir |
| `e`, `Entrée`, double-clic | Ouvrir le détail |
| `d`, `Suppr` | Supprimer |
| `u`, `Ctrl+Z` | Annuler |
| `Alt+↑` / `Alt+↓` | Déplacer la tâche dans sa liste |
| `Ctrl+1` … `Ctrl+4` | Aujourd'hui, Planifié, Tout, Terminé |
| `Échap` | Fermer le détail, vider la recherche |

Dans la saisie rapide : `Entrée` ajoute et ferme, `Maj+Entrée` ajoute et
continue, `Échap` ferme.

## En ligne de commande

```sh
omado                       # ouvre l'application
omado quick                 # la saisie rapide
omado add "Lait demain #courses"
omado ls                    # aujourd'hui ; aussi upcoming, all, done, inbox, #liste, @étiquette
omado ls all --json
omado search dentiste
omado done 3f2a             # le début de l'identifiant suffit
omado undo 3f2a
omado rm 3f2a
omado lists
```

## Intégration Omarchy

- **Thème** : les couleurs viennent de `~/.local/state/omarchy/current/theme/colors.toml`,
  les opacités des contrôles de `shell.toml`, l'arrondi de Hyprland. Un
  `omarchy theme set` s'applique immédiatement, sans redémarrer Omado.
- **Police** : la police monospace du système (`omarchy font set`).
- **Rappels** : `omado-daemon` tourne comme service utilisateur systemd et passe
  par le centre de notifications d'Omarchy. Les notifications proposent
  « Terminé », « +10 min » et « +1 h », et un clic ouvre Omado.

## Architecture

```
crates/
├── omado-core     modèle, SQLite, saisie rapide, récurrences (testé : cargo test)
├── omado-cli      binaire `omado`
├── omado-daemon   binaire `omado-daemon` : les rappels
└── omado-gtk      binaire `omado-gtk` : l'interface GTK4 + Relm4
packaging/         .desktop, icône, service systemd, règles Hyprland
```

Les tâches vivent dans `~/.local/share/omado/omado.db` (SQLite, mode WAL),
partagée par les trois programmes. Pour faire des essais sans toucher à vos
vraies tâches : `OMADO_DB=/tmp/essai.db omado-gtk`.

Les récurrences sont stockées au format RRULE et les identifiants sont des UUID,
en prévision d'une synchronisation CalDAV (VTODO).
