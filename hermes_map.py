#!/usr/bin/env python3
"""Hermes Map — explorateur visuel (mind map) des dossiers Hermes.

Lancement :  python3 hermes_map.py
             -> au premier lancement, un écran d'accueil demande quel dossier explorer
             python3 hermes_map.py ~/Notes --only '*'        (dossier imposé, sans écran d'accueil)

Le serveur n'écoute que sur 127.0.0.1 (ta machine uniquement) et ne fait
que LIRE les fichiers : rien n'est modifié.
Aucune dépendance : Python 3 standard suffit.
"""
import argparse
import fnmatch
import json
import mimetypes
import os
import posixpath
import re
import hashlib
import secrets
import unicodedata
import shutil
import sqlite3
import subprocess
import tempfile
import sys
import threading
import time
import webbrowser
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from pathlib import Path
from urllib.parse import urlparse, parse_qs, unquote

APP_DIR = Path(__file__).resolve().parent
VERSION = "2026.10.26"
sys.path.insert(0, str(APP_DIR))
from hermes_runner import Runner  # noqa: E402
import media_tools  # noqa: E402
import mover  # noqa: E402
import settings  # noqa: E402
import i18n  # noqa: E402
from hermes_runner import find_hermes  # noqa: E402
STARTED = int(time.time())          # le lanceur compare avec la date des fichiers pour repérer une ancienne version
TOKEN = secrets.token_urlsafe(24)   # jeton anti-CSRF pour les actions qui écrivent (bouton Hermes)
STATIC = APP_DIR / "static"

# Dossiers lourds / techniques ignorés par défaut (code source, environnements…)
DEFAULT_EXCLUDES = {
    ".git", "node_modules", "__pycache__", ".venv", "venv", "site-packages",
    ".mypy_cache", ".pytest_cache", ".ruff_cache", ".DS_Store", ".obsidian", ".trash",
}
MAX_INDEX = 400_000        # nb max d'éléments dans l'index de recherche
AUTO_TAGS = 40             # nb de tags automatiques (mots récurrents dans les noms)
AUTO_TAG_LINKS = 25        # liens affichés par tag automatique
MAX_REFS = 4000           # nb max de liens entre fichiers affichés
MAX_REFS_PER_FILE = 12
MAX_NODES = 4000          # au-delà, les dossiers trop gros sont résumés
MAX_CHILDREN = 250        # nb max d'éléments affichés par dossier dans le graphe
MAX_TEXT = 2_000_000      # taille max lue pour un fichier texte (2 Mo)
SCAN_TEXT_LIMIT = 400_000 # taille max analysée pour trouver des liens

TEXT_EXT = {
    ".md", ".markdown", ".txt", ".yaml", ".yml", ".json", ".jsonl", ".toml",
    ".ini", ".cfg", ".conf", ".py", ".sh", ".zsh", ".bash", ".js", ".ts",
    ".mjs", ".css", ".html", ".htm", ".xml", ".csv", ".tsv", ".log", ".env",
    ".rs", ".go", ".rb", ".lua", ".sql", ".j2", ".jinja", ".tmpl", ".rst",
    ".org", ".tex", ".svg", ".gitignore", ".lock", ".service", ".plist", ".url", ".webloc",
}
IMAGE_EXT = {".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico", ".svg", ".avif", ".heic", ".tif", ".tiff"}
AUDIO_EXT = {".mp3", ".wav", ".ogg", ".m4a", ".flac", ".aac", ".opus"}
VIDEO_EXT = media_tools.WEB_VIDEO_EXT | media_tools.OTHER_VIDEO_EXT
DB_EXT = {".db", ".sqlite", ".sqlite3"}
ACTIVE_EXT = {".html", ".htm", ".xhtml", ".shtml", ".svg", ".svgz", ".xml", ".xsl", ".xslt", ".mht", ".mhtml"}
# page principale : pas d'intégration dans un autre site, pas de connexion vers l'extérieur
INDEX_CSP = ("default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; "
             "img-src 'self' data: blob: https:; media-src 'self' blob:; font-src 'self' data:; connect-src 'self'; "
             "frame-src 'self'; worker-src 'self' blob:; object-src 'none'; base-uri 'none'; form-action 'none'; "
             "frame-ancestors 'none'")
SF_DATALESS = 0x40000000  # macOS : fichier iCloud présent dans le cloud mais pas sur le disque
SECRET_NAMES = {".env", "auth.json", "credentials.json", "secrets.json", "tokens.json"}
SECRET_KEY_RE = re.compile(r"(key|token|secret|password|passwd|pwd|auth|bearer|cookie|session_id|private)", re.I)

HASHTAG_RE = re.compile(r"(?<![\w&#/=:])#([A-Za-zÀ-ÿ][\w\-/À-ÿ]{1,40})")
FENCE_RE = re.compile(r"```.*?```", re.S)
FM_RE = re.compile(r"^---\s*\n(.*?)\n---", re.S)
MAX_TAG_LINKS = 4000


def extract_tags(txt: str, is_md: bool):
    """Tags du front-matter YAML (tags: / tag:) et #hashtags dans le texte Markdown."""
    tags = set()
    m = FM_RE.match(txt)
    if m:
        lines = m.group(1).splitlines()
        for i, line in enumerate(lines):
            mm = re.match(r"^(tags?|keywords)\s*:\s*(.*)$", line.strip(), re.I)
            if not mm:
                continue
            val = mm.group(2).strip()
            if val:
                for t in re.split(r"[,\s]+", val.strip("[]")):
                    t = t.strip().strip("\"'#")
                    if t:
                        tags.add(t)
            else:
                for nxt in lines[i + 1:]:
                    li = re.match(r"^\s*-\s*(.+)$", nxt)
                    if not li:
                        break
                    tags.add(li.group(1).strip().strip("\"'#"))
    if is_md:
        body = FENCE_RE.sub("", txt[m.end():] if m else txt)
        body = re.sub(r"`[^`]*`", "", body)
        for h in HASHTAG_RE.findall(body):
            if re.fullmatch(r"[0-9a-fA-F]{3,8}", h):
                continue  # couleurs hexadécimales
            tags.add(h)
    return {t.lower().rstrip("/") for t in tags if 1 < len(t) <= 40}


def norm_text(t: str) -> str:
    """minuscules sans accents, pour comparer « Été » et « ete »."""
    t = unicodedata.normalize("NFKD", t.lower())
    return "".join(c for c in t if not unicodedata.combining(c))


STOPWORDS = set("""
le la les un une des du de d l et ou en au aux a pour par sur sous avec sans dans ce ces cet cette
mon ma mes ton ta tes son sa ses notre nos votre vos leur leurs qui que quoi dont est sont
the a an of to in on for with and or by from at as is are be this that it its my your
copy copie final new nouveau nouvelle old ancien untitled sans titre document doc docs file files
fichier fichiers dossier dossiers folder image images img photo photos picture pictures pic video videos vid
screenshot capture ecran scan scans export exported version draft brouillon test tmp temp misc divers
autre autres other others jpg jpeg png gif webp heic mp4 mov mkv webm pdf txt md json zip dsc dcim
skill skills script scripts run readme index main src lib assets asset data cache log logs backup sauvegarde tmp bin config conf settings default
""".split())


def name_tokens(name: str):
    """Mots significatifs d'un nom de fichier ou de dossier (pour les tags automatiques)."""
    stem = name.rsplit(".", 1)[0] if "." in name[1:] else name
    stem = re.sub(r"([a-zà-ÿ])([A-Z])", r"\1 \2", stem)           # CamelCase -> mots
    out = set()
    for w in re.split(r"[^0-9A-Za-zÀ-ÿ]+", stem):
        w = norm_text(w)
        if len(w) < 3 or len(w) > 30 or w in STOPWORDS:
            continue
        if any(c.isdigit() for c in w):                              # dates, numéros, identifiants
            continue
        out.add(w)
    return out


WIKILINK_RE = re.compile(r"\[\[([^\]|#]+)(?:#[^\]|]*)?(?:\|[^\]]*)?\]\]")
MDLINK_RE = re.compile(r"\]\((<[^>\n]+>|[^)\s]+)(?:\s+\"[^\"]*\")?\)")
ROOT_PREFIX = [""]   # chemin absolu du dossier racine, pour lire les liens absolus


