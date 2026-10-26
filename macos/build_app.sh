#!/bin/zsh
# Construit « Hermes Map.app » à partir des sources (Mac uniquement).
#   ./macos/build_app.sh             -> dist/Hermes Map.app
#   ./macos/build_app.sh --install   -> et la copie dans ~/Applications
# Prérequis : outils de développement Apple (xcode-select --install), qui fournissent aussi Python 3.
set -e
HERE="${0:A:h}"
SRC="${HERE:h}"
OUT="$SRC/dist/Hermes Map.app"
VERSION="$(/usr/bin/sed -n 's/^VERSION = "\(.*\)"/\1/p' "$SRC/hermes_map.py")"

rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources/app"
/usr/bin/sed "s/2026\.10\.26/$VERSION/g" "$HERE/Info.plist" > "$OUT/Contents/Info.plist"
printf 'APPL????' > "$OUT/Contents/PkgInfo"
cp "$HERE/AppIcon.icns" "$OUT/Contents/Resources/"
cp "$HERE/launch.zsh" "$OUT/Contents/Resources/launch.zsh"
chmod +x "$OUT/Contents/Resources/launch.zsh"
cp "$SRC"/*.py "$OUT/Contents/Resources/app/"
cp -R "$SRC/static" "$OUT/Contents/Resources/app/static"
[[ -f "$SRC/LISEZMOI.pdf" ]] && cp "$SRC/LISEZMOI.pdf" "$OUT/Contents/Resources/"

# lanceur natif (universel Apple Silicon + Intel) : indispensable pour que macOS
# puisse donner à l'app l'« Accès complet au disque » (un simple script ne suffit pas)
/usr/bin/clang -O2 -mmacosx-version-min=11.0 -arch arm64 -arch x86_64 \
  -o "$OUT/Contents/MacOS/HermesMap" "$HERE/launcher.c"
/usr/bin/codesign --force --deep --sign - "$OUT"
echo "OK : $OUT"

if [[ "$1" == "--install" ]]; then
  mkdir -p "$HOME/Applications"
  rm -rf "$HOME/Applications/Hermes Map.app"
  cp -R "$OUT" "$HOME/Applications/"
  echo "Installée dans ~/Applications/Hermes Map.app"
fi
