"""Lance Hermes (« hermes chat --query-file - ») sur un dossier ou un fichier .md,
avec sauvegarde préalable des .md, suivi en direct, liste des changements et annulation.

Seuls les fichiers .md sont sauvegardés et restaurables. Les autres fichiers modifiés
pendant le passage sont seulement signalés.
"""
import difflib
import hashlib
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

import settings

BACKUP_ROOT = (Path(os.path.expanduser("~/Library/Application Support/hermes-map/backups"))
               if sys.platform == "darwin" else Path(os.path.expanduser("~/.local/share/hermes-map/backups")))
MAX_BACKUP_FILES = 5000
MAX_BACKUP_BYTES = 300 * 1024 * 1024
MAX_MD_SIZE = 5 * 1024 * 1024
MAX_RUNTIME = 45 * 60          # sécurité : arrêt automatique après 45 min
ANSI_RE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07|\r")
SKIP_DIRS = {".git", "node_modules", "__pycache__", ".venv", "venv", ".obsidian", ".trash"}

DEFAULT_PROMPT_DIR = """Tu travailles dans le dossier « {nom} » de ma base de connaissances Hermes :
{chemin}

Mets à jour les fichiers Markdown (.md) de ce dossier ET de tous ses sous-dossiers :
1. Relie les notes qui parlent du même sujet, de la même personne, du même lieu ou du même projet.
2. Dans chaque note, ajoute ou complète un en-tête YAML (entre deux lignes ---) avec un champ `tags:` : 3 à 6 tags courts, en minuscules, sans accents, réutilise en priorité les tags déjà présents dans le dossier.
3. Crée ou mets à jour un fichier INDEX.md dans chaque dossier qui contient des notes : un court résumé du contenu du dossier, puis la liste de ses fichiers et sous-dossiers avec un lien vers chacun.

Format des liens (impératif) :
- Utilise UNIQUEMENT des liens Markdown classiques : [Titre lisible](chemin/relatif/vers/la-note.md). N'écris JAMAIS de liens [[…]].
- Le chemin est relatif à la note qui contient le lien (ex. ../lieux/paris.md). Si le chemin contient des espaces, entoure-le de < > : [Titre](<../mon dossier/note.md>).
- Le texte du lien est le titre de la note visée (sa ligne « # Titre »), pas le nom du fichier.
- Avant d'ajouter un lien, vérifie qu'il n'existe pas déjà dans la note (même cible, quelle que soit la forme) : aucun doublon.
- Ne modifie pas les liens existants. Respecte les conventions décrites dans les INDEX.md / README.md du dossier.

Règles strictes :
- Ne travaille QUE dans ce dossier et ses sous-dossiers, n'en sors jamais.
- Ne modifie que des fichiers .md ; ne touche à aucun autre type de fichier.
- Ne supprime, ne renomme et ne déplace aucun fichier ni dossier.
- Conserve tout le texte existant : ajoute des liens et des tags, ne réécris pas et ne résume pas le contenu des notes.
- N'exécute aucune commande qui modifie le système ; n'utilise pas Internet.
À la fin, donne la liste des fichiers que tu as créés ou modifiés."""

DEFAULT_PROMPT_FILE = """Tu travailles sur la note « {nom} » de ma base de connaissances Hermes :
{chemin}

Mets à jour CE fichier uniquement :
1. Ajoute des liens vers les autres notes .md du même dossier ({dossier}) et de ses sous-dossiers qui parlent du même sujet.
2. Ajoute ou complète un en-tête YAML (entre deux lignes ---) avec un champ `tags:` : 3 à 6 tags courts, en minuscules, sans accents.

Format des liens (impératif) :
- UNIQUEMENT des liens Markdown classiques [Titre lisible](chemin/relatif.md), jamais de [[…]] ; chemin relatif à cette note, entre < > s'il contient des espaces.
- Texte du lien = titre de la note visée (sa ligne « # Titre »).
- Aucun doublon : ne rajoute pas un lien vers une note déjà liée dans ce fichier, quelle que soit la forme. Ne modifie pas les liens existants.

Règles strictes :
- Ne modifie QUE ce fichier ; tu peux lire les autres notes du dossier mais pas les modifier.
- Conserve tout le texte existant : ajoute des liens et des tags, ne réécris pas le contenu.
- Ne supprime, ne renomme et ne déplace aucun fichier ; n'utilise pas Internet.
À la fin, résume ce que tu as changé."""