def href_target(parent: str, raw: str):
    """Cible d'un lien Markdown, relative à la racine : gère <chemin avec espaces>, %20 et les chemins absolus."""
    h = raw[1:-1] if raw.startswith("<") and raw.endswith(">") else unquote(raw)
    if h.startswith(("http:", "https:", "mailto:", "#", "data:", "file:")):
        return None
    h = h.split("#")[0]
    if not h:
        return None
    if h.startswith("/"):
        root = ROOT_PREFIX[0]
        if root and h.startswith(root + "/"):
            return posixpath.normpath(h[len(root) + 1:])
        return None
    j = posixpath.normpath(posixpath.join(parent, h)) if parent else posixpath.normpath(h)
    return "" if j == "." else j.rstrip("/")
PATHLIKE_RE = re.compile(r"(?:~/\.hermes/|\./|\.\./)?(?:[\w.\-]+/)+[\w.\-]+\.\w{1,6}")


def icloud_real_name(name: str):
    """iCloud remplace un fichier non téléchargé par « .nom.icloud »."""
    if name.startswith(".") and name.endswith(".icloud") and len(name) > 8:
        return name[1:-7]
    return None


def kind_of(p: Path) -> str:
    if icloud_real_name(p.name):
        return "icloud"
    if p.is_dir():
        return "dir"
    ext = p.suffix.lower()
    name = p.name.lower()
    if name in SECRET_NAMES or name.startswith(".env"):
        return "secret"
    if ext in IMAGE_EXT:
        return "image"
    if ext == ".pdf":
        return "pdf"
    if ext in AUDIO_EXT:
        return "audio"
    if ext in VIDEO_EXT:
        return "video"
    if ext in DB_EXT:
        return "db"
    if ext in media_tools.OFFICE_EXT:
        return "office"
    if ext in (".md", ".markdown"):
        return "markdown"
    if ext in (".json", ".jsonl"):
        return "json"
    if ext in (".yaml", ".yml", ".toml", ".ini", ".cfg", ".conf"):
        return "config"
    if ext in TEXT_EXT or ext == "":
        return "text"
    return "binary"


def is_texty(p: Path) -> bool:
    if kind_of(p) in ("markdown", "json", "config", "text", "secret"):
        try:
            with open(p, "rb") as f:
                chunk = f.read(4096)
            return b"\x00" not in chunk
        except OSError:
            return False
    return False


