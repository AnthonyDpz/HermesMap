#!/bin/zsh
# Construit « Hermes Map.app » à partir des sources (Mac uniquement).
#   ./macos/build_app.sh             -> dist/Hermes Map.app
#   ./macos/build_app.sh --install   -> et la copie dans ~/Applications
#   ./macos/build_app.sh --rust [--install]   -> « Hermes Map (Rust).app », moteur Rust (nécessite cargo)
#   ./macos/build_app.sh --config mes-reglages.json   -> réglages de départ intégrés à l'app
#     (copiés dans ~/Library/Application Support/hermes-map/config.json au 1er lancement, s'il n'existe pas)
# Prérequis : outils de développement Apple (xcode-select --install), qui fournissent aussi Python 3.
set -e
INSTALL=0 CONFIG="" RUST=0
while (( $# )); do
  case "$1" in
    --install) INSTALL=1 ;;
    --rust) RUST=1 ;;
    --config) shift; CONFIG="$1" ;;
  esac
  shift
done
HERE="${0:A:h}"
SRC="${HERE:h}"
OUT="$SRC/dist/Hermes Map.app"
(( RUST )) && OUT="$SRC/dist/Hermes Map (Rust).app"
NAME="${OUT:t}"

rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources"
printf 'APPL????' > "$OUT/Contents/PkgInfo"
cp "$HERE/AppIcon.icns" "$OUT/Contents/Resources/"

if (( RUST )); then
  # moteur Rust : un seul binaire, interface intégrée
  (cd "$SRC/rust" && cargo build --release)
  VERSION="$(/usr/bin/sed -n 's/^version = "\(.*\)"/\1/p' "$SRC/rust/Cargo.toml" | head -1)"
  cp "$SRC/rust/target/release/hermes-map" "$OUT/Contents/MacOS/hermes-map"
  /usr/bin/sed -e "s/<string>HermesMap<\/string>/<string>hermes-map<\/string>/" -e "s/2026\.10\.26/$VERSION/g" "$HERE/Info.plist" > "$OUT/Contents/Info.plist"
else
  VERSION="$(/usr/bin/sed -n 's/^VERSION = "\(.*\)"/\1/p' "$SRC/hermes_map.py")"
  mkdir -p "$OUT/Contents/Resources/app"
  /usr/bin/sed "s/2026\.10\.26/$VERSION/g" "$HERE/Info.plist" > "$OUT/Contents/Info.plist"
  cp "$HERE/launch.zsh" "$OUT/Contents/Resources/launch.zsh"
  chmod +x "$OUT/Contents/Resources/launch.zsh"
  cp "$SRC"/*.py "$OUT/Contents/Resources/app/"
  cp -R "$SRC/static" "$OUT/Contents/Resources/app/static"
  [[ -f "$SRC/LISEZMOI.pdf" ]] && cp "$SRC/LISEZMOI.pdf" "$OUT/Contents/Resources/"
  [[ -n "$CONFIG" ]] && cp "$CONFIG" "$OUT/Contents/Resources/config.default.json"
  # lanceur natif (universel Apple Silicon + Intel) : indispensable pour que macOS
  # puisse donner à l'app l'« Accès complet au disque » (un simple script ne suffit pas)
  /usr/bin/clang -O2 -mmacosx-version-min=11.0 -arch arm64 -arch x86_64 \
    -o "$OUT/Contents/MacOS/HermesMap" "$HERE/launcher.c"
fi

# retire les attributs étendus (iCloud, Finder) qui font échouer la signature
/usr/bin/xattr -cr "$OUT"
/usr/bin/codesign --force --deep --sign - "$OUT"
echo "OK : $OUT"

if (( INSTALL )); then
  mkdir -p "$HOME/Applications"
  rm -rf "$HOME/Applications/$NAME"
  cp -R "$OUT" "$HOME/Applications/"
  echo "Installée dans ~/Applications/$NAME"
fi
