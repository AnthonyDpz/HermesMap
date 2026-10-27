//! Analyse des dossiers : arborescence (carte), index complet (recherche), liens entre fichiers,
//! tags (notes + automatiques), catalogues et dossiers miroirs de l'archive.

use crate::config::Settings;
use crate::media::{icloud_real_name, is_dataless, kind_of_name, mtime, SECRET_NAMES};
use crate::util::{basename, dirname, join, norm_text, normpath, now, rel_of, unquote};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::Instant;

pub const MAX_INDEX: usize = 400_000;
const AUTO_TAGS: usize = 40;
const AUTO_TAG_LINKS: usize = 25;
const MAX_REFS: usize = 4000;
const MAX_REFS_PER_FILE: usize = 12;
const MAX_NODES: usize = 4000;
const MAX_CHILDREN: usize = 250;
pub const SCAN_TEXT_LIMIT: u64 = 400_000;
const MAX_TAG_LINKS: usize = 4000;

pub const DEFAULT_EXCLUDES: &[&str] = &[
    ".git", "node_modules", "__pycache__", ".venv", "venv", "site-packages", ".mypy_cache", ".pytest_cache",
    ".ruff_cache", ".DS_Store", ".obsidian", ".trash",
];

static STOPWORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    "le la les un une des du de d l et ou en au aux a pour par sur sous avec sans dans ce ces cet cette
mon ma mes ton ta tes son sa ses notre nos votre vos leur leurs qui que quoi dont est sont
the a an of to in on for with and or by from at as is are be this that it its my your
copy copie final new nouveau nouvelle old ancien untitled sans titre document doc docs file files
fichier fichiers dossier dossiers folder image images img photo photos picture pictures pic video videos vid
screenshot capture ecran scan scans export exported version draft brouillon test tmp temp misc divers
autre autres other others jpg jpeg png gif webp heic mp4 mov mkv webm pdf txt md json zip dsc dcim
skill skills script scripts run readme index main src lib assets asset data cache log logs backup sauvegarde tmp bin config conf settings default"
        .split_whitespace()
        .collect()
});

pub static WIKILINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[\[([^\]|#]+)(?:#[^\]|]*)?(?:\|[^\]]*)?\]\]").unwrap());
pub static MDLINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"\]\((<[^>\n]+>|[^)\s]+)(?:\s+"[^"]*")?\)"#).unwrap());
static PATHLIKE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:~/\.hermes/|\./|\.\./)?(?:[\w.\-]+/)+[\w.\-]+\.\w{1,6}").unwrap());
static HASHTAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"#([A-Za-zÀ-ÿ][\w\-/À-ÿ]{1,40})").unwrap());
static FENCE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)```.*?```").unwrap());
static FM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)\A---\s*\n(.*?)\n---").unwrap());
static TAGLINE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(tags?|keywords)\s*:\s*(.*)$").unwrap());

/// Tags du front-matter YAML (tags: / tag:) et #hashtags dans le texte Markdown.
pub fn extract_tags(txt: &str, is_md: bool) -> BTreeSet<String> {
    let mut tags: Vec<String> = vec![];
    let fm = FM_RE.captures(txt);
    if let Some(m) = &fm {
        let lines: Vec<&str> = m[1].lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let Some(mm) = TAGLINE_RE.captures(line.trim()) else { continue };
            let val = mm[2].trim();
            if !val.is_empty() {
                for t in Regex::new(r"[,\s]+").unwrap().split(val.trim_matches(|c| c == '[' || c == ']')) {
                    let t = t.trim().trim_matches(|c| c == '"' || c == '\'' || c == '#');
                    if !t.is_empty() {
                        tags.push(t.to_string());
                    }
                }
            } else {
                for nxt in &lines[i + 1..] {
                    let li = Regex::new(r"^\s*-\s*(.+)$").unwrap();
                    match li.captures(nxt) {
                        Some(c) => tags.push(c[1].trim().trim_matches(|c| c == '"' || c == '\'' || c == '#').to_string()),
                        None => break,
                    }
                }
            }
        }
    }
    if is_md {
        let body = match &fm {
            Some(m) => &txt[m.get(0).unwrap().end()..],
            None => txt,
        };
        let body = FENCE_RE.replace_all(body, "");
        let body = Regex::new(r"`[^`]*`").unwrap().replace_all(&body, "");
        for c in HASHTAG_RE.captures_iter(&body) {
            let start = c.get(0).unwrap().start();
            if let Some(prev) = body[..start].chars().last() {
                if prev.is_alphanumeric() || prev == '_' || "&#/=:".contains(prev) {
                    continue;
                }
            }
            let h = &c[1];
            if Regex::new(r"^[0-9a-fA-F]{3,8}$").unwrap().is_match(h) {
                continue; // couleurs hexadécimales
            }
            tags.push(h.to_string());
        }
    }
    tags.into_iter()
        .map(|t| t.to_lowercase().trim_end_matches('/').to_string())
        .filter(|t| { let n = t.chars().count(); n > 1 && n <= 40 })
        .collect()
}

