"""Réglages de Hermes Map : un petit fichier config.json, créé par l'écran d'accueil au premier lancement.

Emplacement : ~/Library/Application Support/hermes-map/config.json (Mac)
              ~/.config/hermes-map/config.json (autres systèmes)
"""
import copy
import fnmatch
import json
import os
import re
import shutil
import sys
from pathlib import Path

CONFIG_DIR = (Path(os.path.expanduser("~/Library/Application Support/hermes-map")) if sys.platform == "darwin"
              else Path(os.path.expanduser("~/.config/hermes-map")))
CONFIG_FILE = CONFIG_DIR / "config.json"
ICLOUD = Path(os.path.expanduser("~/Library/Mobile Documents/com~apple~CloudDocs"))
HERMES_HOME = Path(os.path.expanduser("~/.hermes"))
ARCHIVE_NAMES = ("archive", "archives", "archivage")

DEFAULTS = {
    "version": 1,
    "root": None,              # dossier exploré
    "include": "*",            # dossiers de 1er niveau affichés (motifs séparés par des virgules)
    "folders": [],             # liste explicite des dossiers de notes affichés (vide = tous, selon « include »)
    "hidden_tops": [],         # dossiers de 1er niveau masqués (les nouveaux dossiers restent visibles)
    "title": "",               # nom du nœud central (défaut : nom du dossier)
    "zone_prefix": "",         # préfixe retiré du nom des zones (ex. « N_ » : N_Pro -> Pro)
    "ignore": [],              # dossiers ignorés en plus (bacs de tri, sauvegardes…)
    "archive": {
        "enabled": False,      # centraliser fichiers et binaires dans un dossier d'archivage
        "folder": "Archive",   # dossier d'archivage (1er niveau du dossier exploré)
        "catalog": "CATALOG.jsonl",   # fiches des fichiers archivés (dans le dossier d'archivage)
        "log": "LOG.md",              # journal des déplacements (dans le dossier d'archivage)
    },
    "hermes": {
        "enabled": True,
        "command": "",         # chemin de la commande hermes (vide = recherche automatique)
        "args": "",            # options ajoutées à « hermes chat »
        "profile": "",         # profil par défaut (vide = profil par défaut de Hermes)
        "reclass_profile": "", # profil pour le reclassement (vide = même que ci-dessus)
        "workdir": "",         # dossier de travail du reclassement, relatif au dossier exploré
        "conventions": [],     # fichiers de règles que Hermes doit suivre (relatifs au dossier exploré)
        "backup_skip": [],     # dossiers non sauvegardés avant un reclassement (trop gros, bacs de tri…)
    },
}


def _merge(base, extra):
    out = copy.deepcopy(base)
    for k, v in (extra or {}).items():
        if isinstance(v, dict) and isinstance(out.get(k), dict):
            out[k] = _merge(out[k], v)
        else:
            out[k] = v
    return out


