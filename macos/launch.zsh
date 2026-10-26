#!/bin/zsh
# Hermes Map (lancé par Contents/MacOS/HermesMap) : lance le serveur local (sans Terminal) et ouvre la carte dans le navigateur.
# Le dossier exploré est choisi dans l'écran d'accueil (réglages : ~/Library/Application Support/hermes-map/config.json).
RES="${0:A:h}"
SUPPORT="$HOME/Library/Application Support/hermes-map"
LOG="$HOME/Library/Logs/Hermes Map.log"
PORTFILE="$SUPPORT/port"
mkdir -p "$SUPPORT" "$HOME/Library/Logs"
# réglages fournis avec l'app (build_app.sh --config) : utilisés seulement s'il n'y en a pas encore
[[ ! -f "$SUPPORT/config.json" && -f "$RES/config.default.json" ]] && cp "$RES/config.default.json" "$SUPPORT/config.json"

# PATH du shell de connexion (pour trouver hermes, ffmpeg…), plus les emplacements habituels
LOGIN_PATH="$(/bin/zsh -lc 'print -rn -- $PATH' 2>/dev/null)"
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:${LOGIN_PATH:+$LOGIN_PATH:}/usr/bin:/bin:/usr/sbin:/sbin"

alive() { [[ -n "$1" ]] && /usr/bin/curl -s -m 2 "http://127.0.0.1:$1/api/ping" 2>/dev/null | /usr/bin/grep -q hermes-map; }

# déjà lancé ? on ouvre simplement la page
port=""
[[ -f "$PORTFILE" ]] && port="$(<"$PORTFILE")"
if alive "$port"; then
  /usr/bin/open "http://127.0.0.1:$port/"
  exit 0
fi

PY=""
for c in /usr/bin/python3 /opt/homebrew/bin/python3 /usr/local/bin/python3; do
  if [[ -x "$c" ]] && "$c" -c 'import sys; sys.exit(sys.version_info < (3, 8))' 2>/dev/null; then PY="$c"; break; fi
done
if [[ -z "$PY" ]]; then
  /usr/bin/osascript -e 'display dialog "Hermes Map a besoin de Python 3. macOS va proposer de l’installer (outils de développement) : accepte, puis relance Hermes Map." buttons {"OK"} with title "Hermes Map" with icon caution' >/dev/null 2>&1
  /usr/bin/xcode-select --install >/dev/null 2>&1
  exit 1
fi

rm -f "$PORTFILE"
{ echo; echo "===== $(date '+%Y-%m-%d %H:%M:%S') : démarrage ($PY)"; } >> "$LOG"
export HM_APP="${0:A:h:h:h}"
/usr/bin/nohup "$PY" "$RES/app/hermes_map.py" --port 8765 --no-browser \
  --port-file "$PORTFILE" --idle-exit 120 >> "$LOG" 2>&1 &
SRV=$!

for i in {1..80}; do
  sleep 0.25
  [[ -f "$PORTFILE" ]] && port="$(<"$PORTFILE")" && alive "$port" && break
done
if ! alive "$port"; then
  /usr/bin/osascript -e 'display dialog "Hermes Map n’a pas pu démarrer.\n\nLe journal est dans : ~/Library/Logs/Hermes Map.log" buttons {"Ouvrir le journal", "OK"} default button "OK" with title "Hermes Map" with icon caution' -e 'if button returned of result is "Ouvrir le journal" then do shell script "open -a Console " & quoted form of (POSIX path of (path to home folder) & "Library/Logs/Hermes Map.log")' >/dev/null 2>&1
  exit 1
fi
/usr/bin/open "http://127.0.0.1:$port/"
wait $SRV
exit 0