RECLASS_HEAD = """Reclassement de fichiers mal rangés dans ma base de connaissances (racine : {racine}).

Fichiers à traiter ({nb}) :
{fichiers}

Pour CHAQUE fichier :
1. Analyse-le (regarde l'image / lis le document) : de quoi s'agit-il ? À quel dossier appartient-il vraiment ?"""

RECLASS_TAIL = """3. Si tu n'es pas sûr de la destination ou du sujet, NE DÉPLACE PAS ce fichier : explique pourquoi et propose une destination.

Règles strictes :
- Ne déplace et ne renomme QUE les fichiers listés ci-dessus. Ne supprime rien.
- Ne modifie que des fichiers texte d'index (.md, .yaml, .jsonl) en plus de ces déplacements.
- N'utilise pas Internet.
À la fin, donne un tableau : ancien chemin → nouveau chemin (ou « laissé en place » + raison)."""


def reclass_prompt(cfg, zones):
    """Consigne de reclassement, adaptée aux réglages (avec ou sans dossier d'archivage)."""
    z = ", ".join(zones[:30]) or "(dossiers du 1er niveau)"
    lines = [RECLASS_HEAD.replace("À quel dossier appartient-il vraiment ?",
                                  f"À quel dossier appartient-il vraiment ? Dossiers principaux : {z}."),
             "2. Range-le :",
             "   - nouveau nom lisible, sans emoji ni suite de caractères aléatoires, extension correcte ;"]
    arch = cfg.archive
    if arch:
        lines += [f"   - les notes (.md) restent dans les dossiers principaux ; les autres fichiers (images, vidéos, PDF, documents…) "
                  f"sont rangés dans le dossier d'archivage, au chemin « miroir » de la note ou du dossier concerné : "
                  f"déplacement (mv) vers {arch}/<dossier principal sans préfixe, en minuscules>/<même chemin>/ "
                  f"(ex. un fichier lié à {(zones or ['Projets'])[0]}/voyages/japon.md va dans "
                  f"{arch}/{cfg.zone_name((zones or ['Projets'])[0]).lower()}/voyages/) ;",
                  "   - mets à jour les index : retire la fiche ou le lien de l'ancien emplacement (catalog.yaml, INDEX.md, note) "
                  "et ajoute-le au bon endroit de la destination ;",
                  f"   - mets à jour {cfg.archive_catalog} (une fiche JSON par ligne, « rel » relatif à {arch}) "
                  f"et ajoute une ligne au tableau de {cfg.archive_log} (même format que les lignes existantes) ;"]
    else:
        lines += ["   - déplacement (mv) vers le dossier le plus adapté, de préférence un dossier qui existe déjà ;",
                  "   - mets à jour les index : retire la fiche ou le lien de l'ancien emplacement (catalog.yaml, INDEX.md, note) "
                  "et ajoute-le au bon endroit de la destination ;"]
    lines.append("   - liens Markdown classiques et relatifs : [Titre](chemin/relatif) ; corrige les liens des notes "
                 "qui pointaient vers l'ancien emplacement.")
    conv = [c for c in (cfg.hermes.get("conventions") or []) if c]
    if conv:
        lines.append("   - applique aussi les règles de rangement décrites dans : " + ", ".join(conv) + ".")
    return "\n".join(lines) + "\n" + RECLASS_TAIL



def find_hermes(explicit=None):
    if explicit:
        return explicit
    found = shutil.which("hermes")
    if found:
        return found
    for cand in ("~/.local/bin/hermes", "/opt/homebrew/bin/hermes", "/usr/local/bin/hermes",
                 "~/.hermes/hermes-agent/venv/bin/hermes"):
        p = Path(os.path.expanduser(cand))
        if p.exists():
            return str(p)
    return None


def _sha(data: bytes) -> str:
    return hashlib.sha1(data).hexdigest()


def _walk(target: Path, skip=()):
    """Tous les fichiers sous target (ou target lui-même si c'est un fichier)."""
    if target.is_file():
        yield target
        return
    for dirpath, dirnames, filenames in os.walk(target):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS and d not in skip and not d.startswith(".")]
        for f in filenames:
            if f.startswith(".") and not f.endswith(".icloud"):
                continue
            yield Path(dirpath) / f