class Settings:
    def __init__(self, data=None):
        self.data = _merge(DEFAULTS, data or {})

    # ---- dossier exploré
    @property
    def root(self):
        r = self.data.get("root")
        return Path(os.path.expanduser(r)).resolve() if r else None

    @property
    def include(self):
        s = (self.data.get("include") or "*").strip()
        return [] if s in ("", "*") else [x.strip() for x in s.split(",") if x.strip()]

    @property
    def title(self):
        return self.data.get("title") or (self.root.name if self.root else "Hermes Map")

    @property
    def zone_prefix(self):
        return self.data.get("zone_prefix") or ""

    @property
    def ignore(self):
        return set(self.data.get("ignore") or [])

    def zone_name(self, top: str) -> str:
        p = self.zone_prefix
        return top[len(p):] if p and top.startswith(p) else top

    def top_allowed(self, name: str) -> bool:
        if self.archive and name == self.archive:
            return True                       # le dossier d'archivage est toujours suivi
        folders = self.data.get("folders") or []
        if folders:
            return name in folders
        if name in (self.data.get("hidden_tops") or []):
            return False
        inc = self.include
        return not inc or any(fnmatch.fnmatch(name, pat) for pat in inc)

    # ---- archive
    @property
    def archive(self):
        """Nom du dossier d'archivage (ou None si l'option est désactivée)."""
        a = self.data["archive"]
        return a.get("folder") if a.get("enabled") and a.get("folder") else None

    @property
    def archive_catalog(self):
        return f"{self.archive}/{self.data['archive'].get('catalog') or 'CATALOG.jsonl'}" if self.archive else None

    @property
    def archive_log(self):
        return f"{self.archive}/{self.data['archive'].get('log') or 'LOG.md'}" if self.archive else None

    # ---- Hermes
    @property
    def hermes(self):
        return self.data["hermes"]

    def to_public(self):
        d = copy.deepcopy(self.data)
        d["configured"] = bool(self.root)
        d["config_file"] = str(CONFIG_FILE)
        d["archive_dir"] = self.archive
        return d

    def save(self):
        CONFIG_DIR.mkdir(parents=True, exist_ok=True)
        tmp = CONFIG_FILE.with_suffix(".tmp")
        tmp.write_text(json.dumps(self.data, ensure_ascii=False, indent=2), "utf-8")
        tmp.replace(CONFIG_FILE)


def load():
    """Réglages enregistrés, ou None au premier lancement."""
    try:
        return Settings(json.loads(CONFIG_FILE.read_text("utf-8")))
    except (OSError, ValueError):
        return None


current = Settings()


def set_current(s: Settings):
    global current
    current = s


# ---------------------------------------------------------------- détection (écran d'accueil)

def _profiles():
    base = HERMES_HOME / "profiles"
    marks = ("config.yaml", ".env", "SOUL.md", "profile.yaml", "auth.json", "state.db")
    try:
        return sorted(d.name for d in base.iterdir() if d.is_dir() and any((d / m).exists() for m in marks))
    except OSError:
        return []


def inspect(path: str):
    """Analyse un dossier candidat : accessible ? préfixe commun ? dossier d'archive déjà présent ?"""
    p = Path(os.path.expanduser(path or ""))
    out = {"path": str(p), "name": p.name or str(p)}
    if not p.is_dir():
        out["access"] = "missing"
        return out
    try:
        names = sorted(e.name for e in os.scandir(p) if e.is_dir() and not e.name.startswith("."))
    except PermissionError:
        out["access"] = "denied"
        return out
    except OSError:
        out["access"] = "missing"
        return out
    out["access"] = "ok"
    out["dirs"] = len(names)
    # préfixe partagé par plusieurs dossiers (ex. N_Pro, N_Perso… -> « N_ »)
    groups = {}
    for n in names:
        m = re.match(r"^([A-Za-z0-9]{1,4}[_\-.])\S", n)
        if m:
            groups.setdefault(m.group(1), []).append(n)
    prefix, members = "", []
    if groups:
        prefix, members = max(groups.items(), key=lambda kv: len(kv[1]))
        if len(members) < 3 or len(members) < 0.15 * len(names) and len(names) > 20:
            prefix, members = "", []
    shown = members or names
    out["all"] = names[:500]
    out["tops"] = shown[:24]
    out["more"] = max(0, len(shown) - 24)
    out["suggest"] = {"include": f"{prefix}*" if prefix else "*", "zone_prefix": prefix}
    arch = next((n for n in shown if (n[len(prefix):] if prefix else n).lower() in ARCHIVE_NAMES), None)
    out["archive_found"] = arch
    out["archive_default"] = arch or f"{prefix}Archive"
    out["obsidian"] = (p / ".obsidian").is_dir()
    out["hermes_home"] = p.resolve() == HERMES_HOME.resolve() if HERMES_HOME.exists() else False
    return out


def obsidian_vaults():
    f = (Path(os.path.expanduser("~/Library/Application Support/obsidian/obsidian.json")) if sys.platform == "darwin"
         else Path(os.path.expanduser("~/.config/obsidian/obsidian.json")))
    try:
        data = json.loads(f.read_text("utf-8"))
        return [v["path"] for v in (data.get("vaults") or {}).values() if v.get("path")]
    except (OSError, ValueError, AttributeError, TypeError):
        return []


