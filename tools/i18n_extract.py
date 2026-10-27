#!/usr/bin/env python3
"""Liste les textes français de l'interface (static/index.html) qui n'ont pas encore de traduction
dans static/i18n/en.json.  Usage : python3 tools/i18n_extract.py [--all] [--json]

Les morceaux de texte sont découpés comme le navigateur les verra (un nœud texte entre deux balises).
Les parties variables ${…} deviennent {0}, {1}… dans la clé."""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FR_HINT = re.compile(r"[a-zà-ÿ]{2,}", re.I)
FR_WORD = re.compile(r"(?i)[àâçéèêëîïôûùüÿœ]|\b(le|la|les|un|une|des|du|de|et|ou|en|au|aux|pour|par|sur|avec|sans|dans|ce|cette|ces|est|sont|pas|ne|plus|tout|tous|à|ici|fichier|fichiers|dossier|dossiers|nom|taille|type|voir|ouvrir|annuler|fermer|chercher|recherche|lien|liens|carte|note|notes)\b")


def is_text(s):
    t = s.strip()
    if not t or not FR_HINT.search(t):
        return False
    if re.fullmatch(r"[\w\-#.:/\[\]=\"'()*,> ]+", t) and " " not in t and not re.search(r"[À-ÿ]", t):
        return bool(re.fullmatch(r"[A-ZÀ-Ý][a-zà-ÿ]+", t))       # mot seul avec majuscule : « Dossier »
    if t.startswith(("/api", "#", "http", "hm-", "data:")) or (t.startswith(".") and not t.startswith(". ")) \
            or "=>" in t or t.count("(") > 2:
        return False
    return bool(FR_WORD.search(t)) or bool(re.fullmatch(r"[A-ZÀ-Ý][\w\s'’…·:,!?()«»-]*", t))


def scan_js(src, out):
    """Parcourt le JS : chaînes '…', "…" et gabarits `…${…}…` (récursivement)."""
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c == "/" and src[i:i + 2] == "//":
            j = src.find("\n", i)
            i = n if j < 0 else j
            continue
        if c == "/" and src[i:i + 2] == "/*":
            j = src.find("*/", i + 2)
            i = n if j < 0 else j + 2
            continue
        if c in "'\"":
            j, buf = i + 1, []
            while j < n and src[j] != c:
                if src[j] == "\\":
                    buf.append(src[j + 1])
                    j += 2
                    continue
                if src[j] == "\n":
                    break
                buf.append(src[j])
                j += 1
            s = "".join(buf)
            if "<" in s:
                for piece in re.split(r"<[^>]*>", s):
                    add(piece, out)
                add_attrs(s, out)
            else:
                add(s, out)
            i = j + 1
            continue
        if c == "`":
            i = scan_template(src, i + 1, out)
            continue
        i += 1


def scan_template(src, i, out):
    parts, buf, n, k = [], [], len(src), 0
    while i < n:
        c = src[i]
        if c == "\\":
            buf.append(src[i + 1])
            i += 2
            continue
        if c == "`":
            break
        if c == "$" and src[i + 1:i + 2] == "{":
            depth, j = 1, i + 2
            while j < n and depth:
                if src[j] == "`":
                    j = skip_template(src, j + 1)
                    continue
                if src[j] in "'\"":
                    q = src[j]
                    j += 1
                    while j < n and src[j] != q:
                        j += 2 if src[j] == "\\" else 1
                if src[j] == "{":
                    depth += 1
                elif src[j] == "}":
                    depth -= 1
                j += 1
            expr = src[i + 2:j - 1]
            scan_js(expr, out)                      # textes dans l'expression
            inline = not ("`" in expr or "<" in expr or "html" in expr.lower() or ".map(" in expr or ".join(" in expr)
            if inline:
                buf.append("{%d}" % k)
                k += 1
            else:
                buf.append("<x>")                   # l'expression produit du HTML : frontière de nœud
            i = j
            continue
        buf.append(c)
        i += 1
    s = "".join(buf)
    pieces = re.split(r"<[^>]*>", s)
    for piece in pieces:
        add(renumber(piece), out)
    add_attrs(s, out)
    return i + 1


def skip_template(src, i):
    n = len(src)
    while i < n and src[i] != "`":
        if src[i] == "\\":
            i += 2
            continue
        if src[i] == "$" and src[i + 1:i + 2] == "{":
            depth, i = 1, i + 2
            while i < n and depth:
                if src[i] == "`":
                    i = skip_template(src, i + 1)
                    continue
                depth += {"{": 1, "}": -1}.get(src[i], 0)
                i += 1
            continue
        i += 1
    return i + 1


def renumber(piece):
    m = {}
    return re.sub(r"\{(\d+)\}", lambda x: "{%d}" % m.setdefault(x.group(1), len(m)), piece)


def add_attrs(s, out):
    for _, a in re.findall(r"""(?:title|placeholder|aria-label|alt)=(["'])(.*?)\1""", s):
        add(renumber(a), out)


def add(s, out):
    t = re.sub(r"\s+", " ", s).strip()
    if t and is_text(re.sub(r"\{\d+\}", "", t)) and not re.fullmatch(r"(\{\d+\}\s*)+", t):
        out.add(t)


def extract():
    html = (ROOT / "static" / "index.html").read_text("utf-8")
    out = set()
    body = re.sub(r"<script.*?</script>|<style.*?</style>", "", html, flags=re.S)
    for piece in re.split(r"<[^>]*>", body):
        add(piece, out)
    add_attrs(body, out)
    for js in re.findall(r"<script>(.*?)</script>", html, flags=re.S):
        scan_js(js, out)
    return sorted(out)


if __name__ == "__main__":
    keys = extract()
    if "--all" not in sys.argv:
        try:
            done = json.loads((ROOT / "static" / "i18n" / "en.json").read_text("utf-8"))
        except OSError:
            done = {}
        keys = [k for k in keys if k not in done.get("exact", {}) and k not in done.get("patterns", {})
                and k not in done.get("same", [])]
    if "--json" in sys.argv:
        print(json.dumps(keys, ensure_ascii=False, indent=1))
    else:
        print("\n".join(keys))
        print(f"\n{len(keys)} texte(s)", file=sys.stderr)
