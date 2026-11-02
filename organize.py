"""Boîte à outils de Hermes Map : créer un dossier ou une note, et ranger par glisser-déposer.

Règle cerveau / archive (si un dossier d'archivage est configuré) :
- les notes (.md…) restent dans le cerveau ; les autres fichiers vont dans l'archive, au chemin « miroir »
  (Hobby/inspiration/x.md  <->  Archive/hobby/inspiration/x/…) ;
- une note et les fichiers d'archive qu'elle cite forment un groupe qui se déplace ensemble ;
  la structure sous le miroir est conservée (Archive/hobby/inspiration/x/x.jpg -> Archive/hobby/Coloriage/x/x.jpg).

Fichiers d'organisation tenus à jour, s'ils existent dans le monde (convention « MAPS ») :
GRAPH.json (nœuds / liens « contains »), AREA.md, INDEX.md du dossier.
"""
import json
import os
import posixpath
import re
import shutil
import time
import unicodedata

import settings
from mover import IMGLINK, MDLINK, JOURNAL, _href, _sha, safe_name

NOTE_EXT = (".md", ".markdown")
TEXT_EXT = (".md", ".markdown", ".txt", ".json", ".jsonl", ".yaml", ".yml", ".csv", ".toml", ".ini", ".cfg", ".conf")
INDEX_NAMES = ("INDEX.md", "README.md")
GRAPH_NAME = "GRAPH.json"
AREA_NAME = "AREA.md"


def slugify(s):
    s = unicodedata.normalize("NFKD", s).encode("ascii", "ignore").decode()
    s = re.sub(r"[^A-Za-z0-9]+", "-", s).strip("-").lower()
    return s or "x"


def is_note(rel):
    return rel.lower().endswith(NOTE_EXT)


def is_text(rel):
    return rel.lower().endswith(TEXT_EXT)


def link_targets(text, note_rel, root_abs):
    """Cibles (chemins depuis la racine) des liens Markdown d'une note."""
    from urllib.parse import unquote
    note_dir = posixpath.dirname(note_rel)
    out = []
    for rx in (MDLINK, IMGLINK):
        for m in rx.finditer(text):
            raw = m.group(2)
            h = raw[1:-1] if raw.startswith("<") else unquote(raw)
            h = h.split("#", 1)[0]
            if not h or re.match(r"^[a-z][a-z0-9+.-]*:", h, re.I):
                continue
            if h.startswith("/"):
                if not h.startswith(root_abs + "/"):
                    continue
                t = posixpath.normpath(h[len(root_abs) + 1:])
            else:
                t = posixpath.normpath(posixpath.join(note_dir, h))
            if not t.startswith(".."):
                out.append(t)
    return out


