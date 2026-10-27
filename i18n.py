"""Traduction FR -> EN des messages affichés (même dictionnaire que la page : static/i18n/en.json)."""
import json
import re
from pathlib import Path

import settings

DICT_FILE = Path(__file__).resolve().parent / "static" / "i18n" / "en.json"
try:
    DATA = json.loads(DICT_FILE.read_text("utf-8"))
except (OSError, ValueError):
    DATA = {}
EXACT = DATA.get("exact", {})
_PATS = []
for fr, en in sorted((DATA.get("patterns") or {}).items(), key=lambda kv: -len(kv[0])):   # clés longues d'abord
    rx = "^" + re.sub(r"\\\{(\d+)(#?)\\\}",
                      lambda m: rf"(?P<g{m.group(1)}>\d[\d\s.,]*)" if m.group(2) else rf"(?P<g{m.group(1)}>[\s\S]*?)",
                      re.escape(fr)) + "$"
    _PATS.append((re.compile(rx), en))

# clés des réponses JSON dont la valeur est un message à traduire
KEYS = {"error", "why", "note", "notes", "status"}


def lang():
    return (settings.current.data.get("language") or "").lower()


def _core(s):
    if s in EXACT:
        return EXACT[s]
    for rx, en in _PATS:
        m = rx.match(s)
        if m:
            return re.sub(r"\{(\d+)\}", lambda x: EXACT.get(m.group("g" + x.group(1)), m.group("g" + x.group(1))), en)
    return None


def tr(s):
    """Texte traduit en anglais si l'interface est en anglais (sinon inchangé)."""
    if lang() != "en" or not isinstance(s, str) or not s.strip():
        return s
    m = re.match(r"^(\s*)([\s\S]*?)(\s*)$", s)
    r = _core(m.group(2))
    return s if r is None else m.group(1) + r + m.group(3)


def tr_obj(obj, key=None):
    """Traduit les messages d'une réponse JSON (erreurs, statuts, notes, lignes du journal de Hermes Map)."""
    if lang() != "en":
        return obj
    if isinstance(obj, dict):
        return {k: tr_obj(v, k) for k, v in obj.items()}
    if isinstance(obj, list):
        if key in KEYS:
            return [tr(x) if isinstance(x, str) else tr_obj(x) for x in obj]
        if key == "lines":
            return [tr(x) if isinstance(x, str) and "[Hermes Map]" in x else x for x in obj]
        return [tr_obj(x) for x in obj]
    if isinstance(obj, str) and key in KEYS:
        return tr(obj)
    return obj
