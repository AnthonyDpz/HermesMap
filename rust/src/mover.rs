//! Déplacement / renommage manuel de fichiers, avec mise à jour des index :
//! fiche du catalog.yaml, ligne du CATALOG.jsonl, journal LOG.md de l'archive, liens des notes .md.
//! Tout est sauvegardé avant et peut être annulé.

use crate::app::App;
use crate::config;
use crate::scan::Scanner;
use crate::util::{basename, dirname, join, normpath, now, unquote};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

static MDLINK: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(!?\[[^\]]*\]\()(<[^>\n]+>|[^)\s]+)((?:\s+"[^"]*")?\))"#).unwrap());

pub fn journal_dir() -> PathBuf {
    config::data_dir().join("moves")
}

pub fn sha_file(p: &Path) -> Option<String> {
    fs::read(p).ok().map(|b| {
        let mut h = sha1_smol::Sha1::new();
        h.update(&b);
        h.digest().to_string()
    })
}

fn kind(name: &str) -> &'static str {
    let ext = crate::media::suffix(&basename(name).to_lowercase());
    let e = ext.as_str();
    if ["jpg", "jpeg", "png", "gif", "webp", "heic", "bmp", "tif", "tiff", "avif"].contains(&e) {
        "image"
    } else if ["mp4", "mov", "m4v", "webm", "avi", "mkv", "wmv", "mpg", "mpeg"].contains(&e) {
        "video"
    } else if e == "pdf" {
        "pdf"
    } else if ["doc", "docx", "xls", "xlsx", "ppt", "pptx", "pages", "numbers", "key", "odt"].contains(&e) {
        "office"
    } else if ["txt", "md", "csv", "json", "yaml", "yml"].contains(&e) {
        "text"
    } else {
        "file"
    }
}

fn splitext(p: &str) -> (String, String) {
    let b = basename(p);
    match b.rfind('.') {
        Some(i) if i > 0 => (p[..p.len() - (b.len() - i)].to_string(), b[i..].to_string()),
        _ => (p.to_string(), String::new()),
    }
}

