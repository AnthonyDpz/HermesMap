"""Déplacement / renommage manuel de fichiers depuis Hermes Map, avec mise à jour des index :
- la fiche du catalog.yaml (déplacée vers le catalog.yaml du dossier principal de destination s'il existe),
- la ligne du catalogue de l'archive (CATALOG.jsonl), si un dossier d'archivage est configuré,
- une ligne dans le journal de l'archive (LOG.md, format tableau), s'il existe,
- les liens des notes .md qui pointaient vers l'ancien chemin.
Tout est sauvegardé avant et peut être annulé.
"""
import hashlib
import json
import os
import posixpath
import re
import shutil
import sys
import time
from pathlib import Path

import settings

JOURNAL = (Path(os.path.expanduser("~/Library/Application Support/hermes-map/moves")) if sys.platform == "darwin"
           else Path(os.path.expanduser("~/.local/share/hermes-map/moves")))
MDLINK = re.compile(r"(?<!!)(\[[^\]]*\]\()(<[^>\n]+>|[^)\s]+)((?:\s+\"[^\"]*\")?\))")
IMGLINK = re.compile(r"(!\[[^\]]*\]\()(<[^>\n]+>|[^)\s]+)((?:\s+\"[^\"]*\")?\))")
KIND_BY_EXT = {"image": {".jpg", ".jpeg", ".png", ".gif", ".webp", ".heic", ".bmp", ".tif", ".tiff", ".avif"},
               "video": {".mp4", ".mov", ".m4v", ".webm", ".avi", ".mkv", ".wmv", ".mpg", ".mpeg"},
               "pdf": {".pdf"}, "office": {".doc", ".docx", ".xls", ".xlsx", ".ppt", ".pptx", ".pages", ".numbers", ".key", ".odt"},
               "text": {".txt", ".md", ".csv", ".json", ".yaml", ".yml"}}


def _sha(p: Path):
    try:
        return hashlib.sha1(p.read_bytes()).hexdigest()
    except OSError:
        return None


def _kind(name):
    ext = posixpath.splitext(name)[1].lower()
    for k, exts in KIND_BY_EXT.items():
        if ext in exts:
            return k
    return "file"


def _href(from_dir, target, absolute_root=None, keep_absolute=False):
    if keep_absolute and absolute_root:
        h = absolute_root + "/" + target
    else:
        h = posixpath.relpath(target, from_dir or ".")
    return f"<{h}>" if re.search(r"[\s()<>]", h) else h


def safe_name(name):
    name = name.strip().replace("/", "-").replace("\\", "-")
    name = re.sub(r"[\x00-\x1f:]", "", name)
    return name.strip(". ") or "sans-nom"