class Organizer:
    def __init__(self, app):
        self.app = app
        self.root = app.root
        self.sc = app.scanner
        self.cfg = settings.current
        self.arch = self.cfg.archive
        self.root_abs = str(self.root)
        self._texts = {}

    # ------------------------------------------------------------ chemins
    def is_archive(self, rel):
        return bool(self.arch) and (rel == self.arch or rel.startswith(self.arch + "/"))

    def mirror_path(self, brain_rel):
        """Chemin miroir dans l'archive d'un chemin du cerveau (même s'il n'existe pas encore)."""
        if not self.arch or not brain_rel or self.is_archive(brain_rel):
            return None
        top, _, rest = brain_rel.partition("/")
        area = self.cfg.zone_name(top)
        base = f"{self.arch}/{area.lower()}"
        if not (self.root / base).is_dir() and (self.root / f"{self.arch}/{area}").is_dir():
            base = f"{self.arch}/{area}"
        return base + ("/" + rest if rest else "")

    def brain_path(self, arch_rel):
        """Inverse : Archive/hobby/x -> Hobby/x (dossier de premier niveau existant)."""
        parts = arch_rel.split("/")
        if not self.arch or len(parts) < 2 or parts[0] != self.arch:
            return None
        area = parts[1].lower()
        for e in sorted(self.root.iterdir()):
            if e.is_dir() and e.name != self.arch and self.sc.top_allowed(e.name) \
                    and self.cfg.zone_name(e.name).lower() == area:
                return "/".join([e.name] + parts[2:])
        return None

    def text(self, rel):
        if rel not in self._texts:
            try:
                self._texts[rel] = (self.root / rel).read_text("utf-8")
            except (OSError, UnicodeDecodeError):
                self._texts[rel] = None
        return self._texts[rel]

    def notes(self):
        return [r for r in sorted(self.sc.index_paths) if is_note(r) and not self.is_archive(r)]

    def links_of(self, note_rel):
        t = self.text(note_rel)
        if not t:
            return []
        seen, out = set(), []
        for x in link_targets(t, note_rel, self.root_abs):
            if x not in seen and (self.root / x).is_file():
                seen.add(x)
                out.append(x)
        return out

    def linkers(self, target):
        """Notes qui citent ce fichier."""
        name = posixpath.basename(target).lower()
        out = []
        for n in self.notes():
            t = self.text(n)
            if not t:
                continue
            low = t.lower()
            if name not in low and name.replace(" ", "%20") not in low:
                continue
            if target in self.links_of(n):
                out.append(n)
        return out

    # ------------------------------------------------------------ plan du glisser-déposer
    def plan(self, srcs, dest):
        dest = (dest or "").strip("/")
        try:
            dp = self.app.safe(dest) if dest else None
        except ValueError as e:
            return {"ok": False, "error": str(e)}
        if dp is None or not dp.is_dir():
            return {"ok": False, "error": "la destination n'est pas un dossier"}
        if self.arch and dest == self.arch:
            return {"ok": False, "error": "choisis un dossier dans l'archive, pas sa racine"}
        in_arch = self.is_archive(dest)
        brain_dest = (self.brain_path(dest) if in_arch else dest)
        arch_dest = (dest if in_arch else self.mirror_path(dest))
        groups, grouped, errors = [], set(), []
        for src in [(s or "").strip("/") for s in srcs]:
            try:
                sp = self.app.safe(src)
            except ValueError as e:
                errors.append({"src": src, "error": str(e)})
                continue
            if sp.is_dir():
                errors.append({"src": src, "error": "déplacer un dossier entier n'est pas encore possible"})
                continue
            if not sp.is_file():
                errors.append({"src": src, "error": "fichier introuvable"})
                continue
            if src in grouped:
                continue
            note = None
            if is_note(src) and not self.is_archive(src):
                note = src
            elif self.is_archive(src):
                owners = [n for n in self.linkers(src) if self._in_note_mirror(src, n)]
                note = owners[0] if owners else None
            if note and note in grouped:
                continue
            items = []
            if note:
                if not brain_dest:
                    errors.append({"src": src, "error": "pas de dossier de notes correspondant à cette destination"})
                    continue
                items.append(self._item(note, f"{brain_dest}/{posixpath.basename(note)}", "note", src))
                grouped.add(note)
                old_mirror = self.mirror_path(posixpath.dirname(note))
                for c in self.links_of(note):
                    if not self.is_archive(c) or c in grouped:
                        continue
                    shared = [n for n in self.linkers(c) if n != note]
                    if old_mirror and c.startswith(old_mirror + "/") and arch_dest:
                        tgt = f"{arch_dest}/{c[len(old_mirror) + 1:]}"
                        reason = None
                    elif arch_dest:
                        tgt = f"{arch_dest}/{posixpath.basename(c)}"
                        reason = "rangé hors du miroir de la note"
                    else:
                        continue
                    it = self._item(c, tgt, "file", src)
                    if shared:
                        it["shared"] = shared
                    if (shared or reason) and c != src:
                        it["checked"] = False
                    if reason:
                        it["reason"] = reason
                    items.append(it)
                    grouped.add(c)
            else:
                binary = not is_text(src)
                if self.arch and binary:
                    if not arch_dest:
                        errors.append({"src": src, "error": "pas de dossier d'archive correspondant"})
                        continue
                    tgt = f"{arch_dest}/{posixpath.basename(src)}"
                else:
                    if not brain_dest and not in_arch:
                        errors.append({"src": src, "error": "destination introuvable"})
                        continue
                    tgt = f"{brain_dest if (brain_dest and not in_arch) or not self.arch else dest}/{posixpath.basename(src)}"
                items.append(self._item(src, tgt, "file", src))
                grouped.add(src)
            groups.append({"note": note, "items": items})
        new_dirs = sorted({posixpath.dirname(i["target"]) for g in groups for i in g["items"]
                           if i.get("ok") and not (self.root / posixpath.dirname(i["target"])).exists()})
        return {"ok": True, "dest": dest, "brain_dest": brain_dest, "arch_dest": arch_dest,
                "groups": groups, "errors": errors, "new_dirs": new_dirs,
                "maps": self._maps_preview(groups)}

    def _in_note_mirror(self, f, note):
        m = self.mirror_path(posixpath.dirname(note))
        return bool(m) and f.startswith(m + "/")

    def _item(self, src, target, kind, dragged):
        it = {"src": src, "target": target, "kind": kind, "checked": True, "ok": True,
              "dragged": src == dragged, "size": (self.root / src).stat().st_size}
        if target == src:
            it.update(ok=False, checked=False, error="déjà à cet endroit")
        elif (self.root / target).exists():
            it.update(ok=False, checked=False, error="un fichier porte déjà ce nom à l'arrivée")
        return it

    def _maps_preview(self, groups):
        out = set()
        for g in groups:
            for i in g["items"]:
                if i.get("ok") and not self.is_archive(i["src"]):
                    w = i["src"].split("/")[0]
                    for n in (GRAPH_NAME, AREA_NAME):
                        if (self.root / w / n).is_file():
                            out.add(f"{w}/{n}")
                    for n in INDEX_NAMES[:1]:
                        d = posixpath.dirname(i["target"])
                        if (self.root / d / n).is_file():
                            out.add(f"{d}/{n}")
        return sorted(out)

    # ------------------------------------------------------------ création
    def create(self, kind, parent, name, title="", desc=""):
        parent = (parent or "").strip("/")
        try:
            pp = self.app.safe(parent) if parent else None
        except ValueError as e:
            return {"ok": False, "error": str(e)}
        if pp is None or not pp.is_dir():
            return {"ok": False, "error": "choisis d'abord un dossier sur la carte"}
        name = safe_name(name or "")
        if not name or name == "sans-nom":
            return {"ok": False, "error": "nom vide"}
        title = (title or "").strip() or (posixpath.splitext(name)[0].replace("-", " ").replace("_", " ").strip() or name)
        desc = (desc or "").strip().replace("\n", " ")
        if kind == "note":
            if self.is_archive(parent):
                return {"ok": False, "error": "les notes vont dans le cerveau, pas dans l'archive"}
            if not posixpath.splitext(name)[1]:
                name += ".md"
        elif kind != "dir":
            return {"ok": False, "error": "type inconnu"}
        rel = f"{parent}/{name}"
        if (self.root / rel).exists():
            return {"ok": False, "error": "un élément porte déjà ce nom ici"}
        jid, jdir, journal = new_journal("création")
        backups = {}

        def backup(r):
            _backup(self.root, jdir, backups, r)

        report = {"created": [], "notes": []}
        if kind == "dir":
            (self.root / rel).mkdir()
            journal["created_dirs"].append(rel)
            report["created"].append(rel + "/")
        else:
            backup(rel)
            (self.root / rel).write_text(f"# {title}\n\n" + (desc + "\n" if desc else ""), "utf-8")
            report["created"].append(rel)
        if not self.is_archive(rel):
            try:
                self._maps_create(kind, rel, title, desc, backup, report)
            except Exception as e:  # noqa
                report["error"] = f"mise à jour des index incomplète : {e}"
        finish_journal(self.root, jdir, journal, backups)
        self.app.start_scan()
        return {"ok": True, "id": jid, "path": rel, "open": rel + ("/INDEX.md" if kind == "dir" and
                (self.root / rel / "INDEX.md").exists() else ""), "report": report, "journal": str(jdir)}

    def _world_uses_maps(self, world):
        return (self.root / world / GRAPH_NAME).is_file() or (self.root / world / AREA_NAME).is_file()

    def _maps_create(self, kind, rel, title, desc, backup, report):
        world = rel.split("/")[0]
        if not self._world_uses_maps(world):
            return
        parent = posixpath.dirname(rel)
        listed = rel
        if kind == "dir":
            idx = f"{rel}/INDEX.md"
            backup(idx)
            (self.root / idx).write_text(f"# {title}\n\n" + (desc + "\n" if desc else ""), "utf-8")
            report["created"].append(idx)
            listed = idx
        # inscription dans l'index du dossier parent, sinon dans le AREA.md le plus proche
        lst = self._listing_file(parent, world)
        if lst:
            self._append_list_line(lst, listed, title, desc, backup, report)
        # GRAPH.json
        g = self._graph_load(world)
        if g:
            data, raw = g
            wr = listed[len(world) + 1:]
            ids = {n.get("id") for n in data["nodes"]}
            base = slugify(posixpath.basename(rel) if kind == "dir" else posixpath.splitext(posixpath.basename(rel))[0])
            nid, k = base, 2
            while nid in ids:
                pslug = slugify(posixpath.basename(parent)) if parent != world else ""
                nid = f"{pslug}-{base}" if pslug and k == 2 else f"{base}-{k}"
                k += 1
            data["nodes"].append({"id": nid, "label": title, "type": "index" if kind == "dir" else "note", "path": wr})
            pid = self._graph_parent(data, world, parent)
            if pid and isinstance(data.get("links"), list):
                data["links"].append({"from": pid, "to": nid, "rel": "contains"})
            self._graph_save(world, data, raw, backup, report)

    def _listing_file(self, d, world):
        if (self.root / d / "INDEX.md").is_file():
            return f"{d}/INDEX.md"
        while True:
            if (self.root / d / AREA_NAME).is_file():
                return f"{d}/{AREA_NAME}"
            if d == world or "/" not in d:
                return None
            d = posixpath.dirname(d)

    def _append_list_line(self, lst, target, title, desc, backup, report):
        p = self.root / lst
        text = p.read_text("utf-8")
        href = _href(posixpath.dirname(lst), target)
        if f"]({href})" in text or f"](<{href}>)" in text:
            return
        backup(lst)
        line = f"- [{title}]({href})" + (f" — {desc}" if desc else "")
        body = text.rstrip("\n")
        last = body.split("\n")[-1] if body else ""
        sep = "\n" if re.match(r"^\s*[-*]\s", last) or not body else "\n\n"   # une liste se sépare d'un paragraphe
        p.write_text(body + sep + line + "\n", "utf-8")
        report["notes"].append(f"ajouté à {lst}")

    # ------------------------------------------------------------ GRAPH.json
    def _graph_load(self, world):
        p = self.root / world / GRAPH_NAME
        if not p.is_file():
            return None
        try:
            raw = p.read_text("utf-8")
            data = json.loads(raw)
        except (OSError, ValueError):
            return None
        if not isinstance(data, dict) or not isinstance(data.get("nodes"), list):
            return None
        return data, raw

    def _graph_save(self, world, data, raw, backup, report):
        if "updated" in data:
            data["updated"] = time.strftime("%Y-%m-%d")
        m = re.search(r"\n( +)\"", raw)
        indent = len(m.group(1)) if m else 2
        out = json.dumps(data, ensure_ascii=False, indent=indent) + ("\n" if raw.endswith("\n") else "")
        if out != raw:
            backup(f"{world}/{GRAPH_NAME}")
            (self.root / world / GRAPH_NAME).write_text(out, "utf-8")
            report["notes"].append(f"{world}/{GRAPH_NAME} mis à jour")

    def _graph_parent(self, data, world, d):
        """Nœud qui représente le dossier d (son INDEX / README / AREA), sinon celui du dossier au-dessus."""
        by_path = {n.get("path"): n.get("id") for n in data["nodes"] if n.get("path")}
        while True:
            wr = d[len(world) + 1:] if d != world else ""
            for n in ("INDEX.md", "README.md", AREA_NAME, "00-bible.md"):
                key = f"{wr}/{n}" if wr else n
                if key in by_path:
                    return by_path[key]
            if d == world or "/" not in d:
                hub = next((n.get("id") for n in data["nodes"] if n.get("type") == "hub"), None)
                return hub
            d = posixpath.dirname(d)

    # ------------------------------------------------------------ après un déplacement (appelé par Mover.run)
    def after_moves(self, done, backup, report):
        moves = {p["src"]: p["target"] for p in done}
        self._fix_moved_notes(moves, backup, report)
        self._update_mentions(moves, backup, report)
        self._update_graphs(moves, backup, report)
        self._update_indexes(moves, backup, report)
        self._remove_empty_dirs(moves, report)

    def _fix_moved_notes(self, moves, backup, report):
        """Les liens relatifs À L'INTÉRIEUR d'une note déplacée : recalculés depuis son nouvel emplacement."""
        from urllib.parse import unquote
        for old, new in moves.items():
            if not is_note(new):
                continue
            p = self.root / new
            try:
                text = p.read_text("utf-8")
            except OSError:
                continue
            old_dir, new_dir = posixpath.dirname(old), posixpath.dirname(new)
            if old_dir == new_dir:
                continue
            count = [0]

            def fix(m):
                raw = m.group(2)
                h = raw[1:-1] if raw.startswith("<") else unquote(raw)
                anchor = ""
                if "#" in h:
                    h, anchor = h.split("#", 1)
                    anchor = "#" + anchor
                if not h or h.startswith("/") or re.match(r"^[a-z][a-z0-9+.-]*:", h, re.I):
                    return m.group(0)
                tgt = posixpath.normpath(posixpath.join(old_dir, h))
                tgt = moves.get(tgt, tgt)
                href = _href(new_dir, tgt)
                if anchor:
                    href = href[:-1] + anchor + ">" if href.startswith("<") else href + anchor
                if href == raw:
                    return m.group(0)
                count[0] += 1
                return relabel(m.group(1), raw, href) + href + m.group(3)

            out = IMGLINK.sub(fix, MDLINK.sub(fix, text))
            if count[0]:
                backup(new)
                p.write_text(out, "utf-8")
                report["links"] = report.get("links", 0) + count[0]
                report["notes"].append(f"{count[0]} lien(s) recalculé(s) dans la note déplacée {new}")

    def _dir_moves(self, moves):
        """Dossiers vidés par le déplacement : ancien dossier -> nouveau (si tout est parti au même endroit)."""
        by_dir = {}
        for old, new in moves.items():
            by_dir.setdefault(posixpath.dirname(old), set()).add(posixpath.dirname(new))
        out = {}
        for od, nds in by_dir.items():
            if len(nds) != 1 or not od:
                continue
            p = self.root / od
            left = [e for e in (os.listdir(p) if p.is_dir() else []) if e not in (".DS_Store",)]
            if not left:
                out[od] = next(iter(nds))
        return out

    def _update_mentions(self, moves, backup, report):
        """Chemins cités en clair (`Archive/hobby/…/x.jpg`, « Note: Hobby/…/x.md ») dans les notes et le catalogue."""
        repl = dict(moves)
        repl.update(self._dir_moves(moves))
        if not repl:
            return
        keys = sorted(repl, key=len, reverse=True)
        rx = re.compile(r"(?<![\w./-])(" + "|".join(re.escape(k) for k in keys) + r")(?=/|[\s`'\")\],;:|>]|$)")
        files = [r for r in self.sc.index_paths if is_note(r) and (self.root / r).is_file()]
        files += [n for n in moves.values() if is_note(n)]
        if self.cfg.archive_catalog and (self.root / self.cfg.archive_catalog).is_file():
            files.append(self.cfg.archive_catalog)
        files = [f for f in files if f != self.cfg.archive_log]     # le journal est un historique : on n'y réécrit rien
        firsts = {k.split("/")[0] for k in keys}
        n_total = 0
        for rel in sorted(set(files)):
            p = self.root / rel
            try:
                text = p.read_text("utf-8")
            except (OSError, UnicodeDecodeError):
                continue
            if not any(f + "/" in text for f in firsts):
                continue
            new, n = rx.subn(lambda m: repl[m.group(1)], text)
            if n:
                backup(rel)
                p.write_text(new, "utf-8")
                n_total += n
                report["notes"].append(f"{n} chemin(s) cité(s) mis à jour dans {rel}")
        report["mentions"] = n_total

    def _update_graphs(self, moves, backup, report):
        worlds = {}
        for old, new in moves.items():
            if self.is_archive(old) or self.is_archive(new):
                continue
            wo, wn = old.split("/")[0], new.split("/")[0]
            if wo != wn:
                continue
            worlds.setdefault(wo, []).append((old, new))
        for world, pairs in worlds.items():
            g = self._graph_load(world)
            if not g:
                continue
            data, raw = g
            changed = False
            for old, new in pairs:
                wo, wn = old[len(world) + 1:], new[len(world) + 1:]
                for n in data["nodes"]:
                    if n.get("path") == wo:
                        n["path"] = wn
                        changed = True
                        pid_new = self._graph_parent(data, world, posixpath.dirname(new))
                        links = data.get("links")
                        if pid_new and isinstance(links, list):
                            for l in links:
                                if l.get("to") == n.get("id") and l.get("rel") == "contains" and l.get("from") != pid_new:
                                    l["from"] = pid_new
            for key in ("homes",):
                for h in data.get(key) or []:
                    for old, new in pairs:
                        if isinstance(h, dict) and h.get("path") == old[len(world) + 1:]:
                            h["path"] = new[len(world) + 1:]
                            changed = True
            if changed:
                self._graph_save(world, data, raw, backup, report)

    def _update_indexes(self, moves, backup, report):
        """La note arrive dans un dossier qui a un INDEX.md : elle y est inscrite (et retirée de l'INDEX.md de départ)."""
        for old, new in moves.items():
            if not is_note(new) or self.is_archive(new):
                continue
            od, nd = posixpath.dirname(old), posixpath.dirname(new)
            if od == nd or posixpath.basename(new) in INDEX_NAMES:
                continue
            dest_idx = f"{nd}/INDEX.md"
            if not (self.root / dest_idx).is_file():
                continue
            desc = ""
            src_idx = f"{od}/INDEX.md"
            sp = self.root / src_idx
            if sp.is_file():
                text = sp.read_text("utf-8")
                lines, kept = text.split("\n"), []
                href_new = _href(od, new)
                for l in lines:
                    if re.match(r"^\s*[-*]\s", l) and (f"]({href_new})" in l or f"](<{href_new}>)" in l):
                        m = re.search(r"\)\s*(?:—|-|:)\s*(.+)$", l)
                        desc = desc or (m.group(1).strip() if m else "")
                        continue
                    kept.append(l)
                if len(kept) != len(lines):
                    backup(src_idx)
                    sp.write_text("\n".join(kept), "utf-8")
                    report["notes"].append(f"retiré de {src_idx}")
            title = posixpath.splitext(posixpath.basename(new))[0]
            try:
                m = re.search(r"^#\s+(.+)$", (self.root / new).read_text("utf-8"), re.M)
                if m:
                    title = m.group(1).strip()
            except OSError:
                pass
            self._append_list_line(dest_idx, new, title, desc, backup, report)

    def _remove_empty_dirs(self, moves, report):
        """Dossiers laissés vides par le déplacement (on garde la racine de chaque monde et de l'archive)."""
        for od in sorted({posixpath.dirname(o) for o in moves}, key=len, reverse=True):
            d = od
            while d and d.count("/") >= (2 if self.is_archive(d) else 1) and (self.root / d).is_dir():
                left = os.listdir(self.root / d)
                if any(e != ".DS_Store" for e in left):
                    break
                try:
                    for e in left:
                        os.remove(self.root / d / e)
                    os.rmdir(self.root / d)
                except OSError:
                    break
                report.setdefault("removed_dirs", []).append(d)
                d = posixpath.dirname(d)