/// posixpath.relpath
pub fn relpath(target: &str, start: &str) -> String {
    let tn = normpath(target);
    let sn = normpath(if start.is_empty() { "." } else { start });
    let t: Vec<&str> = tn.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    let s: Vec<&str> = sn.split('/').filter(|x| !x.is_empty() && *x != ".").collect();
    let mut i = 0;
    while i < t.len() && i < s.len() && t[i] == s[i] {
        i += 1;
    }
    let mut parts: Vec<&str> = vec![".."; s.len() - i];
    parts.extend_from_slice(&t[i..]);
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

fn href(from_dir: &str, target: &str, absolute_root: &str, keep_absolute: bool) -> String {
    let h = if keep_absolute && !absolute_root.is_empty() {
        format!("{}/{}", absolute_root, target)
    } else {
        relpath(target, if from_dir.is_empty() { "." } else { from_dir })
    };
    if Regex::new(r"[\s()<>]").unwrap().is_match(&h) {
        format!("<{}>", h)
    } else {
        h
    }
}

pub fn safe_name(name: &str) -> String {
    let n = name.trim().replace(['/', '\\'], "-");
    let n = Regex::new(r"[\x00-\x1f:]").unwrap().replace_all(&n, "").to_string();
    let n = n.trim_matches(|c| c == '.' || c == ' ').to_string();
    if n.is_empty() {
        "sans-nom".into()
    } else {
        n
    }
}

pub struct Mover {
    app: Arc<App>,
    root: PathBuf,
    sc: Arc<Scanner>,
}

struct Ctx {
    jdir: PathBuf,
    backups: BTreeMap<String, bool>,
}

impl Ctx {
    fn backup(&mut self, root: &Path, rel: &str) {
        if self.backups.contains_key(rel) {
            return;
        }
        let p = root.join(rel);
        if p.exists() {
            let dst = self.jdir.join("texts").join(rel);
            if let Some(d) = dst.parent() {
                let _ = fs::create_dir_all(d);
            }
            let _ = fs::copy(&p, &dst);
            self.backups.insert(rel.into(), true);
        } else {
            self.backups.insert(rel.into(), false); // fichier créé par l'opération (supprimé à l'annulation)
        }
    }
}

impl Mover {
    pub fn new(app: Arc<App>) -> Result<Self, String> {
        let root = app.root().ok_or("aucun dossier configuré")?;
        let sc = app.scanner().ok_or("aucun dossier configuré")?;
        Ok(Mover { app, root, sc })
    }

    /// Vérifie chaque déplacement demandé. items = [{src, dest_dir, name}]
    pub fn plan(&self, items: &Value, create_dirs: bool) -> Vec<Value> {
        let mut out = vec![];
        let mut seen_targets = HashSet::new();
        for it in items.as_array().cloned().unwrap_or_default() {
            let g = |k: &str| it.get(k).and_then(|v| v.as_str()).unwrap_or("").trim_matches('/').to_string();
            let src = g("src");
            let dest_dir = g("dest_dir");
            let mut name = safe_name(&if g("name").is_empty() { basename(&src) } else { it["name"].as_str().unwrap_or("").to_string() });
            let mut r = json!({"src": src, "dest_dir": dest_dir, "name": name, "ok": false});
            let sp = match self.app.safe(&src) {
                Ok(p) => p,
                Err(_) => {
                    r["error"] = json!("chemin hors du dossier");
                    out.push(r);
                    continue;
                }
            };
            if !sp.is_file() {
                r["error"] = json!("fichier introuvable");
                out.push(r);
                continue;
            }
            if splitext(&name).1.is_empty() && !splitext(&src).1.is_empty() {
                name = format!("{}{}", name, splitext(&src).1);
                r["name"] = json!(name);
            }
            if dest_dir.is_empty() {
                r["error"] = json!("destination vide");
                out.push(r);
                continue;
            }
            let dp = PathBuf::from(normpath(&self.root.join(&dest_dir).to_string_lossy()));
            let dp = fs::canonicalize(&dp).unwrap_or(dp);
            if !dp.starts_with(&self.root) {
                r["error"] = json!("chemin hors du dossier");
                out.push(r);
                continue;
            }
            if !self.sc.allowed(&dp) {
                r["error"] = json!("destination hors des dossiers suivis");
                out.push(r);
                continue;
            }
            if !dp.exists() && !create_dirs {
                r["error"] = json!("le dossier de destination n'existe pas");
                out.push(r);
                continue;
            }
            if dp.exists() && !dp.is_dir() {
                r["error"] = json!("la destination n'est pas un dossier");
                out.push(r);
                continue;
            }
            let target = format!("{}/{}", dest_dir, name);
            if target == src {
                r["error"] = json!("même emplacement et même nom");
                out.push(r);
                continue;
            }
            if self.root.join(&target).exists() || seen_targets.contains(&target) {
                r["error"] = json!("un fichier porte déjà ce nom à cet endroit");
                out.push(r);
                continue;
            }
            seen_targets.insert(target.clone());
            r["ok"] = json!(true);
            r["target"] = json!(target);
            r["new_dir"] = json!(!dp.exists());
            r["size"] = json!(fs::metadata(&sp).map(|m| m.len()).unwrap_or(0));
            out.push(r);
        }
        out
    }

    pub fn run(&self, items: &Value, create_dirs: bool, update_index: bool, note: &str) -> Value {
        let mut plan = self.plan(items, create_dirs);
        if !plan.iter().any(|p| p["ok"] == json!(true)) {
            return json!({"ok": false, "plan": plan, "error": "rien à déplacer"});
        }
        let jid = format!("{}_{}", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"), std::process::id() % 1000);
        let jdir = journal_dir().join(&jid);
        let _ = fs::create_dir_all(jdir.join("texts"));
        let mut ctx = Ctx { jdir: jdir.clone(), backups: BTreeMap::new() };
        let mut moves = vec![];
        let mut created_dirs: Vec<String> = vec![];
        let mut done: Vec<Value> = vec![];
        for p in plan.iter_mut().filter(|p| p["ok"] == json!(true)) {
            let src = self.root.join(p["src"].as_str().unwrap());
            let target = self.root.join(p["target"].as_str().unwrap());
            if let Some(par) = target.parent() {
                if !par.exists() {
                    let _ = fs::create_dir_all(par);
                    created_dirs.push(p["dest_dir"].as_str().unwrap().to_string());
                }
            }
            let r = fs::rename(&src, &target).or_else(|_| fs::copy(&src, &target).and_then(|_| fs::remove_file(&src)));
            if let Err(e) = r {
                p["ok"] = json!(false);
                p["error"] = json!(format!("déplacement impossible : {}", e));
                continue;
            }
            moves.push(json!({"old": p["src"], "new": p["target"], "size": p["size"]}));
            done.push(p.clone());
        }
        let mut report = json!({"catalog_yaml": 0, "catalog_jsonl": 0, "log": 0, "links": 0, "notes": []});
        if update_index && !done.is_empty() {
            // descriptions lues AVANT de toucher aux catalogues
            let cat = self.sc.catalog();
            let desc_by_new: HashMap<String, String> = done
                .iter()
                .filter_map(|p| {
                    let d = cat.get(p["src"].as_str()?)?.get("description")?.as_str()?.to_string();
                    if d.is_empty() { None } else { Some((p["target"].as_str()?.to_string(), d)) }
                })
                .collect();
            self.update_catalog_yaml(&done, &mut ctx, &mut report);
            self.update_catalog_jsonl(&done, &mut ctx, &mut report, &desc_by_new);
            self.update_log(&done, &mut ctx, &mut report, note);
            self.update_links(&done, &mut ctx, &mut report);
        }
        let mut texts = Map::new();
        for (rel, existed) in &ctx.backups {
            texts.insert(rel.clone(), json!({"existed": existed, "sha_after": sha_file(&self.root.join(rel))}));
        }
        let journal = json!({"id": jid, "time": now(), "moves": moves, "texts": texts, "created_dirs": created_dirs, "note": note});
        let _ = fs::write(jdir.join("journal.json"), serde_json::to_string_pretty(&journal).unwrap_or_default());
        self.app.start_scan();
        json!({"ok": !done.is_empty(), "id": jid, "plan": plan, "moved": journal["moves"], "report": report,
               "journal": jdir.to_string_lossy()})
    }

    fn catalogs(&self, yaml: bool) -> Vec<String> {
        let d = self.sc.data.read().unwrap();
        let mut v: Vec<String> = d
            .index_paths
            .iter()
            .filter(|r| {
                let b = basename(r).to_lowercase();
                if yaml { b == "catalog.yaml" || b == "catalog.yml" } else { b == "catalog.jsonl" }
            })
            .cloned()
            .collect();
        v.sort();
        v
    }

    /// catalog.yaml du dossier principal qui correspond à un chemin d'archive (Archive/photos/... -> Photos/catalog.yaml).
    fn vault_catalog(&self, rel: &str) -> Option<String> {
        let top = self.vault_top(rel)?;
        for n in ["catalog.yaml", "catalog.yml"] {
            if self.root.join(&top).join(n).exists() {
                return Some(format!("{}/{}", top, n));
            }
        }
        None
    }

    fn vault_top(&self, rel: &str) -> Option<String> {
        let parts: Vec<&str> = rel.split('/').collect();
        let arch = self.sc.cfg.archive();
        if arch.as_deref() != Some(parts[0]) {
            return Some(parts[0].to_string());
        }
        if parts.len() < 2 {
            return None;
        }
        self.sc.brain_of(&format!("{}/{}", parts[0], parts[1])).map(|b| b.split('/').next().unwrap_or("").to_string())
    }

    fn split_entries(text: &str) -> Option<(Vec<String>, Vec<Vec<String>>)> {
        let lines: Vec<String> = text.split('\n').map(String::from).collect();
        let re = Regex::new(r"^entries\s*:").unwrap();
        let start = lines.iter().position(|l| re.is_match(l))?;
        let mut head: Vec<String> = lines[..=start].to_vec();
        let mut blocks: Vec<Vec<String>> = vec![];
        let item = Regex::new(r"^\s*-\s+\S").unwrap();
        for l in &lines[start + 1..] {
            if item.is_match(l) {
                blocks.push(vec![l.clone()]);
            } else if let Some(b) = blocks.last_mut() {
                b.push(l.clone());
            } else {
                head.push(l.clone());
            }
        }
        Some((head, blocks))
    }

    fn field(block: &[String], key: &str) -> Option<String> {
        let re = Regex::new(&format!(r"^\s*(?:-\s+)?{}\s*:\s*(.*)$", regex::escape(key))).unwrap();
        block.iter().find_map(|l| re.captures(l).map(|m| m[1].trim().trim_matches(|c| c == '"' || c == '\'').to_string()))
    }

    fn block_target(block: &[String], cat_dir: &str) -> Option<String> {
        let (root, path, link) = (Self::field(block, "root"), Self::field(block, "path"), Self::field(block, "link"));
        let nz = |o: &Option<String>| o.as_ref().filter(|s| !s.is_empty()).cloned();
        if let (Some(r), Some(p)) = (nz(&root), nz(&path)) {
            return Some(normpath(&format!("{}/{}", r, p)));
        }
        if let Some(l) = nz(&link) {
            return Some(normpath(&join(cat_dir, &l)));
        }
        nz(&path).map(|p| normpath(&join(cat_dir, &p)))
    }

    fn rewrite_block(block: &[String], new_rel: &str, cat_dir: &str) -> Vec<String> {
        let root = Self::field(block, "root").filter(|s| !s.is_empty());
        let re = Regex::new(r"^(\s*(?:-\s+)?)(path|link)(\s*:\s*)(.*)$").unwrap();
        block
            .iter()
            .map(|l| match re.captures(l) {
                Some(m) => {
                    let key = &m[2];
                    let val = if key == "path" {
                        match &root {
                            Some(r) if new_rel.starts_with(&format!("{}/", r)) => relpath(new_rel, r),
                            _ => relpath(new_rel, if cat_dir.is_empty() { "." } else { cat_dir }),
                        }
                    } else {
                        relpath(new_rel, if cat_dir.is_empty() { "." } else { cat_dir })
                    };
                    if key == "link" {
                        format!("{}{}{}\"{}\"", &m[1], key, &m[3], val)
                    } else {
                        format!("{}{}{}{}", &m[1], key, &m[3], val)
                    }
                }
                None => l.clone(),
            })
            .collect()
    }

    fn note(report: &mut Value, s: String) {
        if let Some(a) = report["notes"].as_array_mut() {
            a.push(json!(s));
        }
    }

    fn inc(report: &mut Value, k: &str, n: u64) {
        report[k] = json!(report[k].as_u64().unwrap_or(0) + n);
    }

    fn update_catalog_yaml(&self, done: &[Value], ctx: &mut Ctx, report: &mut Value) {
        let moves: HashMap<String, String> =
            done.iter().map(|p| (p["src"].as_str().unwrap().to_string(), p["target"].as_str().unwrap().to_string())).collect();
        let mut appended: BTreeMap<String, Vec<(Vec<String>, String)>> = BTreeMap::new();
        for cat in self.catalogs(true) {
            let cp = self.root.join(&cat);
            let Ok(text) = fs::read_to_string(&cp) else { continue };
            let Some((head, blocks)) = Self::split_entries(&text) else { continue };
            let cat_dir = dirname(&cat);
            let mut keep: Vec<Vec<String>> = vec![];
            let mut changed = false;
            for b in blocks {
                let tgt = Self::block_target(&b, &cat_dir);
                match tgt.as_ref().and_then(|t| moves.get(t)) {
                    Some(new) => {
                        let dest_cat = self.vault_catalog(new);
                        let same_vault = self.vault_top(new).as_deref() == cat.split('/').next();
                        if dest_cat.as_deref() == Some(cat.as_str()) || (dest_cat.is_none() && same_vault) {
                            keep.push(Self::rewrite_block(&b, new, &cat_dir));
                        } else if let Some(dc) = dest_cat {
                            Self::note(report, format!("fiche déplacée de {} vers {}", cat, dc));
                            appended.entry(dc).or_default().push((b, new.clone()));
                        } else {
                            // le dossier de destination n'a pas de catalog.yaml : la description part dans CATALOG.jsonl
                            Self::note(report, format!("fiche retirée de {} (description reportée dans CATALOG.jsonl)", cat));
                        }
                        changed = true;
                        Self::inc(report, "catalog_yaml", 1);
                    }
                    None => keep.push(b),
                }
            }
            if changed {
                ctx.backup(&self.root, &cat);
                let mut all = head.clone();
                for b in keep {
                    all.extend(b);
                }
                let _ = fs::write(&cp, all.join("\n"));
            }
        }
        for (dest_cat, entries) in appended {
            let dp = self.root.join(&dest_cat);
            ctx.backup(&self.root, &dest_cat);
            let mut text = fs::read_to_string(&dp).unwrap_or_else(|_| "entries:\n".into());
            if !Regex::new(r"(?m)^entries\s*:").unwrap().is_match(&text) {
                text = format!("{}\nentries:\n", text.trim_end_matches('\n'));
            }
            let cat_dir = dirname(&dest_cat);
            let mut add: Vec<String> = vec![];
            for (b, new) in entries {
                add.push(String::new());
                add.extend(Self::rewrite_block(&b, &new, &cat_dir));
            }
            let _ = fs::write(&dp, format!("{}\n{}\n", text.trim_end_matches('\n'), add.join("\n").trim_end_matches('\n')));
        }
    }

    fn update_catalog_jsonl(&self, done: &[Value], ctx: &mut Ctx, report: &mut Value, desc_by_new: &HashMap<String, String>) {
        for cat in self.catalogs(false) {
            let base = dirname(&cat);
            let pre = if base.is_empty() { String::new() } else { format!("{}/", base) };
            let cp = self.root.join(&cat);
            let Ok(text) = fs::read_to_string(&cp) else { continue };
            let mut changed = false;
            let mut found = HashSet::new();
            let mut out: Vec<String> = vec![];
            for line in text.lines() {
                let mut e: Map<String, Value> = match serde_json::from_str::<Value>(line) {
                    Ok(Value::Object(o)) => o,
                    _ => {
                        out.push(line.to_string());
                        continue;
                    }
                };
                let full = e.get("rel").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|r| normpath(&format!("{}{}", pre, r)));
                let hit = full.and_then(|f| done.iter().find(|p| p["src"].as_str() == Some(f.as_str())));
                match hit {
                    Some(h) => {
                        let src = h["src"].as_str().unwrap().to_string();
                        let target = h["target"].as_str().unwrap().to_string();
                        found.insert(src);
                        if target.starts_with(&pre) {
                            let nrel = target[pre.len()..].to_string();
                            e.insert("rel".into(), json!(nrel));
                            e.insert("name".into(), json!(basename(&nrel)));
                            e.insert("ext".into(), json!(splitext(&nrel).1.to_lowercase()));
                            e.insert("node".into(), json!(nrel.split('/').next().unwrap_or("")));
                            if let Some(d) = desc_by_new.get(&target) {
                                if e.get("note").map(|v| v.is_null() || v == "").unwrap_or(true) {
                                    e.insert("note".into(), json!(d));
                                }
                            }
                            out.push(crate::util::py_json(&Value::Object(e)));
                        }
                        changed = true;
                        Self::inc(report, "catalog_jsonl", 1);
                    }
                    None => out.push(line.to_string()),
                }
            }
            for p in done {
                let src = p["src"].as_str().unwrap();
                let target = p["target"].as_str().unwrap();
                if !found.contains(src) && target.starts_with(&pre) {
                    let tp = self.root.join(target);
                    let nrel = &target[pre.len()..];
                    let mut e = json!({"rel": nrel, "name": basename(nrel), "ext": splitext(nrel).1.to_lowercase(), "kind": kind(nrel),
                        "bytes": fs::metadata(&tp).map(|m| json!(m.len())).unwrap_or(p["size"].clone()),
                        "mtime": chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%z").to_string(), "node": nrel.split('/').next().unwrap_or("")});
                    if let Some(d) = desc_by_new.get(target) {
                        e["note"] = json!(d);
                    }
                    out.push(crate::util::py_json(&e));
                    changed = true;
                    Self::inc(report, "catalog_jsonl", 1);
                }
            }
            if changed {
                ctx.backup(&self.root, &cat);
                let _ = fs::write(&cp, format!("{}\n", out.join("\n")));
            }
        }
    }

    fn update_log(&self, done: &[Value], ctx: &mut Ctx, report: &mut Value, note: &str) {
        let (Some(log), Some(arch)) = (self.sc.cfg.archive_log(), self.sc.cfg.archive()) else { return };
        let lp = self.root.join(&log);
        if !lp.exists() {
            return;
        }
        ctx.backup(&self.root, &log);
        let stamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string();
        let rel = |d: &str| {
            if let Some(x) = d.strip_prefix(&format!("{}/", arch)) {
                format!("{}/", x)
            } else {
                format!("../{}/", d)
            }
        };
        let txt = note.replace('|', "/").trim().to_string();
        let mut rows = vec![];
        for p in done {
            let (s, t) = (p["src"].as_str().unwrap(), p["target"].as_str().unwrap());
            let (od, on, nd, nn) = (dirname(s), basename(s), dirname(t), basename(t));
            let action = if od != nd && on != nn { "move+rename" } else if od == nd { "rename" } else { "move" };
            rows.push(format!("| {} | {} | {} | {} | {} | {} | {} | Hermes Map (manuel){} |", stamp, action, rel(&od), rel(&nd), on, nn,
                p["size"], if txt.is_empty() { String::new() } else { format!(" — {}", txt) }));
        }
        let text = fs::read_to_string(&lp).unwrap_or_default();
        let add = format!("{}{}\n", if text.ends_with('\n') || text.is_empty() { "" } else { "\n" }, rows.join("\n"));
        let _ = fs::write(&lp, format!("{}{}", text, add));
        report["log"] = json!(rows.len());
    }

    fn update_links(&self, done: &[Value], ctx: &mut Ctx, report: &mut Value) {
        let moves: HashMap<String, String> =
            done.iter().map(|p| (p["src"].as_str().unwrap().to_string(), p["target"].as_str().unwrap().to_string())).collect();
        let olds: Vec<String> = moves.keys().map(|s| basename(s).to_lowercase()).collect();
        let root_abs = self.root.to_string_lossy().to_string();
        let mut rels: Vec<String> = self.sc.data.read().unwrap().index_paths.iter().cloned().collect();
        rels.sort();
        for rel in rels {
            let low = rel.to_lowercase();
            if !(low.ends_with(".md") || low.ends_with(".markdown")) {
                continue;
            }
            let p = self.root.join(&rel);
            let Ok(text) = fs::read_to_string(&p) else { continue };
            let lt = text.to_lowercase();
            if !olds.iter().any(|n| lt.contains(n) || lt.contains(&n.replace(' ', "%20"))) {
                continue;
            }
            let note_dir = dirname(&rel);
            let mut count = 0u64;
            let new = MDLINK.replace_all(&text, |m: &regex::Captures| {
                let raw = &m[2];
                let mut h = if raw.starts_with('<') { raw[1..raw.len() - 1].to_string() } else { unquote(raw) };
                let mut anchor = String::new();
                if let Some(i) = h.find('#') {
                    anchor = h[i..].to_string();
                    h.truncate(i);
                }
                let absolute = h.starts_with('/');
                let tgt = if absolute {
                    match h.strip_prefix(&format!("{}/", root_abs)) {
                        Some(x) => normpath(x),
                        None => return m[0].to_string(),
                    }
                } else {
                    normpath(&join(&note_dir, &h))
                };
                let Some(dst) = moves.get(&tgt) else { return m[0].to_string() };
                count += 1;
                let mut hr = href(&note_dir, dst, &root_abs, absolute);
                if !anchor.is_empty() {
                    hr = if hr.starts_with('<') { format!("{}{}>", &hr[..hr.len() - 1], anchor) } else { format!("{}{}", hr, anchor) };
                }
                format!("{}{}{}", &m[1], hr, &m[3])
            });
            if count > 0 {
                ctx.backup(&self.root, &rel);
                let _ = fs::write(&p, new.as_bytes());
                Self::inc(report, "links", count);
                Self::note(report, format!("{} lien(s) mis à jour dans {}", count, rel));
            }
        }
    }

    pub fn undo(&self, jid: &str) -> Value {
        if jid.contains('/') || jid.contains('\\') || jid.contains("..") {
            return json!({"ok": false, "error": "introuvable"});
        }
        let jdir = journal_dir().join(jid);
        let mut j: Value = match fs::read_to_string(jdir.join("journal.json")).ok().and_then(|t| serde_json::from_str(&t).ok()) {
            Some(v) => v,
            None => return json!({"ok": false, "error": "introuvable"}),
        };
        if j.get("undone").map(|v| !v.is_null()).unwrap_or(false) {
            return json!({"ok": false, "error": "déjà annulé"});
        }
        let (mut back, mut skipped, mut restored) = (vec![], vec![], vec![]);
        for m in j["moves"].as_array().cloned().unwrap_or_default().iter().rev() {
            let (new, old) = (self.root.join(m["new"].as_str().unwrap_or("")), self.root.join(m["old"].as_str().unwrap_or("")));
            if new.exists() && !old.exists() {
                if let Some(d) = old.parent() {
                    let _ = fs::create_dir_all(d);
                }
                if fs::rename(&new, &old).is_ok() {
                    back.push(m["old"].clone());
                    continue;
                }
            }
            skipped.push(m["new"].as_str().unwrap_or("").to_string());
        }
        for (rel, info) in j["texts"].as_object().cloned().unwrap_or_default() {
            let p = self.root.join(&rel);
            if let Some(sa) = info.get("sha_after").and_then(|v| v.as_str()) {
                if p.exists() && sha_file(&p).as_deref() != Some(sa) {
                    skipped.push(format!("{} (modifié depuis)", rel));
                    continue;
                }
            }
            if info.get("existed").and_then(|v| v.as_bool()).unwrap_or(false) {
                if fs::copy(jdir.join("texts").join(&rel), &p).is_ok() {
                    restored.push(rel);
                }
            } else if p.exists() && fs::remove_file(&p).is_ok() {
                restored.push(rel);
            }
        }
        let mut dirs: Vec<String> = j["created_dirs"].as_array().cloned().unwrap_or_default().iter().filter_map(|v| v.as_str().map(String::from)).collect();
        dirs.sort_by_key(|d| std::cmp::Reverse(d.len()));
        for d in dirs {
            let _ = fs::remove_dir(self.root.join(d)); // seulement s'il est resté vide
        }
        j["undone"] = json!(now());
        let _ = fs::write(jdir.join("journal.json"), serde_json::to_string_pretty(&j).unwrap_or_default());
        self.app.start_scan();
        json!({"ok": true, "moved_back": back, "restored": restored, "skipped": skipped})
    }
}

/// Dossiers proposés comme destination : par mots, archive d'abord.
pub fn suggest_dirs(sc: &Scanner, q: &str) -> Vec<String> {
    let words: Vec<String> = q.to_lowercase().split_whitespace().map(String::from).collect();
    let arch = sc.cfg.archive();
    let d = sc.data.read().unwrap();
    let mut out: Vec<String> = d
        .index_dirs
        .iter()
        .filter(|x| !x.contains("/.") && !x.starts_with('.'))
        .filter(|x| {
            let low = x.to_lowercase();
            words.iter().all(|w| low.contains(w.as_str()))
        })
        .cloned()
        .collect();
    out.sort_by(|a, b| {
        let k = |x: &String| (!arch.as_ref().map(|ar| x.starts_with(&format!("{}/", ar))).unwrap_or(false), x.matches('/').count(), x.to_lowercase());
        k(a).cmp(&k(b))
    });
    out.truncate(40);
    out
}