class Scanner:
    def __init__(self, root: Path, excludes, show_hidden: bool, cfg=None):
        self.root = root
        self.cfg = cfg or settings.current
        self.excludes = set(excludes) | self.cfg.ignore
        self.show_hidden = show_hidden
        self.only = self.cfg.include   # motifs pour filtrer les dossiers de 1er niveau (ex. H_*)
        self.tag_index = {}      # tag -> [chemins]
        self.index = None        # [(rel, nom, is_dir, taille, mtime, cloud)] : TOUT l'arbre, sans limite d'affichage
        self.index_status = "pas encore construit"
        self.index_paths = set()
        self.index_dirs = set()
        self.name_map = {}

    def top_allowed(self, name: str) -> bool:
        return self.cfg.top_allowed(name)

    def allowed(self, p: Path) -> bool:
        rel = p.relative_to(self.root).as_posix()
        if rel in ("", "."):
            return True
        return self.top_allowed(rel.split("/")[0])

    def skip(self, e: Path, depth: int) -> bool:
        if e.name in self.excludes:
            return True
        if depth == 1 and not self.top_allowed(e.name):
            return True
        if icloud_real_name(e.name):
            return False
        if not self.show_hidden and e.name.startswith(".") and e.name not in SECRET_NAMES and not e.name.startswith(".env"):
            return True
        return False

    def mirror_of(self, rel: str):
        """Dossier « miroir » dans l'archive d'un dossier de notes : Photos/japon -> Archive/photos/japon."""
        rel = (rel or "").strip("/")
        arch = self.cfg.archive
        if not rel or not arch:
            return None
        top, _, rest = rel.partition("/")
        if top == arch or arch not in self.index_dirs:
            return None
        area = self.cfg.zone_name(top)
        cands = [f"{arch}/{area.lower()}/{rest}".rstrip("/"), f"{arch}/{area}/{rest}".rstrip("/")]
        if rest:
            cands.append(f"{arch}/{rest}")
        for c in cands:
            if c in self.index_dirs:
                return c
        return None

    def brain_of(self, rel: str):
        """Inverse : Archive/photos/japon -> Photos/japon (s'il existe)."""
        parts = (rel or "").strip("/").split("/")
        arch = self.cfg.archive
        if not arch or len(parts) < 2 or parts[0] != arch:
            return None
        area, rest = parts[1].lower(), parts[2:]
        for d in self.index_dirs:
            if "/" not in d and d != arch and self.cfg.zone_name(d).lower() == area:
                cand = "/".join([d] + rest)
                return cand if cand in self.index_dirs else (d if not rest else None)
        return None

    def catalog(self):
        """Fiches des catalog.yaml (descriptions des médias), indexées par chemin depuis la racine."""
        cats = [r for r in self.index_paths if r.rsplit("/", 1)[-1].lower() in ("catalog.yaml", "catalog.yml", "catalog.jsonl")]
        sig = tuple(sorted((c, (self.root / c).stat().st_mtime if (self.root / c).exists() else 0) for c in cats))
        if getattr(self, "_cat_sig", None) == sig:
            return self._cat
        out = {}
        for c in cats:
            base = posixpath.dirname(c)
            if c.lower().endswith(".jsonl"):
                # catalogue de l'archive : une fiche JSON par ligne, « rel » relatif au dossier du catalogue
                try:
                    with open(self.root / c, encoding="utf-8", errors="replace") as fh:
                        for line in fh:
                            try:
                                e = json.loads(line)
                            except ValueError:
                                continue
                            if not isinstance(e, dict) or not e.get("rel"):
                                continue
                            target = posixpath.normpath(posixpath.join(base, e["rel"]))
                            d = {k: e.get(k) for k in ("title", "series", "silo", "pages", "url", "source")
                                 if e.get(k) not in (None, "")}
                            desc = e.get("description") or e.get("note") or e.get("snippet")
                            if desc:
                                d["description"] = re.sub(r"\s+", " ", str(desc)).strip()[:600]
                            if d:
                                d["catalog"] = c
                                out.setdefault(target, {}).update({k: v for k, v in d.items() if k not in out.get(target, {})})
                except OSError:
                    pass
                continue
            try:
                ents = parse_catalog((self.root / c).read_text("utf-8", errors="replace"))
            except OSError:
                continue
            for e in ents:
                target = None
                if e.get("root") and e.get("path"):
                    target = posixpath.normpath(f"{e['root']}/{e['path']}")
                elif e.get("link"):
                    target = posixpath.normpath(posixpath.join(base, str(e["link"])))
                elif e.get("path"):
                    target = posixpath.normpath(posixpath.join(base, str(e["path"])))
                if target:
                    d = {k: e.get(k) for k in ("description", "series", "character", "artist", "kind")
                         if e.get(k) not in (None, "")}
                    d["catalog"] = c
                    out[target] = {**out.get(target, {}), **d}
        self._cat_sig, self._cat = sig, out
        return out

    def rel(self, p: Path) -> str:
        r = p.relative_to(self.root).as_posix()
        return "" if r == "." else r

    def scan(self, publish, progress):
        """Phase 1 : arborescence (rapide, publiée tout de suite). Phase 2 : liens entre fichiers."""
        nodes, links = [], []
        files_by_name = {}   # nom de fichier / stem -> [ids]
        skill_names = {}     # nom de skill -> id du dossier
        text_files = []
        count = 0
        truncated = False
        t0 = time.time()

        root_id = "/"
        title = self.cfg.title
        nodes.append({"id": root_id, "name": title, "kind": "root",
                      "path": "", "group": "root", "depth": 0})

        def list_dir(d: Path, depth: int):
            try:
                entries = [de for de in os.scandir(d) if not de.is_symlink() and not self.skip(Path(de.path), depth)]
            except OSError:
                return []
            entries.sort(key=lambda e: (not e.is_dir(follow_symlinks=False), e.name.lower()))
            return entries

        def bfs(top_entry, budget):
            """Parcours niveau par niveau d'un dossier de 1er niveau, limité à `budget` nœuds."""
            g_nodes, g_links, g_files, g_skills = [], [], [], {}
            n = 0
            trunc = False
            queue = [(None, root_id, 1, [top_entry])]
            while queue:
                next_level = []
                for d, parent_id, depth, entries in queue:
                    if entries is None:
                        entries = list_dir(d, depth)
                    shown = hidden_extra = 0
                    for de in entries:
                        if shown >= MAX_CHILDREN or n >= budget:
                            hidden_extra += 1
                            continue
                        try:
                            st = de.stat(follow_symlinks=False)
                        except OSError:
                            continue
                        e = Path(de.path)
                        rel = self.rel(e)
                        g = rel.split("/")[0]
                        is_dir = de.is_dir(follow_symlinks=False)
                        k = "dir" if is_dir else kind_of(e)
                        cloud = bool(getattr(st, "st_flags", 0) & SF_DATALESS) and not is_dir
                        node = {"id": rel, "name": icloud_real_name(e.name) or e.name, "kind": k, "path": rel,
                                "group": g, "depth": depth, "size": st.st_size if not is_dir else None,
                                "mtime": st.st_mtime}
                        if cloud:
                            node["cloud"] = True
                        if is_dir and os.path.isfile(os.path.join(de.path, "SKILL.md")):
                            node["skill"] = True
                            g_skills[e.name.lower()] = rel
                        g_nodes.append(node)
                        g_links.append({"source": parent_id, "target": rel, "type": "tree"})
                        n += 1
                        shown += 1
                        if is_dir:
                            next_level.append((e, rel, depth + 1, None))
                        elif (k in ("markdown", "text", "config", "json") and not cloud
                              and st.st_size <= SCAN_TEXT_LIMIT):
                            g_files.append((e, rel))
                    if hidden_extra and d is not None:
                        trunc = True
                        more_id = self.rel(d) + "/…more"
                        g_nodes.append({"id": more_id, "name": f"+{hidden_extra} autres", "kind": "more",
                                        "path": self.rel(d), "group": self.rel(d).split("/")[0], "depth": depth})
                        g_links.append({"source": parent_id, "target": more_id, "type": "tree"})
                queue = next_level
            return {"nodes": g_nodes, "links": g_links, "files": g_files, "skills": g_skills,
                    "count": n, "truncated": trunc}

        tops = list_dir(self.root, 1)
        # Partage équitable du quota entre dossiers de 1er niveau ; la part inutilisée est redistribuée
        results = {}
        pending = list(tops)
        remaining = MAX_NODES
        while pending:
            share = max(50, remaining // len(pending))
            still = []
            for de in pending:
                progress(f"Lecture de {de.name}…")
                r = bfs(de, share)
                results[de.name] = r
                if r["truncated"] and r["count"] >= share:
                    still.append(de)
                else:
                    remaining -= r["count"]
            if len(still) == len(pending) or not still:
                break
            pending = still   # on relance les gros dossiers avec une part plus grande

        for de in tops:
            r = results[de.name]
            nodes += r["nodes"]
            links += r["links"]
            text_files += r["files"]
            skill_names.update(r["skills"])
            truncated = truncated or r["truncated"]
            count += r["count"]
        for nd in nodes:
            if nd["kind"] not in ("dir", "more", "root"):
                files_by_name.setdefault(nd["name"].lower(), []).append(nd["id"])
                files_by_name.setdefault(posixpath.splitext(nd["name"])[0].lower(), []).append(nd["id"])

        result = {"root": str(self.root), "title": title, "nodes": nodes, "links": list(links),
                  "truncated": truncated, "scanned_at": time.time(), "refs_done": False}
        publish(result)
        print(f"  Arborescence : {count} éléments en {time.time()-t0:.1f} s")

        # ---- Phase 1b : index complet (tous les fichiers, toutes profondeurs) pour la recherche
        t_idx = time.time()
        index = []
        stack = [(Path(de.path), 2) for de in tops if de.is_dir(follow_symlinks=False)]
        for de in tops:
            try:
                st = de.stat(follow_symlinks=False)
            except OSError:
                continue
            index.append((self.rel(Path(de.path)), de.name, de.is_dir(follow_symlinks=False), st.st_size,
                          st.st_mtime, False))
        while stack and len(index) < MAX_INDEX and time.time() - t_idx < 120:
            d, depth = stack.pop()
            try:
                it = list(os.scandir(d))
            except OSError:
                continue
            for de in it:
                if de.is_symlink() or self.skip(Path(de.path), depth):
                    continue
                try:
                    st = de.stat(follow_symlinks=False)
                except OSError:
                    continue
                is_dir = de.is_dir(follow_symlinks=False)
                rel = self.rel(Path(de.path))
                index.append((rel, icloud_real_name(de.name) or de.name, is_dir,
                              st.st_size if not is_dir else 0, st.st_mtime,
                              (bool(getattr(st, "st_flags", 0) & SF_DATALESS) or bool(icloud_real_name(de.name)))
                              and not is_dir))
                if is_dir:
                    stack.append((Path(de.path), depth + 1))
            if len(index) % 2000 < 50:
                progress(f"Indexation pour la recherche : {len(index)} éléments…")
        self.index = index
        self.index_status = f"{len(index)} éléments indexés"
        self.index_paths = {r[0] for r in index}
        self.index_dirs = {r[0] for r in index if r[2]}
        name_map = {}
        for rel, name, is_dir, *_ in index:
            if is_dir:
                continue
            low = name.lower()
            name_map.setdefault(low, []).append(rel)
            name_map.setdefault(low.rsplit(".", 1)[0], []).append(rel)
        self.name_map = name_map
        print(f"  Index de recherche : {len(index)} éléments en {time.time()-t_idx:.1f} s")

        # ---- Phase 2 : liens entre fichiers (sans accès disque pour résoudre les chemins)
        ids = {n["id"] for n in nodes}
        seen = set()
        refs = []

        per_src = {}

        def add_ref(src, dst, why):
            if not dst or dst == src or dst not in ids:
                return
            if len(refs) >= MAX_REFS or per_src.get(src, 0) >= MAX_REFS_PER_FILE:
                return
            if (src, dst) in seen or (dst, src) in seen:
                return
            seen.add((src, dst))
            per_src[src] = per_src.get(src, 0) + 1
            refs.append({"source": src, "target": dst, "type": "ref", "why": why})

        def norm(base_rel, href):
            joined = posixpath.normpath(posixpath.join(base_rel, href)) if base_rel else posixpath.normpath(href)
            return "" if joined == "." else joined

        skill_re = None
        if skill_names:
            names = sorted((re.escape(s) for s in skill_names if len(s) >= 4), key=len, reverse=True)
            if names:
                skill_re = re.compile(r"(?<![\w-])(" + "|".join(names) + r")(?![\w-])", re.I)

        # alterner entre dossiers de 1er niveau (et les .md d'abord) pour que chaque zone ait ses liens
        by_group = {}
        for f, rel in sorted(text_files, key=lambda x: (not x[1].lower().endswith(".md"), x[1].count("/"))):
            by_group.setdefault(rel.split("/")[0], []).append((f, rel))
        text_files = []
        lists = list(by_group.values())
        for i in range(max((len(l) for l in lists), default=0)):
            for l in lists:
                if i < len(l):
                    text_files.append(l[i])

        tag_index = {}
        budget_bytes, read_bytes = 80_000_000, 0
        t1 = time.time()
        for i, (f, src) in enumerate(text_files):
            if read_bytes > budget_bytes or time.time() - t1 > 90:
                print("  (analyse des liens arrêtée : limite de temps/volume atteinte)")
                break
            if i % 100 == 0:
                progress(f"Recherche des liens : {i}/{len(text_files)} fichiers…")
            try:
                with open(f, "rb") as fh:
                    raw = fh.read(SCAN_TEXT_LIMIT)
            except OSError:
                continue
            read_bytes += len(raw)
            if b"\x00" in raw[:4096]:
                continue
            txt = raw.decode("utf-8", errors="ignore")
            for t in extract_tags(txt, src.lower().endswith((".md", ".markdown"))):
                tag_index.setdefault(t, []).append(src)
            parent = posixpath.dirname(src)
            own_skill = None
            parts = src.split("/")
            for j in range(len(parts) - 1, -1, -1):
                if parts[j].lower() in skill_names:
                    own_skill = parts[j].lower()
                    break
            for m in WIKILINK_RE.finditer(txt):
                target = m.group(1).strip().lower()
                for cand in files_by_name.get(target, [])[:5] + files_by_name.get(target + ".md", [])[:5]:
                    add_ref(src, cand, "wikilink")
                if target in skill_names:
                    add_ref(src, skill_names[target], "wikilink")
            for m in MDLINK_RE.finditer(txt):
                add_ref(src, href_target(parent, m.group(1)), "lien")
            for m in PATHLIKE_RE.finditer(txt):
                ptxt = m.group(0).replace("~/.hermes/", "")
                for cand in (norm("", ptxt), norm(parent, ptxt)):
                    if cand in ids:
                        add_ref(src, cand, "chemin")
                        break
            if skill_re:
                for m in skill_re.finditer(txt):
                    sname = m.group(1).lower()
                    if sname != own_skill:
                        add_ref(src, skill_names[sname], "mention de skill")

        # ---- liens « miroir » : dossier de notes <-> son dossier dans l'archive
        arch = self.cfg.archive
        for n in nodes:
            if arch and n.get("kind") == "dir" and n["id"].split("/")[0] != arch:
                m = self.mirror_of(n["id"])
                if m:
                    add_ref(n["id"], m, "miroir")

        # ---- tags automatiques : mots qui reviennent dans les noms de plusieurs branches
        progress("Calcul des tags automatiques…")
        word_items, word_dirs, word_zones = {}, {}, {}
        zone_names = {norm_text(de.name) for de in tops} | {norm_text(self.cfg.zone_name(de.name)) for de in tops}
        for rel, name, is_dir, *_ in self.index or []:
            parent = posixpath.dirname(rel)
            zone = rel.split("/")[0]
            for w in name_tokens(name):
                if w in zone_names:
                    continue
                word_items.setdefault(w, []).append((rel, is_dir))
                word_dirs.setdefault(w, set()).add(parent)
                word_zones.setdefault(w, set()).add(zone)
        cands = []
        for w, items in word_items.items():
            nd = len(word_dirs[w])
            if len(items) < 3 or nd < 2:
                continue
            if len(items) > max(400, len(self.index or []) // 8):
                continue                       # mot trop banal (présent partout)
            score = len(word_zones[w]) * 1000 + nd * 10 + min(len(items), 200) / 100
            cands.append((score, w))
        cands.sort(reverse=True)
        auto_index = {}
        for _, w in cands[:AUTO_TAGS]:
            if w in tag_index:
                continue
            # dossiers d'abord : un dossier « jardin » résume mieux que ses 50 fichiers
            items = sorted(word_items[w], key=lambda x: (not x[1], x[0].count("/"), x[0]))
            auto_index[w] = [rel for rel, _ in items[:2000]]

        # nœuds « tag » reliés aux éléments qui les portent
        tag_nodes, tag_links = [], []
        by_id = {n["id"]: n for n in nodes}
        for t, files in sorted(tag_index.items(), key=lambda kv: -len(kv[1])):
            tid = "#tag:" + t
            tag_nodes.append({"id": tid, "name": "#" + t, "kind": "tag", "path": tid, "group": "#tags",
                              "depth": 1, "count": len(files), "auto": False})
            for f in files:
                by_id[f].setdefault("tags", []).append(t)
                if len(tag_links) < MAX_TAG_LINKS:
                    tag_links.append({"source": tid, "target": f, "type": "tag"})
        for t, files in auto_index.items():
            tid = "#tag:" + t
            tag_nodes.append({"id": tid, "name": t, "kind": "tag", "path": tid, "group": "#tags",
                              "depth": 1, "count": len(files), "auto": True,
                              "zones": sorted(word_zones[t])})
            shown = 0
            for f in files:
                n = by_id.get(f)
                if n is None:
                    continue
                n.setdefault("tags", []).append(t)
                if shown < AUTO_TAG_LINKS:
                    tag_links.append({"source": tid, "target": f, "type": "tag"})
                    shown += 1
            if shown == 0:   # aucun élément visible sur la carte : relier au dossier visible le plus proche
                seen_p = set()
                for f in files:
                    p = f
                    while p and p not in by_id:
                        p = posixpath.dirname(p)
                    if p and p not in seen_p:
                        seen_p.add(p)
                        tag_links.append({"source": tid, "target": p, "type": "tag"})
                        if len(seen_p) >= AUTO_TAG_LINKS:
                            break
        self.tag_index = {**tag_index, **auto_index}
        print(f"  Tags : {len(tag_index)} dans les notes, {len(auto_index)} automatiques")
        result = dict(result, nodes=nodes + tag_nodes, links=links + refs + tag_links,
                      refs_done=True, scanned_at=time.time())
        publish(result)
        print(f"  Liens : {len(refs)} trouvés en {time.time()-t1:.1f} s")


def parse_catalog(txt: str):
    """Lit un catalog.yaml (liste « entries: » de fiches). PyYAML s'il est installé, sinon lecteur simple."""
    try:
        import yaml  # type: ignore
        data = yaml.safe_load(txt) or {}
        ents = data.get("entries", data if isinstance(data, list) else [])
        return [e for e in ents if isinstance(e, dict)]
    except Exception:  # noqa
        pass
    entries, cur, key, block, block_ind = [], None, None, None, 0
    for raw in txt.splitlines():
        if raw.lstrip().startswith("#") and block is None:
            continue
        if block is not None:
            ind = len(raw) - len(raw.lstrip())
            if raw.strip() == "" or ind > block_ind:
                block.append(raw.strip())
                continue
            cur[key] = " ".join(x for x in block if x).strip()
            block = None
        m = re.match(r"^(\s*)-\s+([\w.-]+)\s*:\s*(.*)$", raw)
        if m:
            cur = {}
            entries.append(cur)
            ind, key, val = len(m.group(1)) + 2, m.group(2), m.group(3)
        else:
            m = re.match(r"^(\s+)([\w.-]+)\s*:\s*(.*)$", raw)
            if not m or cur is None:
                continue
            ind, key, val = len(m.group(1)), m.group(2), m.group(3)
        val = val.strip()
        if val in (">", ">-", "|", "|-", ">+", "|+"):
            block, block_ind = [], ind
            continue
        if val in ("null", "~", ""):
            val = None
        elif len(val) >= 2 and val[0] == val[-1] and val[0] in "\"'":
            val = val[1:-1]
        elif val in ("true", "false"):
            val = val == "true"
        cur[key] = val
    if block is not None and cur is not None:
        cur[key] = " ".join(x for x in block if x).strip()
    return entries


def mask_secret_text(txt: str) -> str:
    out = []
    for line in txt.splitlines():
        m = re.match(r"^(\s*(?:export\s+)?[\w.\-]+\s*[=:]\s*)(.*)$", line)
        if m and m.group(2).strip():
            v = m.group(2).strip().strip('"\'')
            out.append(m.group(1) + ("•" * 8 + v[-4:] if len(v) > 8 else "•" * 8))
        else:
            out.append(line)
    return "\n".join(out)


def mask_json(obj):
    if isinstance(obj, dict):
        return {k: ("••••••••" if SECRET_KEY_RE.search(str(k)) and isinstance(v, (str, int)) else mask_json(v))
                for k, v in obj.items()}
    if isinstance(obj, list):
        return [mask_json(v) for v in obj]
    return obj


THUMB_DIR = Path(os.path.expanduser("~/Library/Caches/hermes-map/thumbs")) if sys.platform == "darwin" \
    else Path(tempfile.gettempdir()) / "hermes-map-thumbs"
THUMB_SIZE = 360
_thumb_sem = threading.Semaphore(4)


def make_thumb(p: Path, kind: str):
    """Miniature JPEG/PNG mise en cache. Renvoie le chemin, ou None si impossible."""
    st = p.stat()
    key = hashlib.sha1(f"{p}|{st.st_size}|{st.st_mtime}".encode()).hexdigest()
    THUMB_DIR.mkdir(parents=True, exist_ok=True)
    out = THUMB_DIR / (key + ".jpg")
    if out.exists() and out.stat().st_size > 0:
        return out
    with _thumb_sem:
        if out.exists() and out.stat().st_size > 0:
            return out
        try:
            if kind == "image" and shutil.which("sips"):
                subprocess.run(["sips", "-s", "format", "jpeg", "-Z", str(THUMB_SIZE), str(p), "--out", str(out)],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
            elif kind in ("video", "pdf", "office", "text", "markdown", "json", "config") and shutil.which("qlmanage"):
                # Quick Look : la même vignette que dans le Finder (1re page d'un PDF / Word / Excel…)
                tmp = Path(tempfile.mkdtemp(prefix="hm-"))
                subprocess.run(["qlmanage", "-t", "-s", str(THUMB_SIZE), "-o", str(tmp), str(p)],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
                made = list(tmp.glob("*.png"))
                if made:
                    shutil.move(str(made[0]), str(out))
                shutil.rmtree(tmp, ignore_errors=True)
            if kind == "video" and not (out.exists() and out.stat().st_size > 0):
                ff = media_tools.find_ffmpeg()
                if ff:
                    subprocess.run([ff, "-hide_banner", "-loglevel", "error", "-y", "-ss", "1", "-i", str(p),
                                    "-frames:v", "1", "-vf", f"scale={THUMB_SIZE}:-2", str(out)],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60)
        except (OSError, subprocess.SubprocessError):
            return None
    return out if out.exists() and out.stat().st_size > 0 else None


class App:
    def __init__(self, reveal_secrets: bool, opts=None):
        self.opts = opts or {}         # options de la ligne de commande (all, hidden, hermes, hermes_args, no_hermes)
        self.runner = None
        self.last_ping = 0.0
        self.video = media_tools.VideoConverter()
        self.root = None
        self.scanner = None
        self.cfg = settings.current
        self.setup = {"reason": "first"}   # tant que rien n'est configuré : écran d'accueil
        self.reveal = reveal_secrets
        self.cache = None
        self.status = "Démarrage…"
        self.scanning = False
        self.gen = 0                   # change à chaque nouvelle configuration
        self.lock = threading.Lock()

    def configure(self, cfg):
        """Applique des réglages : dossier exploré, archive, Hermes. Renvoie None ou la raison d'un échec."""
        if self.runner and self.runner.job and self.runner.job.state in ("preparing", "running"):
            return "busy"
        root = cfg.root
        if not root:
            self.setup = {"reason": "first"}
            return "first"
        try:
            os.listdir(root)
        except PermissionError:
            self.setup = {"reason": "denied", "path": str(root)}
            return "denied"
        except OSError:
            self.setup = {"reason": "missing", "path": str(root)}
            return "missing"
        settings.set_current(cfg)
        self.cfg = cfg
        ROOT_PREFIX[0] = str(root)
        o = self.opts
        scanner = Scanner(root, set() if o.get("all") else DEFAULT_EXCLUDES, o.get("hidden", False), cfg)
        h = cfg.hermes
        runner = None
        if not o.get("no_hermes") and h.get("enabled", True):
            runner = Runner(root, o.get("hermes") or h.get("command") or None, o.get("hermes_args") or h.get("args") or "")
        with self.lock:
            self.root, self.scanner, self.runner = root, scanner, runner
            self.cache, self.setup = None, None
            self.gen += 1
        if runner:
            runner.links_of = self.links_of
        tops = [e.name for e in root.iterdir() if e.is_dir() and scanner.top_allowed(e.name)]
        print(f"Dossier : {root}\nDossiers retenus ({len(tops)}) : {', '.join(sorted(tops)) or 'aucun !'}")
        if runner:
            print(f"Hermes : {'commande ' + runner.hermes if runner.hermes else 'commande « hermes » introuvable'}")
        self.start_scan()
        return None

    def _publish(self, result):
        with self.lock:
            self.cache = result

    def _progress(self, msg):
        self.status = msg

    def start_scan(self):
        with self.lock:
            if self.scanning:
                return
            self.scanning = True

        def run():
            gen = self.gen
            try:
                self.scanner.scan(lambda r: self._publish(r) if gen == self.gen else None, self._progress)
                self.status = "Terminé"
            except Exception as e:  # noqa
                self.status = f"Erreur pendant l'analyse : {e}"
                print(self.status, file=sys.stderr)
            finally:
                self.scanning = False
            if gen != self.gen:        # réglages changés pendant l'analyse : on recommence avec les nouveaux
                self.start_scan()

        threading.Thread(target=run, daemon=True).start()

    def graph(self, refresh=False):
        if self.setup:
            return {"setup": True, **self.setup}
        if refresh:
            self.start_scan()
        with self.lock:
            if self.cache is None:
                return {"pending": True, "status": self.status}
            return dict(self.cache, scanning=self.scanning, status=self.status)

    def links_of(self, rel: str):
        """Liens et tags contenus dans une note (pour les afficher en direct pendant que Hermes écrit)."""
        p = self.root / rel
        try:
            txt = p.read_text("utf-8", errors="ignore")[:SCAN_TEXT_LIMIT]
        except OSError:
            return [], []
        sc = self.scanner
        known = sc.index_paths
        out = []
        for m in WIKILINK_RE.finditer(txt):
            t = m.group(1).strip().lower()
            for cand in (sc.name_map.get(t, []) + sc.name_map.get(t + ".md", []))[:3]:
                if cand != rel:
                    out.append(cand)
        parent = posixpath.dirname(rel)
        for m in MDLINK_RE.finditer(txt):
            cand = href_target(parent, m.group(1))
            if cand and cand in known and cand != rel:
                out.append(cand)
        tags = sorted(extract_tags(txt, rel.lower().endswith((".md", ".markdown"))))
        return sorted(set(out))[:60], tags

    def safe(self, rel: str) -> Path:
        if not self.root:
            raise ValueError("aucun dossier configuré")
        p = (self.root / rel).resolve()
        p.relative_to(self.root)  # lève ValueError si on sort du dossier
        if not self.scanner.allowed(p):
            raise ValueError("hors des dossiers autorisés")
        return p


def make_handler(app: App):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def end_headers(self):
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Referrer-Policy", "no-referrer")
            super().end_headers()

        def host_ok(self):
            """Protection contre le « DNS rebinding » : seules les adresses locales de ce serveur sont acceptées
            (un site qui ferait pointer son nom vers 127.0.0.1 enverrait son propre nom dans « Host »)."""
            port = self.server.server_address[1]
            allowed = {f"127.0.0.1:{port}", f"localhost:{port}", f"[::1]:{port}"}
            if (self.headers.get("Host") or "").lower() not in allowed:
                return False
            origin = self.headers.get("Origin")
            if origin and origin.lower() not in {"http://" + h for h in allowed}:
                return False
            return True

        def send_json(self, obj, code=200):
            data = json.dumps(i18n.tr_obj(obj), ensure_ascii=False).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(data)

        def send_file(self, p: Path, ctype=None):
            ctype = ctype or mimetypes.guess_type(p.name)[0] or "application/octet-stream"
            size = p.stat().st_size
            rng = self.headers.get("Range")
            start, end = 0, size - 1
            if rng and rng.startswith("bytes="):
                a, _, b = rng[6:].partition("-")
                start = int(a) if a else 0
                end = int(b) if b else size - 1
                self.send_response(206)
                self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
            else:
                self.send_response(200)
            self.send_header("Content-Type", ctype)
            self.send_header("X-Frame-Options", "SAMEORIGIN")
            if p.suffix.lower() in ACTIVE_EXT:
                # HTML / SVG des notes : affichés sans pouvoir exécuter de script ni piloter l'app
                self.send_header("Content-Security-Policy", "sandbox")
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("Content-Length", str(end - start + 1))
            self.end_headers()
            with open(p, "rb") as f:
                f.seek(start)
                remaining = end - start + 1
                while remaining > 0:
                    chunk = f.read(min(1 << 16, remaining))
                    if not chunk:
                        break
                    try:
                        self.wfile.write(chunk)
                    except (BrokenPipeError, ConnectionResetError):
                        return
                    remaining -= len(chunk)

        def do_GET(self):
            if not self.host_ok():
                return self.send_json({"error": "adresse refusée"}, 403)
            u = urlparse(self.path)
            q = parse_qs(u.query)
            path = q.get("path", [""])[0]
            try:
                if u.path in ("/", "/index.html"):
                    lang = (app.cfg.data.get("language") or "").lower()
                    html = ((STATIC / "index.html").read_text("utf-8").replace("__HM_TOKEN__", TOKEN)
                            .replace("__HM_LANG__", lang if lang in ("fr", "en") else "")
                            .replace("__HM_I18N__", json.dumps({k: i18n.DATA.get(k, {}) for k in ("exact", "patterns")},
                                                               ensure_ascii=False).replace("</", "<\\/")))
                    data = html.encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "text/html; charset=utf-8")
                    self.send_header("Content-Length", str(len(data)))
                    self.send_header("Cache-Control", "no-store")
                    self.send_header("Content-Security-Policy", INDEX_CSP)
                    self.send_header("X-Frame-Options", "DENY")
                    self.end_headers()
                    self.wfile.write(data)
                    return
                if u.path == "/api/hermes/info":
                    if not app.runner:
                        return self.send_json({"available": False, "disabled": True})
                    info = app.runner.info()
                    if path or q.get("path"):
                        info["target"] = self.hermes_target(path)
                    return self.send_json(info)
                if u.path == "/api/hermes/status":
                    job = app.runner.get(q.get("job", [""])[0])
                    return self.send_json(job.public(int(q.get("since", ["0"])[0]), int(q.get("esince", ["0"])[0])))
                if u.path == "/api/hermes/diff":
                    job = app.runner.get(q.get("job", [""])[0])
                    return self.send_json(job.diff(path))
                if u.path.startswith("/static/"):
                    p = (STATIC / u.path[len("/static/"):]).resolve()
                    p.relative_to(STATIC)
                    return self.send_file(p)
                if u.path == "/api/ping":
                    app.last_ping = time.time()
                    return self.send_json({"app": "hermes-map", "version": VERSION, "pid": os.getpid(),
                                           "started": STARTED})
                if u.path == "/api/config":
                    d = app.cfg.to_public()
                    d["setup"] = app.setup
                    d["hermes_found"] = bool(app.runner and app.runner.hermes)
                    d["version"] = VERSION
                    d["platform"] = sys.platform
                    return self.send_json(d)
                if u.path == "/api/setup/detect":
                    d = settings.detect(lambda: find_hermes(app.opts.get("hermes")))
                    d["current"] = app.cfg.to_public()
                    return self.send_json(d)
                if u.path == "/api/setup/inspect":
                    return self.send_json(settings.inspect(path))
                if u.path == "/api/graph":
                    return self.send_json(app.graph(refresh=q.get("refresh", ["0"])[0] == "1"))
                if u.path == "/api/node":
                    return self.send_json(self.node_info(path))
                if u.path == "/api/raw":
                    p = app.safe(path)
                    if kind_of(p) == "secret" and not app.reveal:
                        return self.send_json({"error": "fichier secret masqué"}, 403)
                    return self.send_file(p)
                if u.path == "/api/thumb":
                    p = app.safe(path)
                    k = kind_of(p)
                    t = make_thumb(p, k) if k in ("image", "video", "pdf", "office", "text", "markdown", "json", "config") else None
                    if not t:
                        return self.send_json({"error": "pas de miniature"}, 404)
                    return self.send_file(t, "image/png" if t.read_bytes()[:4] == b"\x89PNG" else "image/jpeg")
                if u.path == "/api/search":
                    return self.send_json(self.search(q.get("q", [""])[0], path,
                                                      q.get("content", ["0"])[0] == "1",
                                                      min(5000, int(q.get("limit", ["1000"])[0]))))
                if u.path == "/api/video":
                    p = app.safe(path)
                    st_ = app.video.status(p)
                    st_.pop("file", None)
                    st_["ffmpeg"] = bool(app.video.ffmpeg)
                    return self.send_json(st_)
                if u.path == "/api/video/file":
                    p = app.safe(path)
                    f = p if p.suffix.lower() in media_tools.WEB_VIDEO_EXT else app.video.cached_file(p)
                    if not f:
                        return self.send_json({"error": "vidéo pas encore convertie"}, 404)
                    return self.send_file(f, "video/mp4" if f != p else None)
                if u.path == "/api/doc":
                    p = app.safe(path)
                    html = media_tools.doc_html(p)
                    return self.send_json({"html": html} if html is not None else {"unsupported": True})
                if u.path == "/api/dirs":
                    return self.send_json({"dirs": mover.suggest_dirs(app.scanner, q.get("q", [""])[0])})
                if u.path == "/api/db":
                    return self.send_json(self.db_table(path, q.get("table", [""])[0],
                                                        int(q.get("offset", ["0"])[0])))
                self.send_json({"error": "introuvable"}, 404)
            except ValueError:
                self.send_json({"error": "chemin hors du dossier"}, 403)
            except FileNotFoundError:
                self.send_json({"error": "fichier introuvable"}, 404)
            except Exception as e:  # noqa
                self.send_json({"error": str(e)}, 500)

        def search(self, query, scope, content, limit):
            """Recherche dans TOUT l'arbre (pas seulement ce qui est affiché sur la carte).
            Un élément correspond si tous les mots sont dans son chemin : chercher « jardin »
            trouve le dossier « Jardin… » ET tout ce qu'il contient."""
            idx = app.scanner.index
            if idx is None:
                return {"pending": True, "status": app.status}
            words = [w for w in norm_text(query).split() if w]
            if not words:
                return {"results": [], "total": 0}
            scope = scope.strip("/")
            if scope.startswith("#tag:"):
                scope = ""
            prefix = scope + "/" if scope else ""
            plen = len(prefix)
            res = []
            total = 0
            for rel, name, is_dir, size, mtime, cloud in idx:
                if prefix and not rel.startswith(prefix):
                    continue
                n_name = norm_text(name)
                n_rel = norm_text(rel[plen:])
                if all(w in n_name for w in words):
                    match, rank = "nom", 0
                elif all(w in n_rel for w in words):
                    match, rank = "chemin", 1     # trouvé via un dossier parent
                else:
                    continue
                total += 1
                res.append((rank, not is_dir, rel.count("/"), rel, name, is_dir, size, mtime, cloud, match))
            res.sort(key=lambda r: r[:4])
            out = [{"path": r[3], "name": r[4], "kind": "dir" if r[5] else kind_of(Path(r[3])),
                    "size": r[6], "mtime": r[7], "cloud": r[8], "match": r[9],
                    "where": posixpath.dirname(r[3])} for r in res[:limit]]
            # recherche dans le contenu des fichiers texte (option, limitée en temps)
            content_hits, content_done = 0, True
            if content:
                seen = {o["path"] for o in out}
                t0 = time.time()
                for rel, name, is_dir, size, mtime, cloud in idx:
                    if is_dir or cloud or size > 2_000_000 or rel in seen:
                        continue
                    if prefix and not rel.startswith(prefix):
                        continue
                    k = kind_of(Path(name))
                    if k not in ("markdown", "text", "config", "json"):
                        continue
                    if time.time() - t0 > 8 or len(out) >= limit:
                        content_done = False
                        break
                    try:
                        with open(app.root / rel, "rb") as fh:
                            raw = fh.read(2_000_000)
                    except OSError:
                        continue
                    if b"\x00" in raw[:2048]:
                        continue
                    txt = raw.decode("utf-8", errors="ignore")
                    ntxt = norm_text(txt)
                    if all(w in ntxt for w in words):
                        i = ntxt.find(words[0])
                        snippet = txt[max(0, i - 60): i + 100].replace("\n", " ")
                        out.append({"path": rel, "name": name, "kind": k, "size": size, "mtime": mtime,
                                    "cloud": False, "match": "contenu", "snippet": snippet,
                                    "where": posixpath.dirname(rel)})
                        content_hits += 1
                        total += 1
            return {"results": out, "total": total, "truncated": total > len(out),
                    "content_hits": content_hits, "content_done": content_done}

        def hermes_target(self, rel):
            """Cible de la mise à jour : le dossier ouvert, ou le fichier .md ouvert (sinon son dossier)."""
            rel = (rel or "").strip("/")
            if not rel or rel.startswith("#tag:"):
                return {"ok": False, "why": "Ouvre d'abord un dossier (ou un sous-dossier, ou une note .md) : "
                                            "la mise à jour part de l'endroit où tu es."}
            p = app.safe(rel)
            if p.is_file() and p.suffix.lower() not in (".md", ".markdown"):
                p = p.parent
            if p == app.root:
                return {"ok": False, "why": "Ouvre un dossier ou un de ses sous-dossiers."}
            kind = "dir" if p.is_dir() else "file"
            n_md = 0
            if kind == "dir":
                for _, dirs, files in os.walk(p):
                    dirs[:] = [d for d in dirs if not d.startswith(".")]
                    n_md += sum(1 for f in files if f.lower().endswith((".md", ".markdown")))
                    if n_md > 20000:
                        break
            return {"ok": True, "path": app.scanner.rel(p), "abs": str(p), "name": p.name, "kind": kind,
                    "parent": app.scanner.rel(p.parent), "md_count": n_md if kind == "dir" else 1}

        def do_POST(self):
            if not self.host_ok():
                return self.send_json({"error": "adresse refusée"}, 403)
            u = urlparse(self.path)
            try:
                if self.headers.get("X-HM-Token") != TOKEN:
                    return self.send_json({"error": "jeton invalide : recharge la page"}, 403)
                if u.path == "/api/quit":
                    busy = bool(app.runner and app.runner.job and app.runner.job.state in ("preparing", "running"))
                    if busy:
                        return self.send_json({"error": "Hermes travaille encore : arrête-le d'abord ou attends la fin."}, 409)
                    self.send_json({"ok": True})
                    threading.Timer(0.3, lambda: os._exit(0)).start()
                    return
                if u.path in ("/api/move", "/api/move/plan", "/api/move/undo"):
                    n = int(self.headers.get("Content-Length") or 0)
                    body = json.loads(self.rfile.read(n) or b"{}") if n else {}
                    mv = mover.Mover(app)
                    if u.path == "/api/move/plan":
                        return self.send_json({"plan": mv.plan(body.get("items", []), body.get("create_dirs", True))})
                    if u.path == "/api/move/undo":
                        return self.send_json(mv.undo(body.get("id", "")))
                    return self.send_json(mv.run(body.get("items", []), body.get("create_dirs", True),
                                                 body.get("update_index", True), body.get("note", "")))
                if u.path == "/api/lang":
                    n = int(self.headers.get("Content-Length") or 0)
                    body = json.loads(self.rfile.read(n) or b"{}") if n else {}
                    lg = body.get("lang")
                    if lg not in ("fr", "en"):
                        return self.send_json({"error": "langue inconnue"}, 400)
                    app.cfg.data["language"] = lg
                    settings.save_language(lg)
                    return self.send_json({"ok": True, "lang": lg})
                if u.path in ("/api/setup/save", "/api/setup/choose", "/api/os/privacy"):
                    n = int(self.headers.get("Content-Length") or 0)
                    body = json.loads(self.rfile.read(n) or b"{}") if n else {}
                    if u.path == "/api/setup/choose":
                        return self.send_json(media_tools.choose_folder(body.get("prompt") or "Choisis le dossier à explorer"))
                    if u.path == "/api/os/privacy":
                        media_tools.open_privacy_settings(os.environ.get("HM_APP"))
                        return self.send_json({"ok": True})
                    return self.send_json(self.setup_save(body))
                if u.path == "/api/open":
                    n = int(self.headers.get("Content-Length") or 0)
                    body = json.loads(self.rfile.read(n) or b"{}") if n else {}
                    media_tools.open_in_app(app.safe(body.get("path", "")), bool(body.get("reveal")))
                    return self.send_json({"ok": True})
                if not app.runner:
                    return self.send_json({"error": "bouton Hermes désactivé (--no-hermes)"}, 403)
                n = int(self.headers.get("Content-Length") or 0)
                body = json.loads(self.rfile.read(n) or b"{}") if n else {}
                if u.path == "/api/hermes/run" and body.get("mode") == "reclass":
                    files = []
                    for f in body.get("files", []):
                        fp = app.safe(f)
                        if fp.is_file():
                            files.append(app.scanner.rel(fp))
                    if not files:
                        return self.send_json({"error": "aucun fichier à reclasser"}, 400)
                    prompt = (body.get("prompt") or "").strip()
                    if not prompt:
                        return self.send_json({"error": "consigne vide"}, 400)
                    return self.send_json(app.runner.start(app.root, prompt, mode="reclass", files=files,
                                                           profile=(body.get("profile") or "").strip() or None))
                if u.path == "/api/hermes/run":
                    t = self.hermes_target(body.get("path", ""))
                    if not t["ok"]:
                        return self.send_json({"error": t["why"]}, 400)
                    prompt = (body.get("prompt") or "").strip()
                    if not prompt:
                        return self.send_json({"error": "consigne vide"}, 400)
                    return self.send_json(app.runner.start(Path(t["abs"]), prompt,
                                                           profile=(body.get("profile") or "").strip() or None))
                job = app.runner.get(body.get("job", ""))
                if u.path == "/api/hermes/stop":
                    job.stop()
                    return self.send_json({"ok": True})
                if u.path == "/api/hermes/undo":
                    if job.state in ("preparing", "running"):
                        return self.send_json({"error": "attends la fin (ou arrête) avant d'annuler"}, 400)
                    return self.send_json(job.undo())
                self.send_json({"error": "introuvable"}, 404)
            except KeyError as e:
                self.send_json({"error": str(e)}, 404)
            except ValueError:
                self.send_json({"error": "chemin hors du dossier"}, 403)
            except RuntimeError as e:
                self.send_json({"error": str(e)}, 409)
            except Exception as e:  # noqa
                self.send_json({"error": str(e)}, 500)

        def setup_save(self, body):
            """Enregistre les réglages de l'écran d'accueil (ou de la fenêtre Réglages) et relance l'analyse."""
            data = settings._merge(app.cfg.data, {})
            for k in ("root", "include", "folders", "hidden_tops", "title", "zone_prefix", "ignore"):
                if k in body:
                    data[k] = body[k]
            for k in ("archive", "hermes"):
                if isinstance(body.get(k), dict):
                    data[k] = settings._merge(data.get(k) or {}, body[k])
            root = data.get("root")
            if not root:
                return {"error": "choisis un dossier"}
            info = settings.inspect(root)
            if info["access"] == "missing":
                return {"error": "dossier introuvable : " + root}
            if info["access"] == "denied":
                return {"error": "denied", "path": root}
            cfg = settings.Settings(data)
            made = []
            if cfg.archive:
                try:
                    made = settings.init_archive(cfg)
                except OSError as e:
                    return {"error": f"impossible de créer le dossier d'archivage : {e}"}
            why = app.configure(cfg)
            if why == "busy":
                return {"error": "Hermes travaille encore : attends la fin avant de changer les réglages."}
            if why:
                return {"error": why, "path": root}
            cfg.save()
            return {"ok": True, "made": made, "config": cfg.to_public()}

        def node_info(self, rel):
            if rel.startswith("#tag:"):
                t = rel[5:]
                children = []
                for f in app.scanner.tag_index.get(t, []):
                    try:
                        fp = app.safe(f)
                        es = fp.stat()
                    except (ValueError, OSError):
                        continue
                    children.append({"name": fp.name, "path": f, "kind": kind_of(fp), "size": es.st_size,
                                     "mtime": es.st_mtime, "where": posixpath.dirname(f),
                                     "cloud": bool(getattr(es, "st_flags", 0) & SF_DATALESS)})
                return {"path": rel, "name": "#" + t, "kind": "tag", "size": 0, "mtime": 0,
                        "children": children, "children_total": len(children)}
            p = app.safe(rel) if rel not in ("", "/") else app.root
            k = "dir" if p.is_dir() else kind_of(p)
            st = p.stat()
            info = {"path": "" if p == app.root else app.scanner.rel(p), "name": p.name, "kind": k,
                    "size": st.st_size, "mtime": st.st_mtime,
                    "cloud": bool(getattr(st, "st_flags", 0) & SF_DATALESS) and k != "dir"}
            if k == "icloud":
                info["name"] = icloud_real_name(p.name)
                info["note"] = ("Ce fichier est dans iCloud mais pas encore téléchargé sur ce Mac. "
                                "Dans le Finder : clic droit → « Télécharger maintenant », puis ↻.")
            elif k == "dir":
                children = []
                depth = 1 if p == app.root else 2
                for e in sorted(p.iterdir(), key=lambda e: (not e.is_dir(), e.name.lower())):
                    if app.scanner.skip(e, depth):
                        continue
                    try:
                        es = e.stat()
                    except OSError:
                        continue
                    children.append({"name": icloud_real_name(e.name) or e.name, "path": app.scanner.rel(e),
                                     "kind": "dir" if e.is_dir() else kind_of(e),
                                     "size": es.st_size, "mtime": es.st_mtime,
                                     "cloud": bool(getattr(es, "st_flags", 0) & SF_DATALESS) and not e.is_dir()})
                cat = app.scanner.catalog()
                for c in children:
                    d = cat.get(c["path"])
                    if d:
                        c["desc"] = d
                info["children"] = children[:2000]
                info["children_total"] = len(children)
                here = app.scanner.rel(p) if p != app.root else ""
                mirror = app.scanner.mirror_of(here)
                if mirror and app.scanner.index:
                    pre = mirror + "/"
                    items = []
                    for rel2, name2, is_dir2, size2, mtime2, cloud2 in app.scanner.index:
                        if rel2.startswith(pre):
                            it = {"name": name2, "path": rel2, "kind": "dir" if is_dir2 else kind_of(Path(name2)),
                                  "size": size2, "mtime": mtime2, "cloud": cloud2, "archive": True,
                                  "where": posixpath.dirname(rel2)}
                            d = cat.get(rel2)
                            if d:
                                it["desc"] = d
                            items.append(it)
                    items.sort(key=lambda x: (x["kind"] != "dir", x["path"].lower()))
                    info["mirror"] = {"path": mirror, "total": len(items), "items": items[:5000],
                                      "described": sum(1 for x in items if "desc" in x)}
                brain = app.scanner.brain_of(here)
                if brain:
                    info["brain"] = brain
                for readme in ("SKILL.md", "README.md", "index.md", "SOUL.md"):
                    if (p / readme).is_file():
                        info["readme"] = {"name": readme,
                                          "text": (p / readme).read_text("utf-8", errors="replace")[:MAX_TEXT]}
                        break
            elif k == "secret":
                txt = p.read_text("utf-8", errors="replace")[:MAX_TEXT]
                if not app.reveal:
                    if p.suffix.lower() == ".json":
                        try:
                            txt = json.dumps(mask_json(json.loads(txt)), indent=2, ensure_ascii=False)
                        except json.JSONDecodeError:
                            txt = mask_secret_text(txt)
                    else:
                        txt = mask_secret_text(txt)
                    info["masked"] = True
                info["text"] = txt
            elif k == "db":
                info["tables"] = self.db_tables(p)
            elif k in ("markdown", "json", "config", "text") or is_texty(p):
                if is_texty(p) or k in ("markdown", "json", "config"):
                    with open(p, "rb") as f:
                        raw = f.read(MAX_TEXT)
                    info["text"] = raw.decode("utf-8", errors="replace")
                    info["truncated"] = st.st_size > MAX_TEXT
                else:
                    info["kind"] = "binary"
            return info

        def db_tables(self, p: Path):
            con = sqlite3.connect(f"file:{p}?mode=ro", uri=True, timeout=2)
            try:
                rows = con.execute("SELECT name FROM sqlite_master WHERE type IN ('table','view') "
                                   "AND name NOT LIKE 'sqlite_%' ORDER BY name").fetchall()
                out = []
                for (name,) in rows:
                    try:
                        n = con.execute(f'SELECT COUNT(*) FROM "{name}"').fetchone()[0]
                    except sqlite3.Error:
                        n = None
                    out.append({"name": name, "rows": n})
                return out
            finally:
                con.close()

        def db_table(self, rel, table, offset):
            p = app.safe(rel)
            con = sqlite3.connect(f"file:{p}?mode=ro", uri=True, timeout=2)
            try:
                names = {r[0] for r in con.execute("SELECT name FROM sqlite_master")}
                if table not in names:
                    return {"error": "table inconnue"}
                cur = con.execute(f'SELECT * FROM "{table}" LIMIT 100 OFFSET ?', (max(0, offset),))
                cols = [d[0] for d in cur.description]
                rows = []
                for r in cur.fetchall():
                    row = []
                    for v in r:
                        if isinstance(v, bytes):
                            v = f"<{len(v)} octets>"
                        elif isinstance(v, str) and len(v) > 4000:
                            v = v[:4000] + "…"
                        row.append(v)
                    rows.append(row)
                return {"columns": cols, "rows": rows, "offset": offset}
            finally:
                con.close()

    return H


def main():
    ap = argparse.ArgumentParser(description="Hermes Map — explorateur visuel")
    ap.add_argument("root", nargs="?", default=None,
                    help="dossier à explorer (défaut : celui choisi dans l'écran d'accueil)")
    ap.add_argument("--only", default=None,
                    help='dossiers de 1er niveau à afficher, motifs séparés par des virgules ("*" = tout)')
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--no-browser", action="store_true")
    ap.add_argument("--port-file", default=None, help="écrit le port utilisé dans ce fichier (lanceur de l'app)")
    ap.add_argument("--idle-exit", type=int, default=0,
                    help="s'arrête tout seul N secondes après la fermeture de la dernière page (0 = jamais)")
    ap.add_argument("--all", action="store_true", help="n'ignorer aucun dossier (code source inclus)")
    ap.add_argument("--hidden", action="store_true", help="afficher aussi les fichiers cachés")
    ap.add_argument("--reveal-secrets", action="store_true", help="afficher les clés API en clair")
    ap.add_argument("--hermes", default=None, help="chemin de la commande hermes (défaut : celle du PATH)")
    ap.add_argument("--hermes-args", default="", help='options ajoutées à « hermes chat », ex. "--toolsets file"')
    ap.add_argument("--no-hermes", action="store_true", help="désactiver les fonctions Hermes")
    args = ap.parse_args()

    cfg = settings.load() or settings.Settings()
    if args.root:                       # dossier imposé en ligne de commande (non enregistré)
        cfg = settings.Settings(settings._merge(cfg.data, {"root": args.root}))
    if args.only is not None:
        cfg.data["include"] = args.only
    settings.set_current(cfg)
    app = App(args.reveal_secrets, {"all": args.all, "hidden": args.hidden, "hermes": args.hermes,
                                    "hermes_args": args.hermes_args, "no_hermes": args.no_hermes})
    why = app.configure(cfg)
    if why == "first":
        print("Premier lancement : choisis le dossier à explorer dans la page qui s'ouvre.")
    elif why == "denied":
        print(f"Accès refusé par macOS à {cfg.root}\n"
              "Réglages Système > Confidentialité et sécurité > Accès complet au disque : active Hermes Map.",
              file=sys.stderr)
    elif why == "missing":
        print(f"Dossier introuvable : {cfg.root} (à choisir à nouveau dans la page)", file=sys.stderr)
    print("Vidéos AVI/MKV/WMV… : " + ("conversion avec " + app.video.ffmpeg if app.video.ffmpeg
          else "ffmpeg absent (pour les lire ici : brew install ffmpeg) ; elles s'ouvriront dans l'app du Mac"))

    port = args.port
    for _ in range(20):
        try:
            srv = ThreadingHTTPServer(("127.0.0.1", port), make_handler(app))
            break
        except OSError:
            port += 1
    else:
        print("Aucun port libre trouvé.", file=sys.stderr)
        sys.exit(1)

    url = f"http://127.0.0.1:{port}/"
    if args.port_file:
        try:
            Path(args.port_file).parent.mkdir(parents=True, exist_ok=True)
            Path(args.port_file).write_text(str(port))
        except OSError:
            pass
    if args.idle_exit > 0:
        # mode app : quand plus aucune page n'est ouverte (et qu'Hermes ne travaille pas), on s'arrête
        started = time.time()

        def watchdog():
            while True:
                time.sleep(15)
                busy = bool(app.runner and app.runner.job and app.runner.job.state in ("preparing", "running"))
                last = app.last_ping or started
                if not busy and time.time() - last > max(args.idle_exit, 60 if app.last_ping else 300):
                    print("Plus aucune fenêtre ouverte : arrêt de Hermes Map.")
                    if args.port_file:
                        try:
                            Path(args.port_file).unlink()
                        except OSError:
                            pass
                    os._exit(0)
        threading.Thread(target=watchdog, daemon=True).start()
    print(f"Hermes Map ouvert sur {url}  (Ctrl+C pour arrêter)")
    if not args.no_browser:
        threading.Timer(0.6, lambda: webbrowser.open(url)).start()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        print("\nArrêt.")


if __name__ == "__main__":
    main()