def detect(find_hermes):
    """Ce que l'écran d'accueil propose : installation Hermes, dossiers candidats."""
    cmd = find_hermes()
    out = {"hermes": {"command": cmd, "home": str(HERMES_HOME) if HERMES_HOME.is_dir() else None,
                      "profiles": _profiles()},
           "candidates": [], "platform": sys.platform, "can_choose": sys.platform == "darwin" and bool(shutil.which("osascript"))}
    cands = []
    if ICLOUD.exists():
        cands.append(("icloud", "iCloud Drive", str(ICLOUD)))
    for v in obsidian_vaults():
        cands.append(("obsidian", "Coffre Obsidian « " + Path(v).name + " »", v))
    if HERMES_HOME.is_dir():
        cands.append(("hermes", "Dossier de Hermes (~/.hermes)", str(HERMES_HOME)))
    docs = Path(os.path.expanduser("~/Documents"))
    if docs.is_dir():
        cands.append(("documents", "Documents", str(docs)))
    seen = set()
    for kind, label, path in cands:
        if path in seen:
            continue
        seen.add(path)
        info = inspect(path)
        info.update(kind=kind, label=label)
        out["candidates"].append(info)
    return out


ARCHIVE_README = """# Archive

Ce dossier est géré avec **Hermes Map** : il centralise les fichiers « lourds » (images, vidéos, PDF,
documents…) pour que les autres dossiers ne contiennent que des notes.

## Règle de rangement

- Les notes (.md) restent dans leur dossier, par exemple `{exemple_note}`.
- Les fichiers qui vont avec sont rangés dans le dossier **miroir** de l'archive :
  `{exemple_miroir}` (même chemin, sous le nom de la zone en minuscules).
- Hermes Map affiche les deux ensemble : en ouvrant le dossier de la note, on voit aussi ses fichiers d'archive.

## Fichiers de suivi

- `{catalog}` : une fiche JSON par ligne (`rel` = chemin depuis ce dossier, `note`, `title`…).
- `{log}` : journal des déplacements, sous forme de tableau
  `| date ISO | action | from | to | ancien nom | nouveau nom | bytes | note |`.
"""

LOG_HEADER = ("# Journal des déplacements\n\n"
              "| date ISO | action | from | to | ancien nom | nouveau nom | bytes | note |\n"
              "|---|---|---|---|---|---|---|---|\n")


def init_archive(s: Settings):
    """Crée le dossier d'archivage et ses fichiers de suivi s'ils n'existent pas (ne remplace rien)."""
    if not s.archive or not s.root:
        return []
    made = []
    d = s.root / s.archive
    if d.exists():
        return made            # dossier déjà là : on n'y ajoute rien
    d.mkdir(parents=True)
    made.append(s.archive)
    tops = sorted(e.name for e in s.root.iterdir() if e.is_dir() and s.top_allowed(e.name) and e.name != s.archive)
    ex = tops[0] if tops else "Projets"
    zone = s.zone_name(ex)
    readme = d / "README.md"
    if not readme.exists() and not any((d / n).exists() for n in ("AGENTS.md", "INDEX.md")):
        readme.write_text(ARCHIVE_README.format(
            exemple_note=f"{ex}/voyages/japon.md", exemple_miroir=f"{s.archive}/{zone.lower()}/voyages/photo.jpg",
            catalog=s.data["archive"]["catalog"], log=s.data["archive"]["log"]), "utf-8")
        made.append(f"{s.archive}/README.md")
    log = s.root / s.archive_log
    if not log.exists():
        log.write_text(LOG_HEADER, "utf-8")
        made.append(s.archive_log)
    cat = s.root / s.archive_catalog
    if not cat.exists():
        cat.write_text("", "utf-8")
        made.append(s.archive_catalog)
    return made