/// Mots significatifs d'un nom de fichier ou de dossier (pour les tags automatiques).
pub fn name_tokens(name: &str) -> HashSet<String> {
    // (comme Python : un point après le 1er caractère ; le découpage se fait par caractères, pas par octets)
    let stem = if name.chars().skip(1).any(|c| c == '.') { &name[..name.rfind('.').unwrap()] } else { name };
    let stem = Regex::new(r"([a-zà-ÿ])([A-Z])").unwrap().replace_all(stem, "$1 $2");
    let mut out = HashSet::new();
    for w in Regex::new(r"[^0-9A-Za-zÀ-ÿ]+").unwrap().split(&stem) {
        let w = norm_text(w);
        let n = w.chars().count();
        if n < 3 || n > 30 || STOPWORDS.contains(w.as_str()) || w.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        out.insert(w);
    }
    out
}

/// Cible d'un lien Markdown, relative à la racine : gère <chemin avec espaces>, %20 et les chemins absolus.
pub fn href_target(root_prefix: &str, parent: &str, raw: &str) -> Option<String> {
    let h = if raw.starts_with('<') && raw.ends_with('>') { raw[1..raw.len() - 1].to_string() } else { unquote(raw) };
    if ["http:", "https:", "mailto:", "#", "data:", "file:"].iter().any(|p| h.starts_with(p)) {
        return None;
    }
    let h = h.split('#').next().unwrap_or("").to_string();
    if h.is_empty() {
        return None;
    }
    if h.starts_with('/') {
        if !root_prefix.is_empty() && h.starts_with(&format!("{}/", root_prefix)) {
            return Some(normpath(&h[root_prefix.len() + 1..]));
        }
        return None;
    }
    let j = if parent.is_empty() { normpath(&h) } else { normpath(&join(parent, &h)) };
    Some(if j == "." { String::new() } else { j.trim_end_matches('/').to_string() })
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub rel: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: f64,
    pub cloud: bool,
}

#[derive(Default)]
pub struct ScanData {
    pub index: Option<Vec<Entry>>,
    pub index_paths: HashSet<String>,
    pub index_dirs: HashSet<String>,
    pub name_map: HashMap<String, Vec<String>>,
    pub tag_index: HashMap<String, Vec<String>>,
}

pub struct Scanner {
    pub root: PathBuf,
    pub cfg: Settings,
    excludes: HashSet<String>,
    show_hidden: bool,
    pub data: RwLock<ScanData>,
    cat_cache: Mutex<Option<(Vec<(String, f64)>, HashMap<String, Map<String, Value>>)>>,
}

struct DirEnt {
    path: PathBuf,
    name: String,
    is_dir: bool,
    md: fs::Metadata,
}

