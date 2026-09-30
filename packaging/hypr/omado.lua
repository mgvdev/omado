-- Omado : à charger depuis ~/.config/hypr/hyprland.lua, par exemple :
--   dofile(os.getenv("HOME") .. "/.local/share/omado/hypr/omado.lua")

-- La saisie rapide flotte au centre, au-dessus de tout, sur tous les bureaux.
-- Son titre est traduit (« Omado — Saisie rapide », « Omado — Quick entry »…) ;
-- la fenêtre principale s'appelle simplement « Omado ».
o.window({ class = "^(dev.omado.Omado)$", title = "^(Omado — .+)$" }, {
  float = true,
  center = true,
  pin = true,
})

-- Super+R : saisie rapide ; Super+Maj+R : l'application.
o.bind("SUPER + R", "Omado : saisie rapide", "omado-gtk --quick")
o.bind("SUPER + SHIFT + R", "Omado", o.launch_sole("dev.omado.Omado", "omado-gtk"))