class Job:
    def __init__(self, jid, root: Path, target: Path, prompt: str, cmd: list, mode="update", files=None,
                 cwd=None):
        self.id = jid
        self.root = root
        self.target = target
        self.mode = mode
        self.files = files or []      # reclassement : fichiers à analyser / renommer / déplacer
        self.cwd = cwd
        self.fps = {}                 # empreinte de chaque fichier à reclasser
        self.moves = []               # [{old, new}] retrouvés après le passage
        if mode == "reclass":
            # index texte de tous les dossiers suivis (catalogues, journaux, notes) : sauvegardés, suivis, restaurables
            cfg = settings.current
            self.roots = [d for d in sorted(root.iterdir()) if d.is_dir() and cfg.top_allowed(d.name)
                          and not d.name.startswith(".") and d.name not in cfg.ignore]
            self.text_exts = (".md", ".markdown", ".yaml", ".yml", ".jsonl", ".json")
            self.skip = tuple(cfg.hermes.get("backup_skip") or ()) + tuple(cfg.ignore)
        else:
            self.roots = [target]
            self.text_exts = (".md", ".markdown")
            self.skip = ()
        self.prompt = prompt
        self.cmd = cmd
        self.lines = []
        self.state = "preparing"     # preparing → running → done | error | stopped
        self.exit_code = None
        self.started = time.time()
        self.ended = None
        self.proc = None
        self.backup_dir = BACKUP_ROOT / time.strftime("%Y-%m-%d_%H-%M-%S", time.localtime(self.started))
        self.snapshot = {}           # rel -> {sha, size, mtime, backup}
        self.changes = None
        self.undone = False
        self.lock = threading.Lock()
        self.events = []             # activité en direct : {seq, t, kind: read|write|create, path, links, tags}
        self.links_of = None
        self._names = {}             # nom de note (minuscules) -> chemin, pour repérer les lectures dans le journal
        self._last_read = {}

    def rel(self, p: Path) -> str:
        return p.relative_to(self.root).as_posix()

    def _iter(self):
        for r in self.roots:
            yield from _walk(r, self.skip)

    def _is_text(self, p: Path):
        return p.suffix.lower() in self.text_exts

    @staticmethod
    def _fingerprint(p: Path):
        """Empreinte rapide (taille + début + fin) pour retrouver un fichier même renommé / déplacé."""
        try:
            size = p.stat().st_size
            with open(p, "rb") as fh:
                head = fh.read(65536)
                if size > 131072:
                    fh.seek(-65536, 2)
                tail = fh.read(65536)
            return f"{size}:{hashlib.sha1(head + tail).hexdigest()}"
        except OSError:
            return None

    def log(self, line: str):
        with self.lock:
            self.lines.append(line)
            if len(self.lines) > 20000:
                self.lines = self.lines[-15000:]
        if self.state == "running" and not line.startswith("[Hermes Map]"):
            self._spot_reads(line)

    def event(self, kind, rel, **extra):
        with self.lock:
            self.events.append({"seq": len(self.events), "t": time.time(), "kind": kind, "path": rel, **extra})

    # lectures : on repère dans le journal de Hermes les noms de notes du dossier ciblé
    MD_IN_LINE = re.compile(r"[^\s'\"`<>()\[\]|:]+\.(?:md|markdown)\b", re.I)

    def _spot_reads(self, line):
        low = line.lower()
        if ".md" not in low and ".markdown" not in low:
            return
        hits = []
        if len(self._names) <= 5000:
            for base, rel in self._names.items():
                if base in low:
                    hits.append(rel)
        else:
            for m in self.MD_IN_LINE.finditer(low):
                rel = self._names.get(m.group(0).rsplit("/", 1)[-1])
                if rel:
                    hits.append(rel)
        # « notes.md » est aussi contenu dans « mes-notes.md » : on garde le nom le plus long
        hits.sort(key=lambda r: -len(r.rsplit("/", 1)[-1]))
        kept = []
        for r in hits:
            b = r.rsplit("/", 1)[-1].lower()
            if not any(b in k.rsplit("/", 1)[-1].lower() for k in kept):
                kept.append(r)
        for rel in kept[:5]:
            if time.time() - self._last_read.get(rel, 0) > 3:
                self._last_read[rel] = time.time()
                self.event("read", rel)

    # écritures : on surveille les .md du dossier ciblé (date de modification / taille)
    def _watch(self):
        seen = {r: (v["mtime"], v["size"]) for r, v in self.snapshot.items()}
        gone = set()
        while self.state in ("preparing", "running"):
            time.sleep(0.8 if self.mode != "reclass" else 2)
            for rel in self.files:
                if rel not in gone and not (self.root / rel).exists():
                    gone.add(rel)
                    self.event("move", rel)
            try:
                for p in self._iter():
                    if not self._is_text(p):
                        continue
                    try:
                        st = p.stat()
                    except OSError:
                        continue
                    rel = self.rel(p)
                    sig = (st.st_mtime, st.st_size)
                    if seen.get(rel) == sig:
                        continue
                    kind = "write" if rel in seen or rel in self.snapshot else "create"
                    seen[rel] = sig
                    links, tags = (self.links_of(rel) if self.links_of and p.suffix.lower() in (".md", ".markdown")
                                   else ([], []))
                    self.event(kind, rel, links=links, tags=tags)
            except Exception:  # noqa
                pass

    # ---- 1. sauvegarde des .md
    def backup(self):
        n, total = 0, 0
        self.backup_dir.mkdir(parents=True, exist_ok=True)
        for p in self._iter():
            if not self._is_text(p):
                continue
            try:
                st = p.stat()
                if st.st_size > MAX_MD_SIZE:
                    continue
                data = p.read_bytes()          # (télécharge le fichier s'il est resté dans iCloud)
            except OSError:
                continue
            rel = self.rel(p)
            dst = self.backup_dir / "files" / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            dst.write_bytes(data)
            self.snapshot[rel] = {"sha": _sha(data), "size": len(data), "mtime": st.st_mtime}
            n += 1
            total += len(data)
            if n >= MAX_BACKUP_FILES or total >= MAX_BACKUP_BYTES:
                self.log(f"[Hermes Map] ⚠️ sauvegarde limitée à {n} fichiers ({total // 1024} Ko)")
                break
        what = ".md" if self.mode != "reclass" else "d'index (.md, .yaml, .jsonl)"
        self.log(f"[Hermes Map] Sauvegarde de {n} fichier(s) {what} dans {self.backup_dir}")
        for rel in self.files:
            self.fps[rel] = self._fingerprint(self.root / rel)
        if self.files:
            self.log(f"[Hermes Map] {len(self.files)} fichier(s) à reclasser, empreintes notées pour les retrouver après.")
        self.save_manifest()

    def save_manifest(self):
        m = {"id": self.id, "root": str(self.root), "target": self.rel(self.target) if self.target != self.root else "",
             "started": self.started, "ended": self.ended, "state": self.state, "prompt": self.prompt,
             "snapshot": self.snapshot, "changes": self.changes, "undone": self.undone}
        try:
            (self.backup_dir / "manifest.json").write_text(json.dumps(m, ensure_ascii=False, indent=1), "utf-8")
        except OSError:
            pass

    # ---- 2. exécution
    def run(self):
        try:
            self.log(f"[Hermes Map] Cible : {self.target}")
            self.backup()
            self._names = {}
            for r in self.snapshot:
                self._names.setdefault(r.rsplit("/", 1)[-1].lower(), r)
            self.state = "running"
            threading.Thread(target=self._watch, daemon=True).start()
            self.log("[Hermes Map] Commande : " + " ".join(shlex.quote(c) for c in self.cmd))
            self.log("[Hermes Map] Hermes travaille… (cela peut prendre plusieurs minutes)\n")
            env = dict(os.environ, NO_COLOR="1", TERM="dumb", PYTHONUNBUFFERED="1")
            cwd = self.cwd or (self.target if self.target.is_dir() else self.target.parent)
            self.proc = subprocess.Popen(self.cmd, cwd=str(cwd), env=env, stdin=subprocess.PIPE,
                                         stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                         start_new_session=True)
            try:
                self.proc.stdin.write(self.prompt.encode("utf-8"))
                self.proc.stdin.close()
            except OSError:
                pass
            killer = threading.Timer(MAX_RUNTIME, self.stop, kwargs={"reason": "durée maximale atteinte"})
            killer.daemon = True
            killer.start()
            buf = b""
            while True:
                chunk = self.proc.stdout.read1(4096) if hasattr(self.proc.stdout, "read1") else self.proc.stdout.read(1)
                if not chunk:
                    break
                buf += chunk
                while b"\n" in buf:
                    line, buf = buf.split(b"\n", 1)
                    self.log(ANSI_RE.sub("", line.decode("utf-8", errors="replace")))
            if buf:
                self.log(ANSI_RE.sub("", buf.decode("utf-8", errors="replace")))
            self.exit_code = self.proc.wait()
            killer.cancel()
            if self.state != "stopped":
                self.state = "done" if self.exit_code == 0 else "error"
            self.log(f"\n[Hermes Map] Hermes a terminé (code {self.exit_code}).")
        except FileNotFoundError:
            self.state = "error"
            self.log("[Hermes Map] ❌ Commande « hermes » introuvable. Relance Hermes Map avec --hermes /chemin/vers/hermes "
                     "(le chemin que donne « which hermes » dans le Terminal).")
        except Exception as e:  # noqa
            self.state = "error"
            self.log(f"[Hermes Map] ❌ Erreur : {e}")
        finally:
            self.ended = time.time()
            try:
                self.changes = self.detect_changes()
                c = self.changes
                self.log(f"[Hermes Map] Changements : {len(c['modified'])} modifié(s), {len(c['created'])} créé(s), "
                         f"{len(c['deleted'])} supprimé(s), {len(c['other'])} autre(s) fichier(s) touché(s).")
            except Exception as e:  # noqa
                self.log(f"[Hermes Map] Impossible de lister les changements : {e}")
            self.save_manifest()

    def stop(self, reason="arrêt demandé"):
        if self.proc and self.proc.poll() is None:
            self.state = "stopped"
            self.log(f"\n[Hermes Map] ⏹ {reason}, arrêt de Hermes…")
            try:
                os.killpg(self.proc.pid, signal.SIGTERM)
            except (OSError, AttributeError):
                self.proc.terminate()
            threading.Timer(5, lambda: self.proc.poll() is None and self.proc.kill()).start()

    # ---- 3. changements
    def detect_changes(self):
        modified, created, other = [], [], []
        seen = set()
        by_size = {}
        want = {fp.split(":")[0] for fp in self.fps.values() if fp}
        missing = [r for r in self.files if not (self.root / r).exists()]
        for p in self._iter():
            try:
                st = p.stat()
            except OSError:
                continue
            rel = self.rel(p)
            if missing and str(st.st_size) in want:
                by_size.setdefault(str(st.st_size), []).append(p)
            is_md = self._is_text(p)
            if rel in self.snapshot:
                seen.add(rel)
                if st.st_mtime != self.snapshot[rel]["mtime"] or st.st_size != self.snapshot[rel]["size"]:
                    try:
                        if _sha(p.read_bytes()) != self.snapshot[rel]["sha"]:
                            modified.append(rel)
                    except OSError:
                        pass
            elif st.st_mtime >= self.started - 1:
                (created if is_md else other).append(rel)
        deleted = [r for r in self.snapshot if r not in seen and not (self.root / r).exists()]
        # reclassement : où sont passés les fichiers ? (même empreinte, nouveau nom / nouveau dossier)
        self.moves = []
        for rel in self.files:
            if (self.root / rel).exists():
                self.moves.append({"old": rel, "new": rel, "same": True})
                continue
            fp = self.fps.get(rel)
            new = None
            for cand in by_size.get(fp.split(":")[0] if fp else "", []):
                if self._fingerprint(cand) == fp:
                    new = self.rel(cand)
                    break
            self.moves.append({"old": rel, "new": new})
        other = [o for o in other if o not in {m["new"] for m in self.moves}]
        return {"modified": sorted(modified), "created": sorted(created), "deleted": sorted(deleted),
                "other": sorted(other), "moves": self.moves}

    def diff(self, rel):
        p = self.root / rel
        old = ""
        if rel in self.snapshot:
            old = (self.backup_dir / "files" / rel).read_text("utf-8", errors="replace")
        new = p.read_text("utf-8", errors="replace") if p.exists() else ""
        lines = list(difflib.unified_diff(old.splitlines(), new.splitlines(), "avant", "après", n=3, lineterm=""))
        return {"path": rel, "diff": lines[2:] if len(lines) > 2 else lines, "is_new": rel not in self.snapshot}

    # ---- 4. annulation
    def undo(self):
        if not self.changes:
            return {"restored": [], "removed": [], "skipped": []}
        restored, removed, skipped = [], [], []
        end = (self.ended or time.time()) + 3
        for m in self.changes.get("moves", []):
            if m.get("same") or not m.get("new"):
                if not m.get("same"):
                    skipped.append(m["old"] + " (introuvable)")
                continue
            new, old = self.root / m["new"], self.root / m["old"]
            try:
                if new.exists() and not old.exists():
                    old.parent.mkdir(parents=True, exist_ok=True)
                    os.rename(new, old)
                    restored.append(m["old"])
                else:
                    skipped.append(m["new"])
            except OSError:
                skipped.append(m["new"])
        for rel in self.changes["modified"] + self.changes["deleted"]:
            p = self.root / rel
            try:
                if p.exists() and p.stat().st_mtime > end:
                    skipped.append(rel)          # modifié à nouveau depuis : on ne l'écrase pas
                    continue
                p.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(self.backup_dir / "files" / rel, p)
                restored.append(rel)
            except OSError:
                skipped.append(rel)
        for rel in self.changes["created"]:
            p = self.root / rel
            try:
                if p.exists() and p.stat().st_mtime <= end:
                    # mis de côté plutôt que supprimé, au cas où
                    keep = self.backup_dir / "créés-par-hermes" / rel
                    keep.parent.mkdir(parents=True, exist_ok=True)
                    shutil.move(str(p), str(keep))
                    removed.append(rel)
                else:
                    skipped.append(rel)
            except OSError:
                skipped.append(rel)
        self.undone = True
        self.save_manifest()
        return {"restored": restored, "removed": removed, "skipped": skipped}

    def public(self, since=0, esince=0):
        with self.lock:
            lines = self.lines[since:]
            n = len(self.lines)
            events = self.events[esince:]
            en = len(self.events)
        return {"id": self.id, "state": self.state, "exit_code": self.exit_code, "started": self.started,
                "ended": self.ended, "lines": lines, "next": n, "changes": self.changes, "undone": self.undone,
                "target": self.rel(self.target) if self.target != self.root else "",
                "backup": str(self.backup_dir), "events": events, "enext": en, "mode": self.mode}