impl Scanner {
    pub fn new(root: PathBuf, all: bool, show_hidden: bool, cfg: Settings) -> Self {
        let mut excludes: HashSet<String> =
            if all { HashSet::new() } else { DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).collect() };
        excludes.extend(cfg.ignore());
        Scanner { root, cfg, excludes, show_hidden, data: RwLock::new(ScanData::default()), cat_cache: Mutex::new(None) }
    }

    pub fn root_prefix(&self) -> String {
        self.root.to_string_lossy().to_string()
    }

    pub fn top_allowed(&self, name: &str) -> bool {
        self.cfg.top_allowed(name)
    }

    pub fn allowed(&self, p: &Path) -> bool {
        let rel = rel_of(&self.root, p);
        if rel.is_empty() {
            return true;
        }
        self.top_allowed(rel.split('/').next().unwrap_or(""))
    }

    pub fn skip(&self, name: &str, depth: usize) -> bool {
        if self.excludes.contains(name) {
            return true;
        }
        if depth == 1 && !self.top_allowed(name) {
            return true;
        }
        if icloud_real_name(name).is_some() {
            return false;
        }
        !self.show_hidden && name.starts_with('.') && !SECRET_NAMES.contains(&name) && !name.starts_with(".env")
    }

    pub fn rel(&self, p: &Path) -> String {
        rel_of(&self.root, p)
    }

    fn list_dir(&self, d: &Path, depth: usize) -> Vec<DirEnt> {
        let mut v: Vec<DirEnt> = match fs::read_dir(d) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let ft = e.file_type().ok()?;
                    if ft.is_symlink() {
                        return None;
                    }
                    let name = e.file_name().to_string_lossy().to_string();
                    if self.skip(&name, depth) {
                        return None;
                    }
                    let md = e.metadata().ok()?;
                    Some(DirEnt { path: e.path(), name, is_dir: ft.is_dir(), md })
                })
                .collect(),
            Err(_) => vec![],
        };
        v.sort_by(|a, b| (!a.is_dir, a.name.to_lowercase()).cmp(&(!b.is_dir, b.name.to_lowercase())));
        v
    }


    /// Dossier « miroir » dans l'archive d'un dossier de notes : Photos/japon -> Archive/photos/japon.
    pub fn mirror_of(&self, rel: &str) -> Option<String> {
        let rel = rel.trim_matches('/');
        let arch = self.cfg.archive()?;
        if rel.is_empty() {
            return None;
        }
        let (top, rest) = match rel.find('/') {
            Some(i) => (&rel[..i], &rel[i + 1..]),
            None => (rel, ""),
        };
        let data = self.data.read().ok()?;
        if top == arch || !data.index_dirs.contains(&arch) {
            return None;
        }
        let area = self.cfg.zone_name(top);
        let mut cands = vec![
            format!("{}/{}/{}", arch, area.to_lowercase(), rest).trim_end_matches('/').to_string(),
            format!("{}/{}/{}", arch, area, rest).trim_end_matches('/').to_string(),
        ];
        if !rest.is_empty() {
            cands.push(format!("{}/{}", arch, rest));
        }
        cands.into_iter().find(|c| data.index_dirs.contains(c))
    }

    /// Inverse : Archive/photos/japon -> Photos/japon (s'il existe).
    pub fn brain_of(&self, rel: &str) -> Option<String> {
        let parts: Vec<&str> = rel.trim_matches('/').split('/').collect();
        let arch = self.cfg.archive()?;
        if parts.len() < 2 || parts[0] != arch {
            return None;
        }
        let area = parts[1].to_lowercase();
        let rest = &parts[2..];
        let data = self.data.read().ok()?;
        for d in &data.index_dirs {
            if !d.contains('/') && *d != arch && self.cfg.zone_name(d).to_lowercase() == area {
                let mut v = vec![d.as_str()];
                v.extend_from_slice(rest);
                let cand = v.join("/");
                return if data.index_dirs.contains(&cand) {
                    Some(cand)
                } else if rest.is_empty() {
                    Some(d.clone())
                } else {
                    None
                };
            }
        }
        None
    }

    /// Fiches des catalog.yaml / CATALOG.jsonl (descriptions des médias), indexées par chemin depuis la racine.
    pub fn catalog(&self) -> HashMap<String, Map<String, Value>> {
        let cats: Vec<String> = {
            let d = self.data.read().unwrap();
            let mut v: Vec<String> = d
                .index_paths
                .iter()
                .filter(|r| {
                    let b = basename(r).to_lowercase();
                    b == "catalog.yaml" || b == "catalog.yml" || b == "catalog.jsonl"
                })
                .cloned()
                .collect();
            v.sort();
            v
        };
        let sig: Vec<(String, f64)> =
            cats.iter().map(|c| (c.clone(), fs::metadata(self.root.join(c)).map(|m| mtime(&m)).unwrap_or(0.0))).collect();
        if let Some((s, c)) = self.cat_cache.lock().unwrap().as_ref() {
            if *s == sig {
                return c.clone();
            }
        }
        let mut out: HashMap<String, Map<String, Value>> = HashMap::new();
        let ws = Regex::new(r"\s+").unwrap();
        for c in &cats {
            let base = dirname(c);
            let txt = match fs::read(self.root.join(c)) {
                Ok(b) => String::from_utf8_lossy(&b).to_string(),
                Err(_) => continue,
            };
            if c.to_lowercase().ends_with(".jsonl") {
                for line in txt.lines() {
                    let e: Value = match serde_json::from_str(line) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let Some(o) = e.as_object() else { continue };
                    let Some(r) = o.get("rel").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else { continue };
                    let target = normpath(&join(&base, r));
                    let mut d = Map::new();
                    for k in ["title", "series", "silo", "pages", "url", "source"] {
                        if let Some(v) = o.get(k) {
                            if !v.is_null() && v != "" {
                                d.insert(k.into(), v.clone());
                            }
                        }
                    }
                    let desc = ["description", "note", "snippet"].iter().find_map(|k| {
                        o.get(*k).filter(|v| !v.is_null() && *v != "" && *v != false).map(|v| match v {
                            Value::String(s) => s.clone(),
                            x => x.to_string(),
                        })
                    });
                    if let Some(desc) = desc {
                        let s: String = ws.replace_all(&desc, " ").trim().chars().take(600).collect();
                        d.insert("description".into(), json!(s));
                    }
                    if !d.is_empty() {
                        d.insert("catalog".into(), json!(c));
                        let ent = out.entry(target).or_default();
                        for (k, v) in d {
                            ent.entry(k).or_insert(v);
                        }
                    }
                }
                continue;
            }
            for e in parse_catalog(&txt) {
                let g = |k: &str| e.get(k).and_then(|v| match v {
                    Value::String(s) if !s.is_empty() => Some(s.clone()),
                    Value::Bool(b) => Some(b.to_string()),
                    _ => None,
                });
                let target = if let (Some(r), Some(p)) = (g("root"), g("path")) {
                    Some(normpath(&format!("{}/{}", r, p)))
                } else if let Some(l) = g("link") {
                    Some(normpath(&join(&base, &l)))
                } else {
                    g("path").map(|p| normpath(&join(&base, &p)))
                };
                if let Some(t) = target {
                    let ent = out.entry(t).or_default();
                    for k in ["description", "series", "character", "artist", "kind"] {
                        if let Some(v) = e.get(k) {
                            if !v.is_null() && v != "" {
                                ent.insert(k.into(), v.clone());
                            }
                        }
                    }
                    ent.insert("catalog".into(), json!(c));
                }
            }
        }
        *self.cat_cache.lock().unwrap() = Some((sig, out.clone()));
        out
    }

    /// Phase 1 : arborescence (rapide, publiée tout de suite). Phase 2 : index, liens et tags.
    pub fn scan(&self, publish: &dyn Fn(Value), progress: &dyn Fn(String)) {
        let t0 = Instant::now();
        let root_id = "/".to_string();
        let title = self.cfg.title();
        let mut nodes: Vec<Map<String, Value>> = vec![obj(json!({"id": root_id, "name": title, "kind": "root",
                                                               "path": "", "group": "root", "depth": 0}))];
        let mut links: Vec<Value> = vec![];
        let mut text_files: Vec<(PathBuf, String)> = vec![];
        let mut skill_names: HashMap<String, String> = HashMap::new();
        let mut truncated = false;
        let mut count = 0usize;

        struct Res {
            nodes: Vec<Map<String, Value>>,
            links: Vec<Value>,
            files: Vec<(PathBuf, String)>,
            skills: HashMap<String, String>,
            count: usize,
            truncated: bool,
        }
        let bfs = |top: &DirEnt, budget: usize| -> Res {
            let mut r = Res { nodes: vec![], links: vec![], files: vec![], skills: HashMap::new(), count: 0, truncated: false };
            // (dossier, id du parent, profondeur, entrées déjà lues)
            let mut queue: Vec<(Option<PathBuf>, String, usize, Option<Vec<DirEnt>>)> = vec![(
                None,
                root_id.clone(),
                1,
                Some(vec![DirEnt { path: top.path.clone(), name: top.name.clone(), is_dir: top.is_dir, md: top.md.clone() }]),
            )];
            while !queue.is_empty() {
                let mut next = vec![];
                for (d, parent_id, depth, entries) in queue.drain(..) {
                    let entries = match entries {
                        Some(e) => e,
                        None => self.list_dir(d.as_ref().unwrap(), depth),
                    };
                    let (mut shown, mut hidden_extra) = (0usize, 0usize);
                    for de in entries {
                        if shown >= MAX_CHILDREN || r.count >= budget {
                            hidden_extra += 1;
                            continue;
                        }
                        let rel = self.rel(&de.path);
                        let g = rel.split('/').next().unwrap_or("").to_string();
                        let k = kind_of_name(&de.name, de.is_dir);
                        let cloud = is_dataless(&de.md) && !de.is_dir;
                        let mut node = obj(json!({"id": rel, "name": icloud_real_name(&de.name).unwrap_or(de.name.clone()),
                            "kind": k, "path": rel, "group": g, "depth": depth,
                            "size": if de.is_dir { Value::Null } else { json!(de.md.len()) }, "mtime": mtime(&de.md)}));
                        if cloud {
                            node.insert("cloud".into(), json!(true));
                        }
                        if de.is_dir && de.path.join("SKILL.md").is_file() {
                            node.insert("skill".into(), json!(true));
                            r.skills.insert(de.name.to_lowercase(), rel.clone());
                        }
                        r.nodes.push(node);
                        r.links.push(json!({"source": parent_id, "target": rel, "type": "tree"}));
                        r.count += 1;
                        shown += 1;
                        if de.is_dir {
                            next.push((Some(de.path.clone()), rel, depth + 1, None));
                        } else if ["markdown", "text", "config", "json"].contains(&k.as_str())
                            && !cloud
                            && de.md.len() <= SCAN_TEXT_LIMIT
                        {
                            r.files.push((de.path.clone(), rel));
                        }
                    }
                    if hidden_extra > 0 {
                        if let Some(dp) = &d {
                            r.truncated = true;
                            let drel = self.rel(dp);
                            let more_id = format!("{}/…more", drel);
                            r.nodes.push(obj(json!({"id": more_id, "name": format!("+{} autres", hidden_extra),
                                "kind": "more", "path": drel, "group": drel.split('/').next().unwrap_or(""), "depth": depth})));
                            r.links.push(json!({"source": parent_id, "target": more_id, "type": "tree"}));
                        }
                    }
                }
                queue = next;
            }
            r
        };

        let tops = self.list_dir(&self.root, 1);
        // partage équitable du quota entre dossiers de 1er niveau ; la part inutilisée est redistribuée
        let mut results: HashMap<String, Res> = HashMap::new();
        let mut pending: Vec<usize> = (0..tops.len()).collect();
        let mut remaining = MAX_NODES as i64;
        while !pending.is_empty() {
            let share = std::cmp::max(50, remaining / pending.len() as i64) as usize;
            let mut still = vec![];
            for &i in &pending {
                let de = &tops[i];
                progress(format!("Lecture de {}…", de.name));
                let r = bfs(de, share);
                let (tr, c) = (r.truncated, r.count);
                results.insert(de.name.clone(), r);
                if tr && c >= share {
                    still.push(i);
                } else {
                    remaining -= c as i64;
                }
            }
            if still.len() == pending.len() || still.is_empty() {
                break;
            }
            pending = still;
        }
        for de in &tops {
            if let Some(r) = results.remove(&de.name) {
                nodes.extend(r.nodes);
                links.extend(r.links);
                text_files.extend(r.files);
                skill_names.extend(r.skills);
                truncated |= r.truncated;
                count += r.count;
            }
        }
        let mut files_by_name: HashMap<String, Vec<String>> = HashMap::new();
        for nd in &nodes {
            let k = nd["kind"].as_str().unwrap_or("");
            if !["dir", "more", "root"].contains(&k) {
                let name = nd["name"].as_str().unwrap_or("").to_lowercase();
                let id = nd["id"].as_str().unwrap_or("").to_string();
                let stem = match name.rfind('.') { Some(i) if i > 0 => name[..i].to_string(), _ => name.clone() };
                files_by_name.entry(name).or_default().push(id.clone());
                files_by_name.entry(stem).or_default().push(id);
            }
        }
        let base = json!({"root": self.root.to_string_lossy(), "title": title, "truncated": truncated, "refs_done": false});
        let mk = |nodes: &Vec<Map<String, Value>>, links: &Vec<Value>, refs_done: bool| {
            let mut r = base.clone();
            r["nodes"] = Value::Array(nodes.iter().cloned().map(Value::Object).collect());
            r["links"] = Value::Array(links.clone());
            r["scanned_at"] = json!(now());
            r["refs_done"] = json!(refs_done);
            r
        };
        publish(mk(&nodes, &links, false));
        println!("  Arborescence : {} éléments en {:.1} s", count, t0.elapsed().as_secs_f64());

        // ---- index complet (tous les fichiers, toutes profondeurs) pour la recherche
        let t_idx = Instant::now();
        let mut index: Vec<Entry> = vec![];
        let mut stack: Vec<(PathBuf, usize)> = tops.iter().filter(|d| d.is_dir).map(|d| (d.path.clone(), 2)).collect();
        for de in &tops {
            index.push(Entry { rel: self.rel(&de.path), name: de.name.clone(), is_dir: de.is_dir, size: de.md.len(),
                               mtime: mtime(&de.md), cloud: false });
        }
        while let Some((d, depth)) = stack.pop() {
            if index.len() >= MAX_INDEX || t_idx.elapsed().as_secs() > 120 {
                break;
            }
            let rd = match fs::read_dir(&d) {
                Ok(r) => r,
                Err(_) => continue,
            };
            for e in rd.flatten() {
                let Ok(ft) = e.file_type() else { continue };
                let name = e.file_name().to_string_lossy().to_string();
                if ft.is_symlink() || self.skip(&name, depth) {
                    continue;
                }
                let Ok(md) = e.metadata() else { continue };
                let is_dir = ft.is_dir();
                let icl = icloud_real_name(&name);
                index.push(Entry {
                    rel: self.rel(&e.path()),
                    name: icl.clone().unwrap_or(name),
                    is_dir,
                    size: if is_dir { 0 } else { md.len() },
                    mtime: mtime(&md),
                    cloud: (is_dataless(&md) || icl.is_some()) && !is_dir,
                });
                if is_dir {
                    stack.push((e.path(), depth + 1));
                }
            }
            if index.len() % 2000 < 50 {
                progress(format!("Indexation pour la recherche : {} éléments…", index.len()));
            }
        }
        {
            let mut w = self.data.write().unwrap();
            w.index_paths = index.iter().map(|e| e.rel.clone()).collect();
            w.index_dirs = index.iter().filter(|e| e.is_dir).map(|e| e.rel.clone()).collect();
            let mut nm: HashMap<String, Vec<String>> = HashMap::new();
            for e in index.iter().filter(|e| !e.is_dir) {
                let low = e.name.to_lowercase();
                let stem = match low.rfind('.') { Some(i) => low[..i].to_string(), None => low.clone() };
                nm.entry(low).or_default().push(e.rel.clone());
                nm.entry(stem).or_default().push(e.rel.clone());
            }
            w.name_map = nm;
            w.index = Some(index.clone());
        }
        println!("  Index de recherche : {} éléments en {:.1} s", index.len(), t_idx.elapsed().as_secs_f64());

        // ---- liens entre fichiers
        let ids: HashSet<String> = nodes.iter().map(|n| n["id"].as_str().unwrap_or("").to_string()).collect();
        let mut refs: Vec<Value> = vec![];
        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut per_src: HashMap<String, usize> = HashMap::new();
        let mut add_ref = |refs: &mut Vec<Value>, src: &str, dst: Option<&str>, why: &str| {
            let Some(dst) = dst else { return };
            if dst.is_empty() || dst == src || !ids.contains(dst) {
                return;
            }
            if refs.len() >= MAX_REFS || per_src.get(src).copied().unwrap_or(0) >= MAX_REFS_PER_FILE {
                return;
            }
            let (a, b) = (src.to_string(), dst.to_string());
            if seen.contains(&(a.clone(), b.clone())) || seen.contains(&(b.clone(), a.clone())) {
                return;
            }
            seen.insert((a.clone(), b.clone()));
            *per_src.entry(a).or_insert(0) += 1;
            refs.push(json!({"source": src, "target": dst, "type": "ref", "why": why}));
        };
        let skill_list: Vec<String> = {
            let mut v: Vec<String> = skill_names.keys().filter(|s| s.chars().count() >= 4).map(|s| regex::escape(s)).collect();
            v.sort_by(|a, b| b.len().cmp(&a.len()));
            v
        };
        let skill_re = if skill_list.is_empty() { None } else { Regex::new(&format!("(?i)({})", skill_list.join("|"))).ok() };

        // alterner entre dossiers de 1er niveau (et les .md d'abord) pour que chaque zone ait ses liens
        text_files.sort_by(|a, b| {
            let ka = (!a.1.to_lowercase().ends_with(".md"), a.1.matches('/').count());
            let kb = (!b.1.to_lowercase().ends_with(".md"), b.1.matches('/').count());
            ka.cmp(&kb)
        });
        let mut by_group: Vec<(String, Vec<(PathBuf, String)>)> = vec![];
        for (f, rel) in text_files {
            let g = rel.split('/').next().unwrap_or("").to_string();
            match by_group.iter_mut().find(|(k, _)| *k == g) {
                Some((_, v)) => v.push((f, rel)),
                None => by_group.push((g, vec![(f, rel)])),
            }
        }
        let maxlen = by_group.iter().map(|(_, v)| v.len()).max().unwrap_or(0);
        let mut ordered = vec![];
        for i in 0..maxlen {
            for (_, l) in &by_group {
                if i < l.len() {
                    ordered.push(l[i].clone());
                }
            }
        }

        let root_prefix = self.root_prefix();
        let mut tag_index: HashMap<String, Vec<String>> = HashMap::new();
        let mut tag_order: Vec<String> = vec![];
        let (budget_bytes, mut read_bytes) = (80_000_000usize, 0usize);
        let t1 = Instant::now();
        let total = ordered.len();
        for (i, (f, src)) in ordered.iter().enumerate() {
            if read_bytes > budget_bytes || t1.elapsed().as_secs() > 90 {
                println!("  (analyse des liens arrêtée : limite de temps/volume atteinte)");
                break;
            }
            if i % 100 == 0 {
                progress(format!("Recherche des liens : {}/{} fichiers…", i, total));
            }
            let mut raw = vec![];
            match fs::File::open(f) {
                Ok(fh) => {
                    let _ = fh.take(SCAN_TEXT_LIMIT).read_to_end(&mut raw);
                }
                Err(_) => continue,
            }
            read_bytes += raw.len();
            if raw[..raw.len().min(4096)].contains(&0) {
                continue;
            }
            let txt = String::from_utf8_lossy(&raw).to_string();
            let low = src.to_lowercase();
            for t in extract_tags(&txt, low.ends_with(".md") || low.ends_with(".markdown")) {
                if !tag_index.contains_key(&t) {
                    tag_order.push(t.clone());
                }
                tag_index.entry(t).or_default().push(src.clone());
            }
            let parent = dirname(src);
            let parts: Vec<&str> = src.split('/').collect();
            let own_skill = parts.iter().rev().map(|p| p.to_lowercase()).find(|p| skill_names.contains_key(p));
            for m in WIKILINK_RE.captures_iter(&txt) {
                let target = m[1].trim().to_lowercase();
                let mut cands: Vec<String> = files_by_name.get(&target).map(|v| v.iter().take(5).cloned().collect()).unwrap_or_default();
                cands.extend(files_by_name.get(&format!("{}.md", target)).map(|v| v.iter().take(5).cloned().collect::<Vec<_>>()).unwrap_or_default());
                for c in cands {
                    add_ref(&mut refs, src, Some(&c), "wikilink");
                }
                if let Some(sk) = skill_names.get(&target) {
                    add_ref(&mut refs, src, Some(sk), "wikilink");
                }
            }
            for m in MDLINK_RE.captures_iter(&txt) {
                let t = href_target(&root_prefix, &parent, &m[1]);
                add_ref(&mut refs, src, t.as_deref(), "lien");
            }
            for m in PATHLIKE_RE.find_iter(&txt) {
                let ptxt = m.as_str().replace("~/.hermes/", "");
                for cand in [normpath(&ptxt), normpath(&join(&parent, &ptxt))] {
                    let cand = if cand == "." { String::new() } else { cand };
                    if ids.contains(&cand) {
                        add_ref(&mut refs, src, Some(&cand), "chemin");
                        break;
                    }
                }
            }
            if let Some(re) = &skill_re {
                for m in re.find_iter(&txt) {
                    let (s, e) = (m.start(), m.end());
                    let before = txt[..s].chars().last();
                    let after = txt[e..].chars().next();
                    let bad = |c: Option<char>| c.map(|c| c.is_alphanumeric() || c == '_' || c == '-').unwrap_or(false);
                    if bad(before) || bad(after) {
                        continue;
                    }
                    let sname = m.as_str().to_lowercase();
                    if Some(&sname) != own_skill.as_ref() {
                        if let Some(sk) = skill_names.get(&sname) {
                            add_ref(&mut refs, src, Some(sk), "mention de skill");
                        }
                    }
                }
            }
        }

        // ---- liens « miroir » : dossier de notes <-> son dossier dans l'archive
        if let Some(arch) = self.cfg.archive() {
            for n in &nodes {
                let id = n["id"].as_str().unwrap_or("");
                if n["kind"] == "dir" && id.split('/').next() != Some(arch.as_str()) {
                    if let Some(m) = self.mirror_of(id) {
                        add_ref(&mut refs, id, Some(&m), "miroir");
                    }
                }
            }
        }

        // ---- tags automatiques : mots qui reviennent dans les noms de plusieurs branches
        progress("Calcul des tags automatiques…".into());
        let mut word_items: HashMap<String, Vec<(String, bool)>> = HashMap::new();
        let mut word_dirs: HashMap<String, HashSet<String>> = HashMap::new();
        let mut word_zones: HashMap<String, BTreeSet<String>> = HashMap::new();
        let zone_names: HashSet<String> =
            tops.iter().flat_map(|d| [norm_text(&d.name), norm_text(&self.cfg.zone_name(&d.name))]).collect();
        for e in &index {
            let parent = dirname(&e.rel);
            let zone = e.rel.split('/').next().unwrap_or("").to_string();
            for w in name_tokens(&e.name) {
                if zone_names.contains(&w) {
                    continue;
                }
                word_items.entry(w.clone()).or_default().push((e.rel.clone(), e.is_dir));
                word_dirs.entry(w.clone()).or_default().insert(parent.clone());
                word_zones.entry(w).or_default().insert(zone.clone());
            }
        }
        let mut cands: Vec<(f64, String)> = vec![];
        for (w, items) in &word_items {
            let nd = word_dirs[w].len();
            if items.len() < 3 || nd < 2 {
                continue;
            }
            if items.len() > std::cmp::max(400, index.len() / 8) {
                continue; // mot trop banal (présent partout)
            }
            let score = word_zones[w].len() as f64 * 1000.0 + nd as f64 * 10.0 + (items.len().min(200) as f64) / 100.0;
            cands.push((score, w.clone()));
        }
        cands.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let mut auto_index: Vec<(String, Vec<String>)> = vec![];
        for (_, w) in cands.into_iter().take(AUTO_TAGS) {
            if tag_index.contains_key(&w) {
                continue;
            }
            let mut items = word_items[&w].clone();
            items.sort_by(|a, b| (!a.1, a.0.matches('/').count(), &a.0).cmp(&(!b.1, b.0.matches('/').count(), &b.0)));
            auto_index.push((w, items.into_iter().take(2000).map(|x| x.0).collect()));
        }

        // nœuds « tag » reliés aux éléments qui les portent
        let mut tag_nodes: Vec<Map<String, Value>> = vec![];
        let mut tag_links: Vec<Value> = vec![];
        let pos: HashMap<String, usize> =
            nodes.iter().enumerate().map(|(i, n)| (n["id"].as_str().unwrap_or("").to_string(), i)).collect();
        let mut tags_sorted: Vec<&String> = tag_order.iter().collect();
        tags_sorted.sort_by(|a, b| tag_index[*b].len().cmp(&tag_index[*a].len())); // tri stable : ordre d'apparition gardé
        for t in tags_sorted {
            let files = &tag_index[t];
            let tid = format!("#tag:{}", t);
            tag_nodes.push(obj(json!({"id": tid, "name": format!("#{}", t), "kind": "tag", "path": tid, "group": "#tags",
                                      "depth": 1, "count": files.len(), "auto": false})));
            for f in files {
                if let Some(&i) = pos.get(f) {
                    push_tag(&mut nodes[i], t);
                }
                if tag_links.len() < MAX_TAG_LINKS {
                    tag_links.push(json!({"source": tid, "target": f, "type": "tag"}));
                }
            }
        }
        for (t, files) in &auto_index {
            let tid = format!("#tag:{}", t);
            tag_nodes.push(obj(json!({"id": tid, "name": t, "kind": "tag", "path": tid, "group": "#tags", "depth": 1,
                                      "count": files.len(), "auto": true,
                                      "zones": word_zones[t].iter().collect::<Vec<_>>()})));
            let mut shown = 0;
            for f in files {
                let Some(&i) = pos.get(f) else { continue };
                push_tag(&mut nodes[i], t);
                if shown < AUTO_TAG_LINKS {
                    tag_links.push(json!({"source": tid, "target": f, "type": "tag"}));
                    shown += 1;
                }
            }
            if shown == 0 {
                // aucun élément visible sur la carte : relier au dossier visible le plus proche
                let mut seen_p = HashSet::new();
                for f in files {
                    let mut p = f.clone();
                    while !p.is_empty() && !pos.contains_key(&p) {
                        p = dirname(&p);
                    }
                    if !p.is_empty() && seen_p.insert(p.clone()) {
                        tag_links.push(json!({"source": tid, "target": p, "type": "tag"}));
                        if seen_p.len() >= AUTO_TAG_LINKS {
                            break;
                        }
                    }
                }
            }
        }
        println!("  Tags : {} dans les notes, {} automatiques", tag_index.len(), auto_index.len());
        {
            let mut w = self.data.write().unwrap();
            let mut ti = tag_index.clone();
            for (t, f) in &auto_index {
                ti.insert(t.clone(), f.clone());
            }
            w.tag_index = ti;
        }
        let all_nodes: Vec<Map<String, Value>> = nodes.into_iter().chain(tag_nodes).collect();
        let all_links: Vec<Value> = links.into_iter().chain(refs.iter().cloned()).chain(tag_links).collect();
        publish(mk(&all_nodes, &all_links, true));
        println!("  Liens : {} trouvés en {:.1} s", refs.len(), t1.elapsed().as_secs_f64());
    }
}

