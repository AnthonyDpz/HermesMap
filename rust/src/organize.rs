//! Boîte à outils : créer un dossier ou une note, et ranger par glisser-déposer (même logique que organize.py).
//!
//! Règle cerveau / archive (si un dossier d'archivage est configuré) : les notes restent dans le cerveau, les
//! autres fichiers vont dans l'archive au chemin « miroir ». Une note et les fichiers d'archive qu'elle cite
//! se déplacent ensemble, en gardant la structure sous le miroir.
//! Fichiers d'organisation tenus à jour s'ils existent : GRAPH.json, AREA.md, INDEX.md.

use crate::app::App;
use crate::mover::{finish_journal, href, new_journal, safe_name, Ctx};
use crate::scan::Scanner;
use crate::util::{basename, dirname, join, normpath, unquote};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use unicode_normalization::UnicodeNormalization;

static LINK: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(!?\[[^\]]*\]\()(<[^>\n]+>|[^)\s]+)((?:\s+"[^"]*")?\))"#).unwrap());
static SCHEME: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^[a-z][a-z0-9+.-]*:").unwrap());
const NOTE_EXT: [&str; 2] = [".md", ".markdown"];
const TEXT_EXT: [&str; 12] = [".md", ".markdown", ".txt", ".json", ".jsonl", ".yaml", ".yml", ".csv", ".toml", ".ini", ".cfg", ".conf"];
const GRAPH_NAME: &str = "GRAPH.json";
const AREA_NAME: &str = "AREA.md";

pub fn slugify(s: &str) -> String {
    let a: String = s.nfkd().filter(|c| c.is_ascii()).collect();
    let out = Regex::new(r"[^A-Za-z0-9]+").unwrap().replace_all(&a, "-").trim_matches('-').to_lowercase();
    if out.is_empty() { "x".into() } else { out }
}

fn is_note(rel: &str) -> bool {
    let l = rel.to_lowercase();
    NOTE_EXT.iter().any(|e| l.ends_with(e))
}

fn is_text(rel: &str) -> bool {
    let l = rel.to_lowercase();
    TEXT_EXT.iter().any(|e| l.ends_with(e))
}

fn ext_of(name: &str) -> String {
    let b = basename(name);
    match b.rfind('.') {
        Some(i) if i > 0 => b[i..].to_string(),
        _ => String::new(),
    }
}

fn strip_href(raw: &str) -> String {
    if raw.starts_with('<') && raw.ends_with('>') { raw[1..raw.len() - 1].to_string() } else { unquote(raw) }
}

/// Cibles (chemins depuis la racine) des liens Markdown d'une note.
fn link_targets(text: &str, note_rel: &str, root_abs: &str) -> Vec<String> {
    let note_dir = dirname(note_rel);
    let mut out = vec![];
    for m in LINK.captures_iter(text) {
        let mut h = strip_href(&m[2]);
        if let Some(i) = h.find('#') {
            h.truncate(i);
        }
        if h.is_empty() || SCHEME.is_match(&h) {
            continue;
        }
        let t = if h.starts_with('/') {
            match h.strip_prefix(&format!("{}/", root_abs)) {
                Some(x) => normpath(x),
                None => continue,
            }
        } else {
            normpath(&join(&note_dir, &h))
        };
        if !t.starts_with("..") {
            out.push(t);
        }
    }
    out
}

/// [texte](lien) : si le texte affiché est le chemin du lien lui-même, il suit le nouveau chemin.
pub fn relabel(prefix: &str, old_href: &str, new_href: &str) -> String {
    let (Some(i), Some(j)) = (prefix.find('['), prefix.rfind(']')) else { return prefix.to_string() };
    if j <= i {
        return prefix.to_string();
    }
    let inner = &prefix[i + 1..j];
    let o = strip_href(old_href);
    let n = strip_href(new_href);
    if inner.trim() == o || inner.trim() == old_href {
        return format!("{}{}{}", &prefix[..i + 1], n, &prefix[j..]);
    }
    prefix.to_string()
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

pub struct Organizer {
    app: Arc<App>,
    root: PathBuf,
    sc: Arc<Scanner>,
    arch: Option<String>,
    root_abs: String,
    texts: RefCell<HashMap<String, Option<String>>>,
}

impl Organizer {
    pub fn new(app: Arc<App>) -> Self {
        let root = app.root().unwrap_or_default();
        let sc = app.scanner().expect("scanner");
        let arch = sc.cfg.archive();
        let root_abs = root.to_string_lossy().to_string();
        Organizer { app, root, sc, arch, root_abs, texts: RefCell::new(HashMap::new()) }
    }

    // ------------------------------------------------------------ chemins
    fn is_archive(&self, rel: &str) -> bool {
        self.arch.as_ref().map(|a| rel == a || rel.starts_with(&format!("{}/", a))).unwrap_or(false)
    }

    fn mirror_path(&self, brain_rel: &str) -> Option<String> {
        let arch = self.arch.as_ref()?;
        if brain_rel.is_empty() || self.is_archive(brain_rel) {
            return None;
        }
        let (top, rest) = match brain_rel.find('/') {
            Some(i) => (&brain_rel[..i], &brain_rel[i + 1..]),
            None => (brain_rel, ""),
        };
        let area = self.sc.cfg.zone_name(top);
        let mut base = format!("{}/{}", arch, area.to_lowercase());
        if !self.root.join(&base).is_dir() && self.root.join(format!("{}/{}", arch, area)).is_dir() {
            base = format!("{}/{}", arch, area);
        }
        Some(if rest.is_empty() { base } else { format!("{}/{}", base, rest) })
    }

    fn brain_path(&self, arch_rel: &str) -> Option<String> {
        let arch = self.arch.as_ref()?;
        let parts: Vec<&str> = arch_rel.split('/').collect();
        if parts.len() < 2 || parts[0] != arch {
            return None;
        }
        let area = parts[1].to_lowercase();
        let mut tops: Vec<String> = fs::read_dir(&self.root).ok()?.flatten()
            .filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
        tops.sort();
        for t in tops {
            if &t != arch && self.sc.top_allowed(&t) && self.sc.cfg.zone_name(&t).to_lowercase() == area {
                let mut v = vec![t];
                v.extend(parts[2..].iter().map(|s| s.to_string()));
                return Some(v.join("/"));
            }
        }
        None
    }

    fn text(&self, rel: &str) -> Option<String> {
        if let Some(t) = self.texts.borrow().get(rel) {
            return t.clone();
        }
        let t = fs::read_to_string(self.root.join(rel)).ok();
        self.texts.borrow_mut().insert(rel.to_string(), t.clone());
        t
    }

    fn index_paths(&self) -> Vec<String> {
        let mut v: Vec<String> = self.sc.data.read().unwrap().index_paths.iter().cloned().collect();
        v.sort();
        v
    }

    fn notes(&self) -> Vec<String> {
        self.index_paths().into_iter().filter(|r| is_note(r) && !self.is_archive(r)).collect()
    }

    fn links_of(&self, note: &str) -> Vec<String> {
        let Some(t) = self.text(note) else { return vec![] };
        let mut seen = BTreeSet::new();
        let mut out = vec![];
        for x in link_targets(&t, note, &self.root_abs) {
            if !seen.contains(&x) && self.root.join(&x).is_file() {
                seen.insert(x.clone());
                out.push(x);
            }
        }
        out
    }

    fn linkers(&self, target: &str) -> Vec<String> {
        let name = basename(target).to_lowercase();
        let mut out = vec![];
        for n in self.notes() {
            let Some(t) = self.text(&n) else { continue };
            let low = t.to_lowercase();
            if !low.contains(&name) && !low.contains(&name.replace(' ', "%20")) {
                continue;
            }
            if self.links_of(&n).iter().any(|x| x == target) {
                out.push(n);
            }
        }
        out
    }

    fn in_note_mirror(&self, f: &str, note: &str) -> bool {
        self.mirror_path(&dirname(note)).map(|m| f.starts_with(&format!("{}/", m))).unwrap_or(false)
    }

    fn item(&self, src: &str, target: &str, kind: &str, dragged: &str) -> Value {
        let size = fs::metadata(self.root.join(src)).map(|m| m.len()).unwrap_or(0);
        let mut it = json!({"src": src, "target": target, "kind": kind, "checked": true, "ok": true,
                            "dragged": src == dragged, "size": size});
        if target == src {
            it["ok"] = json!(false);
            it["checked"] = json!(false);
            it["error"] = json!("déjà à cet endroit");
        } else if self.root.join(target).exists() {
            it["ok"] = json!(false);
            it["checked"] = json!(false);
            it["error"] = json!("un fichier porte déjà ce nom à l'arrivée");
        }
        it
    }

    // ------------------------------------------------------------ plan du glisser-déposer
    pub fn plan(&self, srcs: &[String], dest: &str) -> Value {
        let dest = dest.trim_matches('/').to_string();
        if dest.is_empty() {
            return json!({"ok": false, "error": "la destination n'est pas un dossier"});
        }
        match self.app.safe(&dest) {
            Ok(p) if p.is_dir() => {}
            Ok(_) => return json!({"ok": false, "error": "la destination n'est pas un dossier"}),
            Err(e) => return json!({"ok": false, "error": e}),
        }
        if self.arch.as_deref() == Some(dest.as_str()) {
            return json!({"ok": false, "error": "choisis un dossier dans l'archive, pas sa racine"});
        }
        let in_arch = self.is_archive(&dest);
        let brain_dest = if in_arch { self.brain_path(&dest) } else { Some(dest.clone()) };
        let arch_dest = if in_arch { Some(dest.clone()) } else { self.mirror_path(&dest) };
        let mut groups: Vec<Value> = vec![];
        let mut grouped: BTreeSet<String> = BTreeSet::new();
        let mut errors: Vec<Value> = vec![];
        for src in srcs.iter().map(|s| s.trim_matches('/').to_string()) {
            let sp = match self.app.safe(&src) {
                Ok(p) => p,
                Err(e) => {
                    errors.push(json!({"src": src, "error": e}));
                    continue;
                }
            };
            if sp.is_dir() {
                errors.push(json!({"src": src, "error": "déplacer un dossier entier n'est pas encore possible"}));
                continue;
            }
            if !sp.is_file() {
                errors.push(json!({"src": src, "error": "fichier introuvable"}));
                continue;
            }
            if grouped.contains(&src) {
                continue;
            }
            let note: Option<String> = if is_note(&src) && !self.is_archive(&src) {
                Some(src.clone())
            } else if self.is_archive(&src) {
                self.linkers(&src).into_iter().find(|n| self.in_note_mirror(&src, n))
            } else {
                None
            };
            if let Some(n) = &note {
                if grouped.contains(n) {
                    continue;
                }
            }
            let mut items: Vec<Value> = vec![];
            if let Some(note) = &note {
                let Some(bd) = &brain_dest else {
                    errors.push(json!({"src": src, "error": "pas de dossier de notes correspondant à cette destination"}));
                    continue;
                };
                items.push(self.item(note, &format!("{}/{}", bd, basename(note)), "note", &src));
                grouped.insert(note.clone());
                let old_mirror = self.mirror_path(&dirname(note));
                for c in self.links_of(note) {
                    if !self.is_archive(&c) || grouped.contains(&c) {
                        continue;
                    }
                    let Some(ad) = &arch_dest else { continue };
                    let shared: Vec<String> = self.linkers(&c).into_iter().filter(|n| n != note).collect();
                    let (tgt, reason) = match &old_mirror {
                        Some(om) if c.starts_with(&format!("{}/", om)) => (format!("{}/{}", ad, &c[om.len() + 1..]), None),
                        _ => (format!("{}/{}", ad, basename(&c)), Some("rangé hors du miroir de la note")),
                    };
                    let mut it = self.item(&c, &tgt, "file", &src);
                    if !shared.is_empty() {
                        it["shared"] = json!(shared);
                    }
                    if (!shared.is_empty() || reason.is_some()) && c != src {
                        it["checked"] = json!(false);
                    }
                    if let Some(r) = reason {
                        it["reason"] = json!(r);
                    }
                    items.push(it);
                    grouped.insert(c);
                }
            } else {
                let binary = !is_text(&src);
                let tgt = if self.arch.is_some() && binary {
                    let Some(ad) = &arch_dest else {
                        errors.push(json!({"src": src, "error": "pas de dossier d'archive correspondant"}));
                        continue;
                    };
                    format!("{}/{}", ad, basename(&src))
                } else {
                    let base = match (&brain_dest, in_arch) {
                        (Some(b), false) => b.clone(),
                        _ if self.arch.is_none() || in_arch => dest.clone(),
                        _ => {
                            errors.push(json!({"src": src, "error": "destination introuvable"}));
                            continue;
                        }
                    };
                    format!("{}/{}", base, basename(&src))
                };
                items.push(self.item(&src, &tgt, "file", &src));
                grouped.insert(src.clone());
            }
            groups.push(json!({"note": note, "items": items}));
        }
        let mut new_dirs: BTreeSet<String> = BTreeSet::new();
        let mut maps: BTreeSet<String> = BTreeSet::new();
        for g in &groups {
            for i in g["items"].as_array().unwrap() {
                if i["ok"] != json!(true) {
                    continue;
                }
                let (s, t) = (i["src"].as_str().unwrap(), i["target"].as_str().unwrap());
                let td = dirname(t);
                if !self.root.join(&td).exists() {
                    new_dirs.insert(td.clone());
                }
                if !self.is_archive(s) {
                    let w = s.split('/').next().unwrap_or("");
                    for n in [GRAPH_NAME, AREA_NAME] {
                        if self.root.join(w).join(n).is_file() {
                            maps.insert(format!("{}/{}", w, n));
                        }
                    }
                    if self.root.join(&td).join("INDEX.md").is_file() {
                        maps.insert(format!("{}/INDEX.md", td));
                    }
                }
            }
        }
        json!({"ok": true, "dest": dest, "brain_dest": brain_dest, "arch_dest": arch_dest, "groups": groups,
               "errors": errors, "new_dirs": new_dirs, "maps": maps})
    }

    // ------------------------------------------------------------ création
    pub fn create(&self, kind: &str, parent: &str, name: &str, title: &str, desc: &str) -> Value {
        let parent = parent.trim_matches('/').to_string();
        if parent.is_empty() {
            return json!({"ok": false, "error": "choisis d'abord un dossier sur la carte"});
        }
        match self.app.safe(&parent) {
            Ok(p) if p.is_dir() => {}
            Ok(_) => return json!({"ok": false, "error": "choisis d'abord un dossier sur la carte"}),
            Err(e) => return json!({"ok": false, "error": e}),
        }
        let mut name = safe_name(name);
        if name.is_empty() || name == "sans-nom" {
            return json!({"ok": false, "error": "nom vide"});
        }
        let title = {
            let t = title.trim().to_string();
            if !t.is_empty() {
                t
            } else {
                let stem = match name.rfind('.') { Some(i) if i > 0 => name[..i].to_string(), _ => name.clone() };
                let s = stem.replace(['-', '_'], " ").trim().to_string();
                if s.is_empty() { name.clone() } else { s }
            }
        };
        let desc = desc.trim().replace('\n', " ");
        match kind {
            "note" => {
                if self.is_archive(&parent) {
                    return json!({"ok": false, "error": "les notes vont dans le cerveau, pas dans l'archive"});
                }
                if ext_of(&name).is_empty() {
                    name.push_str(".md");
                }
            }
            "dir" => {}
            _ => return json!({"ok": false, "error": "type inconnu"}),
        }
        let rel = format!("{}/{}", parent, name);
        if self.root.join(&rel).exists() {
            return json!({"ok": false, "error": "un élément porte déjà ce nom ici"});
        }
        let (jid, jdir, mut journal) = new_journal("création");
        let mut ctx = Ctx { jdir: jdir.clone(), backups: BTreeMap::new() };
        let mut report = json!({"created": [], "notes": []});
        let first_line = |t: &str, d: &str| format!("# {}\n\n{}", t, if d.is_empty() { String::new() } else { format!("{}\n", d) });
        if kind == "dir" {
            if let Err(e) = fs::create_dir(self.root.join(&rel)) {
                return json!({"ok": false, "error": format!("création impossible : {}", e)});
            }
            journal["created_dirs"] = json!([rel]);
            push(&mut report["created"], format!("{}/", rel));
        } else {
            ctx.backup(&self.root, &rel);
            if let Err(e) = fs::write(self.root.join(&rel), first_line(&title, &desc)) {
                return json!({"ok": false, "error": format!("création impossible : {}", e)});
            }
            push(&mut report["created"], rel.clone());
        }
        if !self.is_archive(&rel) {
            self.maps_create(kind, &rel, &title, &desc, &mut ctx, &mut report);
        }
        finish_journal(&self.root, &ctx, &mut journal);
        self.app.start_scan();
        let open = if kind == "dir" && self.root.join(&rel).join("INDEX.md").exists() { format!("{}/INDEX.md", rel) } else { rel.clone() };
        json!({"ok": true, "id": jid, "path": rel, "open": open, "report": report, "journal": jdir.to_string_lossy()})
    }

    fn world_uses_maps(&self, world: &str) -> bool {
        self.root.join(world).join(GRAPH_NAME).is_file() || self.root.join(world).join(AREA_NAME).is_file()
    }

    fn maps_create(&self, kind: &str, rel: &str, title: &str, desc: &str, ctx: &mut Ctx, report: &mut Value) {
        let world = rel.split('/').next().unwrap_or("").to_string();
        if !self.world_uses_maps(&world) {
            return;
        }
        let parent = dirname(rel);
        let mut listed = rel.to_string();
        if kind == "dir" {
            let idx = format!("{}/INDEX.md", rel);
            ctx.backup(&self.root, &idx);
            let _ = fs::write(self.root.join(&idx), format!("# {}\n\n{}", title, if desc.is_empty() { String::new() } else { format!("{}\n", desc) }));
            push(&mut report["created"], idx.clone());
            listed = idx;
        }
        if let Some(lst) = self.listing_file(&parent, &world) {
            self.append_list_line(&lst, &listed, title, desc, ctx, report);
        }
        if let Some((mut data, raw)) = self.graph_load(&world) {
            let wr = listed[world.len() + 1..].to_string();
            let ids: BTreeSet<String> = data["nodes"].as_array().unwrap().iter().filter_map(|n| n["id"].as_str().map(String::from)).collect();
            let stem = if kind == "dir" {
                basename(rel)
            } else {
                let b = basename(rel);
                match b.rfind('.') { Some(i) if i > 0 => b[..i].to_string(), _ => b }
            };
            let base = slugify(&stem);
            let (mut nid, mut k) = (base.clone(), 2);
            while ids.contains(&nid) {
                let pslug = if parent != world { slugify(&basename(&parent)) } else { String::new() };
                nid = if !pslug.is_empty() && k == 2 { format!("{}-{}", pslug, base) } else { format!("{}-{}", base, k) };
                k += 1;
            }
            data["nodes"].as_array_mut().unwrap().push(json!({"id": nid, "label": title, "type": if kind == "dir" { "index" } else { "note" }, "path": wr}));
            if let Some(pid) = self.graph_parent(&data, &world, &parent) {
                if let Some(l) = data.get_mut("links").and_then(|v| v.as_array_mut()) {
                    l.push(json!({"from": pid, "to": nid, "rel": "contains"}));
                }
            }
            self.graph_save(&world, &mut data, &raw, ctx, report);
        }
    }

    fn listing_file(&self, d: &str, world: &str) -> Option<String> {
        if self.root.join(d).join("INDEX.md").is_file() {
            return Some(format!("{}/INDEX.md", d));
        }
        let mut d = d.to_string();
        loop {
            if self.root.join(&d).join(AREA_NAME).is_file() {
                return Some(format!("{}/{}", d, AREA_NAME));
            }
            if d == world || !d.contains('/') {
                return None;
            }
            d = dirname(&d);
        }
    }

    fn append_list_line(&self, lst: &str, target: &str, title: &str, desc: &str, ctx: &mut Ctx, report: &mut Value) {
        let p = self.root.join(lst);
        let Ok(text) = fs::read_to_string(&p) else { return };
        let h = href(&dirname(lst), target, "", false);
        if text.contains(&format!("]({})", h)) || text.contains(&format!("](<{}>)", h)) {
            return;
        }
        ctx.backup(&self.root, lst);
        let line = format!("- [{}]({}){}", title, h, if desc.is_empty() { String::new() } else { format!(" — {}", desc) });
        let body = text.trim_end_matches('\n');
        let last = body.rsplit('\n').next().unwrap_or("");
        let is_item = Regex::new(r"^\s*[-*]\s").unwrap().is_match(last);
        let sep = if is_item || body.is_empty() { "\n" } else { "\n\n" }; // une liste se sépare d'un paragraphe
        let _ = fs::write(&p, format!("{}{}{}\n", body, sep, line));
        push(&mut report["notes"], format!("ajouté à {}", lst));
    }

    // ------------------------------------------------------------ GRAPH.json
    fn graph_load(&self, world: &str) -> Option<(Value, String)> {
        let raw = fs::read_to_string(self.root.join(world).join(GRAPH_NAME)).ok()?;
        let data: Value = serde_json::from_str(&raw).ok()?;
        if !data.is_object() || !data["nodes"].is_array() {
            return None;
        }
        Some((data, raw))
    }

    fn graph_save(&self, world: &str, data: &mut Value, raw: &str, ctx: &mut Ctx, report: &mut Value) {
        if data.get("updated").is_some() {
            data["updated"] = json!(today());
        }
        let indent = Regex::new("\n( +)\"").unwrap().captures(raw).map(|c| c[1].len()).unwrap_or(2);
        let ind = " ".repeat(indent);
        let mut buf = vec![];
        let fmt = serde_json::ser::PrettyFormatter::with_indent(ind.as_bytes());
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
        if serde::Serialize::serialize(&*data, &mut ser).is_err() {
            return;
        }
        let mut out = String::from_utf8(buf).unwrap_or_default();
        if raw.ends_with('\n') {
            out.push('\n');
        }
        if out != raw {
            let rel = format!("{}/{}", world, GRAPH_NAME);
            ctx.backup(&self.root, &rel);
            let _ = fs::write(self.root.join(&rel), out);
            push(&mut report["notes"], format!("{} mis à jour", rel));
        }
    }

    fn graph_parent(&self, data: &Value, world: &str, d: &str) -> Option<String> {
        let by_path: HashMap<String, String> = data["nodes"].as_array()?.iter()
            .filter_map(|n| Some((n["path"].as_str()?.to_string(), n["id"].as_str()?.to_string()))).collect();
        let mut d = d.to_string();
        loop {
            let wr = if d != world && d.len() > world.len() { d[world.len() + 1..].to_string() } else { String::new() };
            for n in ["INDEX.md", "README.md", AREA_NAME, "00-bible.md"] {
                let key = if wr.is_empty() { n.to_string() } else { format!("{}/{}", wr, n) };
                if let Some(id) = by_path.get(&key) {
                    return Some(id.clone());
                }
            }
            if d == world || !d.contains('/') {
                return data["nodes"].as_array()?.iter().find(|n| n["type"] == "hub").and_then(|n| n["id"].as_str().map(String::from));
            }
            d = dirname(&d);
        }
    }

    // ------------------------------------------------------------ après un déplacement (appelé par Mover::run)
    pub fn after_moves(&self, done: &[Value], ctx: &mut Ctx, report: &mut Value) {
        let moves: BTreeMap<String, String> = done.iter()
            .map(|p| (p["src"].as_str().unwrap_or("").to_string(), p["target"].as_str().unwrap_or("").to_string())).collect();
        self.fix_moved_notes(&moves, ctx, report);
        self.update_mentions(&moves, ctx, report);
        self.update_graphs(&moves, ctx, report);
        self.update_indexes(&moves, ctx, report);
        self.remove_empty_dirs(&moves, report);
    }

    fn fix_moved_notes(&self, moves: &BTreeMap<String, String>, ctx: &mut Ctx, report: &mut Value) {
        for (old, new) in moves {
            if !is_note(new) {
                continue;
            }
            let p = self.root.join(new);
            let Ok(text) = fs::read_to_string(&p) else { continue };
            let (od, nd) = (dirname(old), dirname(new));
            if od == nd {
                continue;
            }
            let mut count = 0u64;
            let out = LINK.replace_all(&text, |m: &regex::Captures| {
                let raw = &m[2];
                let mut h = strip_href(raw);
                let mut anchor = String::new();
                if let Some(i) = h.find('#') {
                    anchor = h[i..].to_string();
                    h.truncate(i);
                }
                if h.is_empty() || h.starts_with('/') || SCHEME.is_match(&h) {
                    return m[0].to_string();
                }
                let tgt = normpath(&join(&od, &h));
                let tgt = moves.get(&tgt).cloned().unwrap_or(tgt);
                let mut hr = href(&nd, &tgt, "", false);
                if !anchor.is_empty() {
                    hr = if hr.starts_with('<') { format!("{}{}>", &hr[..hr.len() - 1], anchor) } else { format!("{}{}", hr, anchor) };
                }
                if hr == raw {
                    return m[0].to_string();
                }
                count += 1;
                format!("{}{}{}", relabel(&m[1], raw, &hr), hr, &m[3])
            });
            if count > 0 {
                ctx.backup(&self.root, new);
                let _ = fs::write(&p, out.as_bytes());
                report["links"] = json!(report["links"].as_u64().unwrap_or(0) + count);
                push(&mut report["notes"], format!("{} lien(s) recalculé(s) dans la note déplacée {}", count, new));
            }
        }
    }

    fn dir_moves(&self, moves: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut by_dir: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (o, n) in moves {
            by_dir.entry(dirname(o)).or_default().insert(dirname(n));
        }
        let mut out = BTreeMap::new();
        for (od, nds) in by_dir {
            if nds.len() != 1 || od.is_empty() {
                continue;
            }
            let left = fs::read_dir(self.root.join(&od)).map(|it| it.flatten().filter(|e| e.file_name() != ".DS_Store").count()).unwrap_or(0);
            if left == 0 {
                out.insert(od, nds.into_iter().next().unwrap());
            }
        }
        out
    }

    fn update_mentions(&self, moves: &BTreeMap<String, String>, ctx: &mut Ctx, report: &mut Value) {
        let mut repl = moves.clone();
        repl.extend(self.dir_moves(moves));
        if repl.is_empty() {
            return;
        }
        let mut keys: Vec<&String> = repl.keys().collect();
        keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
        let rx = Regex::new(&keys.iter().map(|k| regex::escape(k)).collect::<Vec<_>>().join("|")).unwrap();
        let mut files: BTreeSet<String> = self.index_paths().into_iter().filter(|r| is_note(r) && self.root.join(r).is_file()).collect();
        files.extend(moves.values().filter(|n| is_note(n)).cloned());
        if let Some(c) = self.sc.cfg.archive_catalog() {
            if self.root.join(&c).is_file() {
                files.insert(c);
            }
        }
        if let Some(l) = self.sc.cfg.archive_log() {
            files.remove(&l); // le journal est un historique : on n'y réécrit rien
        }
        let firsts: BTreeSet<String> = keys.iter().map(|k| format!("{}/", k.split('/').next().unwrap_or(""))).collect();
        let before_ok = |c: char| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '/' || c == '-');
        let after_ok = |c: char| c == '/' || c.is_whitespace() || "`'\")],;:|>".contains(c);
        let mut total = 0u64;
        for rel in files {
            let p = self.root.join(&rel);
            let Ok(text) = fs::read_to_string(&p) else { continue };
            if !firsts.iter().any(|f| text.contains(f.as_str())) {
                continue;
            }
            let mut out = String::with_capacity(text.len());
            let (mut last, mut pos, mut n) = (0usize, 0usize, 0u64);
            while let Some(m) = rx.find_at(&text, pos) {
                let ok_b = text[..m.start()].chars().next_back().map(before_ok).unwrap_or(true);
                let ok_a = text[m.end()..].chars().next().map(after_ok).unwrap_or(true);
                if ok_b && ok_a {
                    out.push_str(&text[last..m.start()]);
                    out.push_str(&repl[m.as_str()]);
                    last = m.end();
                    pos = m.end();
                    n += 1;
                } else {
                    pos = m.start() + text[m.start()..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                }
                if pos >= text.len() {
                    break;
                }
            }
            if n > 0 {
                out.push_str(&text[last..]);
                ctx.backup(&self.root, &rel);
                let _ = fs::write(&p, out);
                total += n;
                push(&mut report["notes"], format!("{} chemin(s) cité(s) mis à jour dans {}", n, rel));
            }
        }
        report["mentions"] = json!(total);
    }

    fn update_graphs(&self, moves: &BTreeMap<String, String>, ctx: &mut Ctx, report: &mut Value) {
        let mut worlds: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for (o, n) in moves {
            if self.is_archive(o) || self.is_archive(n) {
                continue;
            }
            let (wo, wn) = (o.split('/').next().unwrap_or(""), n.split('/').next().unwrap_or(""));
            if wo == wn {
                worlds.entry(wo.to_string()).or_default().push((o.clone(), n.clone()));
            }
        }
        for (world, pairs) in worlds {
            let Some((mut data, raw)) = self.graph_load(&world) else { continue };
            let mut changed = false;
            for (o, n) in &pairs {
                let (wo, wn) = (o[world.len() + 1..].to_string(), n[world.len() + 1..].to_string());
                let pid_new = self.graph_parent(&data, &world, &dirname(n));
                let mut ids = vec![];
                for node in data["nodes"].as_array_mut().unwrap() {
                    if node["path"].as_str() == Some(wo.as_str()) {
                        node["path"] = json!(wn);
                        changed = true;
                        if let Some(id) = node["id"].as_str() {
                            ids.push(id.to_string());
                        }
                    }
                }
                if let (Some(pid), Some(links)) = (&pid_new, data.get_mut("links").and_then(|v| v.as_array_mut())) {
                    for l in links {
                        if l["rel"] == "contains" && ids.iter().any(|i| l["to"].as_str() == Some(i)) && l["from"].as_str() != Some(pid) {
                            l["from"] = json!(pid);
                        }
                    }
                }
                if let Some(homes) = data.get_mut("homes").and_then(|v| v.as_array_mut()) {
                    for h in homes {
                        if h["path"].as_str() == Some(wo.as_str()) {
                            h["path"] = json!(wn);
                            changed = true;
                        }
                    }
                }
            }
            if changed {
                self.graph_save(&world, &mut data, &raw, ctx, report);
            }
        }
    }

    fn update_indexes(&self, moves: &BTreeMap<String, String>, ctx: &mut Ctx, report: &mut Value) {
        let item_line = Regex::new(r"^\s*[-*]\s").unwrap();
        let desc_rx = Regex::new(r"\)\s*(?:—|-|:)\s*(.+)$").unwrap();
        let title_rx = Regex::new(r"(?m)^#\s+(.+)$").unwrap();
        for (old, new) in moves {
            if !is_note(new) || self.is_archive(new) {
                continue;
            }
            let (od, nd) = (dirname(old), dirname(new));
            let b = basename(new);
            if od == nd || b == "INDEX.md" || b == "README.md" {
                continue;
            }
            let dest_idx = format!("{}/INDEX.md", nd);
            if !self.root.join(&dest_idx).is_file() {
                continue;
            }
            let mut desc = String::new();
            let src_idx = format!("{}/INDEX.md", od);
            let sp = self.root.join(&src_idx);
            if let Ok(text) = fs::read_to_string(&sp) {
                let hn = href(&od, new, "", false);
                let lines: Vec<&str> = text.split('\n').collect();
                let mut kept = vec![];
                for l in &lines {
                    if item_line.is_match(l) && (l.contains(&format!("]({})", hn)) || l.contains(&format!("](<{}>)", hn))) {
                        if desc.is_empty() {
                            if let Some(c) = desc_rx.captures(l) {
                                desc = c[1].trim().to_string();
                            }
                        }
                        continue;
                    }
                    kept.push(*l);
                }
                if kept.len() != lines.len() {
                    ctx.backup(&self.root, &src_idx);
                    let _ = fs::write(&sp, kept.join("\n"));
                    push(&mut report["notes"], format!("retiré de {}", src_idx));
                }
            }
            let stem = match b.rfind('.') { Some(i) if i > 0 => b[..i].to_string(), _ => b.clone() };
            let title = fs::read_to_string(self.root.join(new)).ok()
                .and_then(|t| title_rx.captures(&t).map(|c| c[1].trim().to_string())).unwrap_or(stem);
            self.append_list_line(&dest_idx, new, &title, &desc, ctx, report);
        }
    }

    fn remove_empty_dirs(&self, moves: &BTreeMap<String, String>, report: &mut Value) {
        let mut dirs: Vec<String> = moves.keys().map(|o| dirname(o)).collect::<BTreeSet<_>>().into_iter().collect();
        dirs.sort_by_key(|d| std::cmp::Reverse(d.len()));
        for od in dirs {
            let mut d = od;
            while !d.is_empty() && d.matches('/').count() >= (if self.is_archive(&d) { 2 } else { 1 }) && self.root.join(&d).is_dir() {
                let left: Vec<String> = fs::read_dir(self.root.join(&d)).map(|it| it.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
                if left.iter().any(|e| e != ".DS_Store") {
                    break;
                }
                for e in &left {
                    let _ = fs::remove_file(self.root.join(&d).join(e));
                }
                if fs::remove_dir(self.root.join(&d)).is_err() {
                    break;
                }
                push(&mut report["removed_dirs"], d.clone());
                d = dirname(&d);
            }
        }
    }
}

fn push(v: &mut Value, s: String) {
    if !v.is_array() {
        *v = json!([]);
    }
    v.as_array_mut().unwrap().push(json!(s));
}