class Mover:
    def __init__(self, app):
        self.app = app
        self.root = app.root
        self.sc = app.scanner

    # ------------------------------------------------------------ préparation
    def plan(self, items, create_dirs=True):
        """Vérifie chaque déplacement demandé. items = [{src, dest_dir, name}]"""
        out = []
        seen_targets = set()
        for it in items:
            src = (it.get("src") or "").strip("/")
            dest_dir = (it.get("dest_dir") or "").strip("/")
            name = safe_name(it.get("name") or posixpath.basename(src))
            r = {"src": src, "dest_dir": dest_dir, "name": name, "ok": False}
            try:
                sp = self.app.safe(src)
                if not sp.is_file():
                    r["error"] = "fichier introuvable"
                    out.append(r)
                    continue
                if not posixpath.splitext(name)[1] and posixpath.splitext(src)[1]:
                    name = name + posixpath.splitext(src)[1]
                    r["name"] = name
                if not dest_dir:
                    r["error"] = "destination vide"
                    out.append(r)
                    continue
                dp = (self.root / dest_dir).resolve()
                dp.relative_to(self.root)
                if not self.sc.allowed(dp):
                    r["error"] = "destination hors des dossiers suivis"
                    out.append(r)
                    continue
                if not dp.exists() and not create_dirs:
                    r["error"] = "le dossier de destination n'existe pas"
                    out.append(r)
                    continue
                if dp.exists() and not dp.is_dir():
                    r["error"] = "la destination n'est pas un dossier"
                    out.append(r)
                    continue
                target = f"{dest_dir}/{name}"
                if target == src:
                    r["error"] = "même emplacement et même nom"
                    out.append(r)
                    continue
                if (self.root / target).exists() or target in seen_targets:
                    r["error"] = "un fichier porte déjà ce nom à cet endroit"
                    out.append(r)
                    continue
                seen_targets.add(target)
                r.update(ok=True, target=target, new_dir=not dp.exists(),
                         size=sp.stat().st_size)
            except ValueError:
                r["error"] = "chemin hors du dossier"
            out.append(r)
        return out

    # ------------------------------------------------------------ exécution
    def run(self, items, create_dirs=True, update_index=True, note=""):
        plan = self.plan(items, create_dirs)
        todo = [p for p in plan if p["ok"]]
        if not todo:
            return {"ok": False, "plan": plan, "error": "rien à déplacer"}
        jid = time.strftime("%Y-%m-%d_%H-%M-%S") + f"_{os.getpid() % 1000}"
        jdir = JOURNAL / jid
        (jdir / "texts").mkdir(parents=True, exist_ok=True)
        journal = {"id": jid, "time": time.time(), "moves": [], "texts": {}, "created_dirs": [], "note": note}
        backups = {}

        def backup(rel):
            if rel in backups:
                return
            p = self.root / rel
            if p.exists():
                dst = jdir / "texts" / rel
                dst.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(p, dst)
                backups[rel] = True
            else:
                backups[rel] = False      # fichier créé par l'opération (sera supprimé à l'annulation)

        # 1. déplacements
        done = []
        for p in todo:
            src, target = self.root / p["src"], self.root / p["target"]
            try:
                if not target.parent.exists():
                    target.parent.mkdir(parents=True, exist_ok=True)
                    journal["created_dirs"].append(p["dest_dir"])
                os.rename(src, target)
            except OSError:
                try:
                    shutil.move(str(src), str(target))
                except OSError as e:
                    p.update(ok=False, error=f"déplacement impossible : {e}")
                    continue
            journal["moves"].append({"old": p["src"], "new": p["target"], "size": p["size"]})
            done.append(p)

        # 2. index
        report = {"catalog_yaml": 0, "catalog_jsonl": 0, "log": 0, "links": 0, "notes": []}
        if update_index and done:
            try:
                # descriptions lues AVANT de toucher aux catalogues
                cat = self.sc.catalog() if hasattr(self.sc, "catalog") else {}
                desc_by_new = {p["target"]: cat[p["src"]]["description"] for p in done
                               if cat.get(p["src"], {}).get("description")}
                self._update_catalog_yaml(done, backup, report)
                self._update_catalog_jsonl(done, backup, report, desc_by_new)
                self._update_log(done, backup, report, note)
                self._update_links(done, backup, report)
            except Exception as e:  # noqa
                report["error"] = f"mise à jour des index incomplète : {e}"

        # signature de chaque texte modifié (pour ne pas écraser une retouche faite après, en cas d'annulation)
        for rel, existed in backups.items():
            journal["texts"][rel] = {"existed": existed, "sha_after": _sha(self.root / rel)}
        (jdir / "journal.json").write_text(json.dumps(journal, ensure_ascii=False, indent=1), "utf-8")
        self.app.start_scan()
        return {"ok": bool(done), "id": jid, "plan": plan, "moved": journal["moves"], "report": report,
                "journal": str(jdir)}

    # ------------------------------------------------------------ catalog.yaml
    def _catalogs(self, kind):
        names = ("catalog.yaml", "catalog.yml") if kind == "yaml" else ("catalog.jsonl",)
        return sorted(r for r in self.sc.index_paths if r.rsplit("/", 1)[-1].lower() in names)

    def _vault_catalog(self, archive_rel):
        """catalog.yaml du dossier principal qui correspond à un chemin d'archive (Archive/photos/... -> Photos/catalog.yaml)."""
        parts = archive_rel.split("/")
        arch = settings.current.archive
        if len(parts) < 2 or parts[0] != arch:
            top = parts[0]
        else:
            brain = self.sc.brain_of(arch + "/" + parts[1])
            top = brain.split("/")[0] if brain else None
        if not top:
            return None
        for n in ("catalog.yaml", "catalog.yml"):
            if (self.root / top / n).exists():
                return f"{top}/{n}"
        return None

    def _vault_top(self, rel):
        parts = rel.split("/")
        arch = settings.current.archive
        if parts[0] != arch:
            return parts[0]
        brain = self.sc.brain_of(arch + "/" + parts[1]) if len(parts) > 1 else None
        return brain.split("/")[0] if brain else None

    @staticmethod
    def _split_entries(text):
        """Découpe un catalog.yaml en : en-tête, liste de blocs (une fiche chacun), fin."""
        lines = text.split("\n")
        start = next((i for i, l in enumerate(lines) if re.match(r"^entries\s*:", l)), None)
        if start is None:
            return None
        head, blocks, cur = lines[:start + 1], [], None
        for l in lines[start + 1:]:
            if re.match(r"^\s*-\s+\S", l):
                cur = [l]
                blocks.append(cur)
            elif cur is not None:
                cur.append(l)
            else:
                head.append(l)
        return head, blocks

    @staticmethod
    def _field(block, key):
        for l in block:
            m = re.match(r"^\s*(?:-\s+)?" + re.escape(key) + r"\s*:\s*(.*)$", l)
            if m:
                return m.group(1).strip().strip("\"'")
        return None

    def _block_target(self, block, cat_dir):
        root, path, link = self._field(block, "root"), self._field(block, "path"), self._field(block, "link")
        if root and path:
            return posixpath.normpath(f"{root}/{path}")
        if link:
            return posixpath.normpath(posixpath.join(cat_dir, link))
        if path:
            return posixpath.normpath(posixpath.join(cat_dir, path))
        return None

    def _rewrite_block(self, block, new_rel, cat_dir):
        root = self._field(block, "root")
        out = []
        for l in block:
            m = re.match(r"^(\s*(?:-\s+)?)(path|link)(\s*:\s*)(.*)$", l)
            if m:
                key = m.group(2)
                if key == "path":
                    val = posixpath.relpath(new_rel, root) if root and new_rel.startswith(root + "/") else \
                        posixpath.relpath(new_rel, cat_dir or ".")
                else:
                    val = posixpath.relpath(new_rel, cat_dir or ".")
                l = f'{m.group(1)}{key}{m.group(3)}"{val}"' if key == "link" else f"{m.group(1)}{key}{m.group(3)}{val}"
            out.append(l)
        return out

    def _update_catalog_yaml(self, done, backup, report):
        moves = {p["src"]: p["target"] for p in done}
        appended = {}
        for cat in self._catalogs("yaml"):
            cp = self.root / cat
            try:
                text = cp.read_text("utf-8")
            except OSError:
                continue
            parsed = self._split_entries(text)
            if not parsed:
                continue
            head, blocks = parsed
            cat_dir = posixpath.dirname(cat)
            keep, changed = [], False
            for b in blocks:
                tgt = self._block_target(b, cat_dir)
                if tgt in moves:
                    new = moves[tgt]
                    dest_cat = self._vault_catalog(new)
                    same_vault = self._vault_top(new) == cat.split("/")[0]
                    if dest_cat == cat or (dest_cat is None and same_vault):
                        keep.append(self._rewrite_block(b, new, cat_dir))
                    elif dest_cat:
                        appended.setdefault(dest_cat, []).append((b, new))
                        report["notes"].append(f"fiche déplacée de {cat} vers {dest_cat}")
                    else:
                        # le vault de destination n'a pas de catalog.yaml : la description part dans CATALOG.jsonl
                        report["notes"].append(f"fiche retirée de {cat} (description reportée dans CATALOG.jsonl)")
                    changed = True
                    report["catalog_yaml"] += 1
                else:
                    keep.append(b)
            if changed:
                backup(cat)
                cp.write_text("\n".join(head + [l for b in keep for l in b]), "utf-8")
        for dest_cat, entries in appended.items():
            dp = self.root / dest_cat
            backup(dest_cat)
            text = dp.read_text("utf-8") if dp.exists() else "entries:\n"
            if not re.search(r"^entries\s*:", text, re.M):
                text = text.rstrip("\n") + "\nentries:\n"
            cat_dir = posixpath.dirname(dest_cat)
            add = []
            for b, new in entries:
                rb = self._rewrite_block(b, new, cat_dir)
                # « root/path » relatifs à la racine : on les recalcule pour le nouveau vault
                add += [""] + rb
            dp.write_text(text.rstrip("\n") + "\n" + "\n".join(add).rstrip("\n") + "\n", "utf-8")

    # ------------------------------------------------------------ CATALOG.jsonl
    def _update_catalog_jsonl(self, done, backup, report, desc_by_new):
        for cat in self._catalogs("jsonl"):
            base = posixpath.dirname(cat)
            pre = base + "/" if base else ""
            cp = self.root / cat
            try:
                lines = cp.read_text("utf-8").splitlines()
            except OSError:
                continue
            changed, found = False, set()
            out = []
            for line in lines:
                try:
                    e = json.loads(line)
                except ValueError:
                    out.append(line)
                    continue
                full = posixpath.normpath(pre + e.get("rel", "")) if e.get("rel") else None
                hit = next((p for p in done if p["src"] == full), None)
                if hit:
                    found.add(hit["src"])
                    if hit["target"].startswith(pre):
                        nrel = hit["target"][len(pre):]
                        e.update(rel=nrel, name=posixpath.basename(nrel), ext=posixpath.splitext(nrel)[1].lower(),
                                 node=nrel.split("/")[0])
                        if desc_by_new.get(hit["target"]) and not e.get("note"):
                            e["note"] = desc_by_new[hit["target"]]
                        out.append(json.dumps(e, ensure_ascii=False))
                    changed = True
                    report["catalog_jsonl"] += 1
                else:
                    out.append(line)
            for p in done:
                if p["src"] not in found and p["target"].startswith(pre):
                    tp = self.root / p["target"]
                    nrel = p["target"][len(pre):]
                    e = {"rel": nrel, "name": posixpath.basename(nrel), "ext": posixpath.splitext(nrel)[1].lower(),
                         "kind": _kind(nrel), "bytes": tp.stat().st_size if tp.exists() else p.get("size"),
                         "mtime": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "node": nrel.split("/")[0]}
                    if desc_by_new.get(p["target"]):
                        e["note"] = desc_by_new[p["target"]]
                    out.append(json.dumps(e, ensure_ascii=False))
                    changed = True
                    report["catalog_jsonl"] += 1
            if changed:
                backup(cat)
                cp.write_text("\n".join(out) + "\n", "utf-8")

    # ------------------------------------------------------------ LOG.md
    def _update_log(self, done, backup, report, note):
        log = settings.current.archive_log
        if not log:
            return
        arch = settings.current.archive
        lp = self.root / log
        if not lp.exists():
            return
        backup(log)
        stamp = time.strftime("%Y-%m-%dT%H:%M:%S") + time.strftime("%z")[:3] + ":" + time.strftime("%z")[3:]
        rows = []
        for p in done:
            od, on = posixpath.split(p["src"])
            nd, nn = posixpath.split(p["target"])
            rel = lambda d: (d[len(arch) + 1:] if d.startswith(arch + "/") else ("../" + d)) + "/"  # noqa: E731
            action = "move+rename" if od != nd and on != nn else ("rename" if od == nd else "move")
            txt = (note or "").replace("|", "/").strip()
            rows.append(f"| {stamp} | {action} | {rel(od)} | {rel(nd)} | {on} | {nn} | {p.get('size', '')} | "
                        f"Hermes Map (manuel){' — ' + txt if txt else ''} |")
        text = lp.read_text("utf-8")
        with open(lp, "a", encoding="utf-8") as fh:
            fh.write(("" if text.endswith("\n") else "\n") + "\n".join(rows) + "\n")
        report["log"] = len(rows)

    # ------------------------------------------------------------ liens des notes
    def _update_links(self, done, backup, report):
        moves = {p["src"]: p["target"] for p in done}
        olds_names = {posixpath.basename(s).lower() for s in moves}
        from urllib.parse import quote, unquote  # noqa: F401
        root_abs = str(self.root)
        for rel in sorted(self.sc.index_paths):
            if not rel.lower().endswith((".md", ".markdown")):
                continue
            p = self.root / rel
            try:
                text = p.read_text("utf-8")
            except OSError:
                continue
            low = text.lower()
            if not any(n in low or n.replace(" ", "%20") in low for n in olds_names):
                continue
            note_dir = posixpath.dirname(rel)
            count = [0]

            def fix(m):
                raw = m.group(2)
                h = raw[1:-1] if raw.startswith("<") else unquote(raw)
                anchor = ""
                if "#" in h:
                    h, anchor = h.split("#", 1)
                    anchor = "#" + anchor
                absolute = h.startswith("/")
                if absolute:
                    if not h.startswith(root_abs + "/"):
                        return m.group(0)
                    tgt = posixpath.normpath(h[len(root_abs) + 1:])
                else:
                    tgt = posixpath.normpath(posixpath.join(note_dir, h))
                if tgt not in moves:
                    return m.group(0)
                count[0] += 1
                href = _href(note_dir, moves[tgt], root_abs, keep_absolute=absolute)
                if anchor:
                    href = href[:-1] + anchor + ">" if href.startswith("<") else href + anchor
                return m.group(1) + href + m.group(3)

            new = MDLINK.sub(fix, text)
            new = IMGLINK.sub(fix, new)
            if count[0]:
                backup(rel)
                p.write_text(new, "utf-8")
                report["links"] += count[0]
                report["notes"].append(f"{count[0]} lien(s) mis à jour dans {rel}")

    # ------------------------------------------------------------ annulation
    def undo(self, jid):
        jdir = JOURNAL / jid
        j = json.loads((jdir / "journal.json").read_text("utf-8"))
        if j.get("undone"):
            return {"ok": False, "error": "déjà annulé"}
        back, skipped = [], []
        for m in reversed(j["moves"]):
            new, old = self.root / m["new"], self.root / m["old"]
            if new.exists() and not old.exists():
                old.parent.mkdir(parents=True, exist_ok=True)
                os.rename(new, old)
                back.append(m["old"])
            else:
                skipped.append(m["new"])
        restored = []
        for rel, info in j["texts"].items():
            p = self.root / rel
            if p.exists() and info.get("sha_after") and _sha(p) != info["sha_after"]:
                skipped.append(rel + " (modifié depuis)")
                continue
            if info.get("existed"):
                shutil.copy2(jdir / "texts" / rel, p)
                restored.append(rel)
            elif p.exists():
                p.unlink()
                restored.append(rel)
        for d in sorted(j.get("created_dirs", []), key=len, reverse=True):
            try:
                (self.root / d).rmdir()        # seulement s'il est resté vide
            except OSError:
                pass
        j["undone"] = time.time()
        (jdir / "journal.json").write_text(json.dumps(j, ensure_ascii=False, indent=1), "utf-8")
        self.app.start_scan()
        return {"ok": True, "moved_back": back, "restored": restored, "skipped": skipped}


def suggest_dirs(scanner, q, limit=40):
    """Dossiers proposés comme destination : par mots, archive d'abord."""
    words = [w for w in re.split(r"\s+", q.lower().strip()) if w]
    out = []
    for d in scanner.index_dirs:
        if "/." in d or d.startswith("."):
            continue
        low = d.lower()
        if all(w in low for w in words):
            out.append(d)
    arch = settings.current.archive
    out.sort(key=lambda d: (not (arch and d.startswith(arch + "/")), d.count("/"), d.lower()))
    return out[:limit]