fn push_tag(n: &mut Map<String, Value>, t: &str) {
    let e = n.entry("tags").or_insert_with(|| json!([]));
    if let Some(a) = e.as_array_mut() {
        a.push(json!(t));
    }
}

pub fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// Lit un catalog.yaml (liste « entries: » de fiches) avec un lecteur simple.
pub fn parse_catalog(txt: &str) -> Vec<Map<String, Value>> {
    let mut entries: Vec<Map<String, Value>> = vec![];
    let item = Regex::new(r"^(\s*)-\s+([\w.-]+)\s*:\s*(.*)$").unwrap();
    let field = Regex::new(r"^(\s+)([\w.-]+)\s*:\s*(.*)$").unwrap();
    let mut key = String::new();
    let mut block: Option<Vec<String>> = None;
    let mut block_ind = 0usize;
    for raw in txt.lines() {
        if raw.trim_start().starts_with('#') && block.is_none() {
            continue;
        }
        if let Some(b) = block.as_mut() {
            let ind = raw.len() - raw.trim_start().len();
            if raw.trim().is_empty() || ind > block_ind {
                b.push(raw.trim().to_string());
                continue;
            }
            let joined = b.iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" ");
            if let Some(cur) = entries.last_mut() {
                cur.insert(key.clone(), json!(joined.trim()));
            }
            block = None;
        }
        let (ind, k, val) = if let Some(m) = item.captures(raw) {
            entries.push(Map::new());
            (m[1].len() + 2, m[2].to_string(), m[3].to_string())
        } else if let Some(m) = field.captures(raw) {
            if entries.is_empty() {
                continue;
            }
            (m[1].len(), m[2].to_string(), m[3].to_string())
        } else {
            continue;
        };
        key = k;
        let val = val.trim();
        if [">", ">-", "|", "|-", ">+", "|+"].contains(&val) {
            block = Some(vec![]);
            block_ind = ind;
            continue;
        }
        let v = if val == "null" || val == "~" || val.is_empty() {
            Value::Null
        } else if val.len() >= 2
            && val.chars().next() == val.chars().last()
            && (val.starts_with('"') || val.starts_with('\''))
        {
            json!(&val[1..val.len() - 1])
        } else if val == "true" || val == "false" {
            json!(val == "true")
        } else {
            json!(val)
        };
        if let Some(cur) = entries.last_mut() {
            cur.insert(key.clone(), v);
        }
    }
    if let (Some(b), Some(cur)) = (block, entries.last_mut()) {
        let joined = b.iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" ");
        cur.insert(key, json!(joined.trim()));
    }
    entries
}