class Runner:
    def __init__(self, root: Path, hermes_cmd: str = None, extra_args: str = ""):
        self.root = root
        self.hermes = find_hermes(hermes_cmd)
        self.extra = shlex.split(extra_args) if extra_args else []
        self.job = None
        self.n = 0

    def info(self):
        cfg = settings.current
        try:
            zones = sorted(d.name for d in self.root.iterdir() if d.is_dir() and cfg.top_allowed(d.name)
                           and not d.name.startswith(".") and d.name != cfg.archive)
        except OSError:
            zones = []
        h = cfg.hermes
        return {"hermes": self.hermes, "available": bool(self.hermes),
                "running": bool(self.job and self.job.state in ("preparing", "running")),
                "last": self.job.public(len(self.job.lines)) if self.job else None,
                "prompt_dir": DEFAULT_PROMPT_DIR, "prompt_file": DEFAULT_PROMPT_FILE,
                "prompt_reclass": reclass_prompt(cfg, zones), "profiles": self.profiles(),
                "profile": h.get("profile") or "", "reclass_profile": h.get("reclass_profile") or h.get("profile") or "",
                "zone_prefix": cfg.zone_prefix, "archive": cfg.archive}

    def profiles(self):
        base = Path(os.path.expanduser("~/.hermes/profiles"))
        marks = ("config.yaml", ".env", "SOUL.md", "profile.yaml", "auth.json", "state.db")
        try:
            return sorted(d.name for d in base.iterdir() if d.is_dir() and any((d / m).exists() for m in marks))
        except OSError:
            return []

    def start(self, target: Path, prompt: str, mode="update", files=None, profile=None):
        if not self.hermes:
            raise RuntimeError("commande hermes introuvable")
        if self.job and self.job.state in ("preparing", "running"):
            raise RuntimeError("une mise à jour Hermes est déjà en cours")
        self.n += 1
        prof = ["-p", profile] if profile and profile != "default" else []
        cmd = [self.hermes, *prof, "chat", *self.extra, "--query-file", "-"]
        cwd = None
        if mode == "reclass":
            wd = (settings.current.hermes.get("workdir") or "").strip("/")
            cwd = self.root / wd if wd and (self.root / wd).is_dir() else self.root
        self.job = Job(f"j{self.n}-{int(time.time())}", self.root, target, prompt, cmd, mode=mode, files=files,
                       cwd=cwd)
        self.job.links_of = getattr(self, "links_of", None)
        threading.Thread(target=self.job.run, daemon=True).start()
        return self.job.public()

    def get(self, jid):
        if not self.job or self.job.id != jid:
            raise KeyError("tâche inconnue")
        return self.job