def relabel(prefix, old_href, new_href):
    """[texte](lien) : si le texte affiché est le chemin du lien lui-même, il suit le nouveau chemin."""
    from urllib.parse import unquote
    i, j = prefix.find("["), prefix.rfind("]")
    if i < 0 or j <= i:
        return prefix
    inner = prefix[i + 1:j]
    o = old_href[1:-1] if old_href.startswith("<") else unquote(old_href)
    n = new_href[1:-1] if new_href.startswith("<") else unquote(new_href)
    if inner.strip() in (o, old_href):
        return prefix[:i + 1] + n + prefix[j:]
    return prefix


# ------------------------------------------------------------ journal (annulation)
def new_journal(note):
    jid = time.strftime("%Y-%m-%d_%H-%M-%S") + f"_{os.getpid() % 1000}"
    n = 1
    while (JOURNAL / jid).exists():
        n += 1
        jid = time.strftime("%Y-%m-%d_%H-%M-%S") + f"_{os.getpid() % 1000}_{n}"
    jdir = JOURNAL / jid
    (jdir / "texts").mkdir(parents=True, exist_ok=True)
    return jid, jdir, {"id": jid, "time": time.time(), "moves": [], "texts": {}, "created_dirs": [], "note": note}


def _backup(root, jdir, backups, rel):
    if rel in backups:
        return
    p = root / rel
    if p.exists():
        dst = jdir / "texts" / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(p, dst)
        backups[rel] = True
    else:
        backups[rel] = False


def finish_journal(root, jdir, journal, backups):
    for rel, existed in backups.items():
        journal["texts"][rel] = {"existed": existed, "sha_after": _sha(root / rel)}
    (jdir / "journal.json").write_text(json.dumps(journal, ensure_ascii=False, indent=1), "utf-8")
