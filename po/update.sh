#!/bin/sh
# Régénère po/omado.pot à partir des sources et du .desktop, puis met à jour
# chaque traduction (les chaînes nouvelles y arrivent vides, à traduire).
#
# Nouvelle langue : msginit -i po/omado.pot -l xx -o po/xx.po, puis ajoutez
# « xx » à po/LINGUAS et, si sa règle de pluriel diffère de celle de
# l'anglais, à `plural_form` dans crates/omado-core/src/i18n.rs.
set -eu
cd "$(dirname "$0")/.."

xgettext_omado() {
  xgettext --from-code=UTF-8 --add-location=file --package-name=Omado \
    --copyright-holder="Maxence Guyonvarho" \
    --msgid-bugs-address=https://github.com/mgvdev/omado/issues "$@"
}
# shellcheck disable=SC2046
xgettext_omado --language=Rust --output=po/omado.pot --add-comments=Translators: \
  --keyword --keyword='tr!' --keyword='trn!:1,2' --keyword='trc!:1c,2' \
  $(git ls-files --cached --others --exclude-standard 'crates/*.rs')
xgettext_omado --language=Desktop --output=po/omado.pot --join-existing \
  packaging/dev.omado.Omado.desktop.in

for po in po/*.po; do
  [ -e "$po" ] || continue
  msgmerge --quiet --update --backup=none --previous "$po" po/omado.pot
done
