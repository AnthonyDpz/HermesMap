//! Lance Hermes (« hermes chat --query-file - ») sur un dossier ou une note, ou pour reclasser des fichiers :
//! sauvegarde préalable, suivi en direct (lectures, écritures, déplacements), liste des changements, annulation.

use crate::app::App;
use crate::config::{self, Settings};
use crate::prompts::*;
use crate::util::{basename, now, rel_of};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

const MAX_BACKUP_FILES: usize = 5000;
const MAX_BACKUP_BYTES: u64 = 300 * 1024 * 1024;
const MAX_MD_SIZE: u64 = 5 * 1024 * 1024;
const MAX_RUNTIME: u64 = 45 * 60; // sécurité : arrêt automatique après 45 min
const SKIP_DIRS: &[&str] = &[".git", "node_modules", "__pycache__", ".venv", "venv", ".obsidian", ".trash"];
static ANSI_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07|\r").unwrap());
static MD_IN_LINE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?i)[^\s'"`<>()\[\]|:]+\.(?:md|markdown)\b"#).unwrap());

pub fn backup_root() -> PathBuf {
    config::data_dir().join("backups")
}

pub fn find_hermes(explicit: Option<String>) -> Option<PathBuf> {
    if let Some(e) = explicit.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(e));
    }
    if let Some(p) = crate::media::which("hermes") {
        return Some(p);
    }
    for c in ["~/.local/bin/hermes", "/opt/homebrew/bin/hermes", "/usr/local/bin/hermes", "~/.hermes/hermes-agent/venv/bin/hermes"] {
        let p = config::expand(c);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Consigne de reclassement, adaptée aux réglages (avec ou sans dossier d'archivage) et à la langue.
pub fn reclass_prompt(cfg: &Settings, zones: &[String], en: bool) -> String {
    let ex = zones.first().cloned().unwrap_or_else(|| if en { "Projects".into() } else { "Projets".into() });
    let conv: Vec<String> = cfg.hermes_list("conventions").into_iter().filter(|c| !c.is_empty()).collect();
    let mut lines: Vec<String> = vec![];
    if en {
        let z = if zones.is_empty() { "(top-level folders)".into() } else { zones.iter().take(30).cloned().collect::<Vec<_>>().join(", ") };
        lines.extend(["Reclassifying misfiled files in my knowledge base (root: {racine}).".into(), String::new(),
            "Files to process ({nb}):".into(), "{fichiers}".into(), String::new(), "For EACH file:".into(),
            format!("1. Analyse it (look at the image / read the document): what is it? Which folder does it really belong to? Main folders: {}.", z),
            "2. File it away:".into(), "   - new readable name, no emoji or random characters, correct extension;".into()]);
        if let Some(arch) = cfg.archive() {
            lines.push(format!("   - notes (.md) stay in the main folders; other files (images, videos, PDFs, documents…) go into the archive folder, at the “mirror” path of the related note or folder: move (mv) to {}/<main folder without prefix, lowercase>/<same path>/ (e.g. a file related to {}/trips/japan.md goes into {}/{}/trips/);", arch, ex, arch, cfg.zone_name(&ex).to_lowercase()));
            lines.push("   - update the indexes: remove the entry or link from the old location (catalog.yaml, INDEX.md, note) and add it in the right place at the destination;".into());
            lines.push(format!("   - update {} (one JSON entry per line, “rel” relative to {}) and add a row to the table in {} (same format as the existing rows);",
                cfg.archive_catalog().unwrap_or_default(), arch, cfg.archive_log().unwrap_or_default()));
        } else {
            lines.push("   - move (mv) it to the most suitable folder, preferably one that already exists;".into());
            lines.push("   - update the indexes: remove the entry or link from the old location (catalog.yaml, INDEX.md, note) and add it in the right place at the destination;".into());
        }
        lines.push("   - standard relative Markdown links: [Title](relative/path); fix the links in notes that pointed to the old location.".into());
        if !conv.is_empty() {
            lines.push(format!("   - also apply the filing rules described in: {}.", conv.join(", ")));
        }
        return format!("{}\n{}", lines.join("\n"), RECLASS_TAIL_EN);
    }
    let z = if zones.is_empty() { "(dossiers du 1er niveau)".into() } else { zones.iter().take(30).cloned().collect::<Vec<_>>().join(", ") };
    lines.push(RECLASS_HEAD.replace("À quel dossier appartient-il vraiment ?", &format!("À quel dossier appartient-il vraiment ? Dossiers principaux : {}.", z)));
    lines.push("2. Range-le :".into());
    lines.push("   - nouveau nom lisible, sans emoji ni suite de caractères aléatoires, extension correcte ;".into());
    if let Some(arch) = cfg.archive() {
        lines.push(format!("   - les notes (.md) restent dans les dossiers principaux ; les autres fichiers (images, vidéos, PDF, documents…) sont rangés dans le dossier d'archivage, au chemin « miroir » de la note ou du dossier concerné : déplacement (mv) vers {}/<dossier principal sans préfixe, en minuscules>/<même chemin>/ (ex. un fichier lié à {}/voyages/japon.md va dans {}/{}/voyages/) ;", arch, ex, arch, cfg.zone_name(&ex).to_lowercase()));
        lines.push("   - mets à jour les index : retire la fiche ou le lien de l'ancien emplacement (catalog.yaml, INDEX.md, note) et ajoute-le au bon endroit de la destination ;".into());
        lines.push(format!("   - mets à jour {} (une fiche JSON par ligne, « rel » relatif à {}) et ajoute une ligne au tableau de {} (même format que les lignes existantes) ;",
            cfg.archive_catalog().unwrap_or_default(), arch, cfg.archive_log().unwrap_or_default()));
    } else {
        lines.push("   - déplacement (mv) vers le dossier le plus adapté, de préférence un dossier qui existe déjà ;".into());
        lines.push("   - mets à jour les index : retire la fiche ou le lien de l'ancien emplacement (catalog.yaml, INDEX.md, note) et ajoute-le au bon endroit de la destination ;".into());
    }
    lines.push("   - liens Markdown classiques et relatifs : [Titre](chemin/relatif) ; corrige les liens des notes qui pointaient vers l'ancien emplacement.".into());
    if !conv.is_empty() {
        lines.push(format!("   - applique aussi les règles de rangement décrites dans : {}.", conv.join(", ")));
    }
    format!("{}\n{}", lines.join("\n"), RECLASS_TAIL)
}

fn sha(data: &[u8]) -> String {
    let mut h = sha1_smol::Sha1::new();
    h.update(data);
    h.digest().to_string()
}

fn walk(target: &Path, skip: &[String], out: &mut Vec<PathBuf>) {
    if target.is_file() {
        out.push(target.to_path_buf());
        return;
    }
    let mut stack = vec![target.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) && !skip.contains(&name) && !name.starts_with('.') {
                    stack.push(e.path());
                }
            } else if !name.starts_with('.') || name.ends_with(".icloud") {
                out.push(e.path());
            }
        }
    }
}

fn mtime_of(md: &fs::Metadata) -> f64 {
    crate::media::mtime(md)
}

#[derive(Default)]
struct Inner {
    lines: Vec<String>,
    state: String,
    exit_code: Option<i32>,
    ended: Option<f64>,
    changes: Option<Value>,
    undone: bool,
    events: Vec<Value>,
    snapshot: HashMap<String, (String, u64, f64)>, // rel -> (sha, size, mtime)
    fps: HashMap<String, Option<String>>,
    names: HashMap<String, String>,
    last_read: HashMap<String, f64>,
}

pub struct Job {
    pub id: String,
    root: PathBuf,
    target: PathBuf,
    mode: String,
    files: Vec<String>,
    cwd: Option<PathBuf>,
    roots: Vec<PathBuf>,
    text_exts: Vec<&'static str>,
    skip: Vec<String>,
    prompt: String,
    cmd: Vec<String>,
    started: f64,
    backup_dir: PathBuf,
    inner: Mutex<Inner>,
    child: Mutex<Option<Child>>,
    app: Weak<App>,
}

impl Job {
    fn rel(&self, p: &Path) -> String {
        rel_of(&self.root, p)
    }

    fn iter(&self) -> Vec<PathBuf> {
        let mut out = vec![];
        for r in &self.roots {
            walk(r, &self.skip, &mut out);
        }
        out
    }

    fn is_text(&self, p: &Path) -> bool {
        let s = crate::media::suffix(&p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
        self.text_exts.contains(&s.as_str())
    }

    /// Empreinte rapide (taille + début + fin) pour retrouver un fichier même renommé / déplacé.
    fn fingerprint(p: &Path) -> Option<String> {
        let size = fs::metadata(p).ok()?.len();
        let mut f = fs::File::open(p).ok()?;
        let mut head = vec![0u8; 65536];
        let n = f.read(&mut head).ok()?;
        head.truncate(n);
        let mut tail = vec![];
        if size > 131072 {
            f.seek(SeekFrom::End(-65536)).ok()?;
        }
        let _ = f.read_to_end(&mut tail);
        let mut both = head;
        both.extend(tail);
        Some(format!("{}:{}", size, sha(&both)))
    }

    pub fn state(&self) -> String {
        self.inner.lock().unwrap().state.clone()
    }

    pub fn running(&self) -> bool {
        let s = self.state();
        s == "preparing" || s == "running"
    }

    fn log(&self, line: &str) {
        let running = {
            let mut i = self.inner.lock().unwrap();
            i.lines.push(line.to_string());
            if i.lines.len() > 20000 {
                let cut = i.lines.len() - 15000;
                i.lines.drain(..cut);
            }
            i.state == "running"
        };
        if running && !line.starts_with("[Hermes Map]") {
            self.spot_reads(line);
        }
    }

    fn event(&self, kind: &str, rel: &str, extra: Value) {
        let mut i = self.inner.lock().unwrap();
        let mut e = json!({"seq": i.events.len(), "t": now(), "kind": kind, "path": rel});
        if let Some(o) = extra.as_object() {
            for (k, v) in o {
                e[k] = v.clone();
            }
        }
        i.events.push(e);
    }

    // lectures : on repère dans le journal de Hermes les noms de notes du dossier ciblé
    fn spot_reads(&self, line: &str) {
        let low = line.to_lowercase();
        if !low.contains(".md") && !low.contains(".markdown") {
            return;
        }
        let names = self.inner.lock().unwrap().names.clone();
        let mut hits: Vec<String> = vec![];
        if names.len() <= 5000 {
            for (base, rel) in &names {
                if low.contains(base.as_str()) {
                    hits.push(rel.clone());
                }
            }
        } else {
            for m in MD_IN_LINE.find_iter(&low) {
                if let Some(rel) = names.get(&basename(m.as_str())) {
                    hits.push(rel.clone());
                }
            }
        }
        // « notes.md » est aussi contenu dans « mes-notes.md » : on garde le nom le plus long
        hits.sort_by_key(|r| std::cmp::Reverse(basename(r).len()));
        let mut kept: Vec<String> = vec![];
        for r in hits {
            let b = basename(&r).to_lowercase();
            if !kept.iter().any(|k| basename(k).to_lowercase().contains(&b)) {
                kept.push(r);
            }
        }
        for rel in kept.into_iter().take(5) {
            let fire = {
                let mut i = self.inner.lock().unwrap();
                let last = i.last_read.get(&rel).copied().unwrap_or(0.0);
                if now() - last > 3.0 {
                    i.last_read.insert(rel.clone(), now());
                    true
                } else {
                    false
                }
            };
            if fire {
                self.event("read", &rel, json!({}));
            }
        }
    }

    // écritures : on surveille les fichiers texte suivis (date de modification / taille)
    fn watch(self: Arc<Self>) {
        let mut seen: HashMap<String, (f64, u64)> =
            self.inner.lock().unwrap().snapshot.iter().map(|(r, v)| (r.clone(), (v.2, v.1))).collect();
        let snap: HashSet<String> = seen.keys().cloned().collect();
        let mut gone = HashSet::new();
        while self.running() {
            std::thread::sleep(Duration::from_millis(if self.mode != "reclass" { 800 } else { 2000 }));
            for rel in &self.files {
                if !gone.contains(rel) && !self.root.join(rel).exists() {
                    gone.insert(rel.clone());
                    self.event("move", rel, json!({}));
                }
            }
            for p in self.iter() {
                if !self.is_text(&p) {
                    continue;
                }
                let Ok(md) = fs::metadata(&p) else { continue };
                let rel = self.rel(&p);
                let sig = (mtime_of(&md), md.len());
                if seen.get(&rel) == Some(&sig) {
                    continue;
                }
                let kind = if seen.contains_key(&rel) || snap.contains(&rel) { "write" } else { "create" };
                seen.insert(rel.clone(), sig);
                let low = rel.to_lowercase();
                let (links, tags) = if low.ends_with(".md") || low.ends_with(".markdown") {
                    self.app.upgrade().map(|a| a.links_of(&rel)).unwrap_or_default()
                } else {
                    (vec![], vec![])
                };
                self.event(kind, &rel, json!({"links": links, "tags": tags}));
            }
        }
    }

    // ---- 1. sauvegarde
    fn backup(&self) {
        let (mut n, mut total) = (0usize, 0u64);
        let _ = fs::create_dir_all(&self.backup_dir);
        for p in self.iter() {
            if !self.is_text(&p) {
                continue;
            }
            let Ok(md) = fs::metadata(&p) else { continue };
            if md.len() > MAX_MD_SIZE {
                continue;
            }
            let Ok(data) = fs::read(&p) else { continue }; // (télécharge le fichier s'il est resté dans iCloud)
            let rel = self.rel(&p);
            let dst = self.backup_dir.join("files").join(&rel);
            if let Some(d) = dst.parent() {
                let _ = fs::create_dir_all(d);
            }
            let _ = fs::write(&dst, &data);
            self.inner.lock().unwrap().snapshot.insert(rel, (sha(&data), data.len() as u64, mtime_of(&md)));
            n += 1;
            total += data.len() as u64;
            if n >= MAX_BACKUP_FILES || total >= MAX_BACKUP_BYTES {
                self.log(&format!("[Hermes Map] ⚠️ sauvegarde limitée à {} fichiers ({} Ko)", n, total / 1024));
                break;
            }
        }
        let what = if self.mode != "reclass" { ".md" } else { "d'index (.md, .yaml, .jsonl)" };
        self.log(&format!("[Hermes Map] Sauvegarde de {} fichier(s) {} dans {}", n, what, self.backup_dir.display()));
        for rel in &self.files {
            let fp = Self::fingerprint(&self.root.join(rel));
            self.inner.lock().unwrap().fps.insert(rel.clone(), fp);
        }
        if !self.files.is_empty() {
            self.log(&format!("[Hermes Map] {} fichier(s) à reclasser, empreintes notées pour les retrouver après.", self.files.len()));
        }
        self.save_manifest();
    }

    fn save_manifest(&self) {
        let i = self.inner.lock().unwrap();
        let snap: Map<String, Value> =
            i.snapshot.iter().map(|(k, v)| (k.clone(), json!({"sha": v.0, "size": v.1, "mtime": v.2}))).collect();
        let m = json!({"id": self.id, "root": self.root.to_string_lossy(), "target": self.target_rel(), "started": self.started,
            "ended": i.ended, "state": i.state, "prompt": self.prompt, "snapshot": snap, "changes": i.changes, "undone": i.undone});
        let _ = fs::write(self.backup_dir.join("manifest.json"), serde_json::to_string_pretty(&m).unwrap_or_default());
    }

    fn target_rel(&self) -> String {
        if self.target == self.root { String::new() } else { self.rel(&self.target) }
    }

    // ---- 2. exécution
    fn run(self: Arc<Self>) {
        self.log(&format!("[Hermes Map] Cible : {}", self.target.display()));
        self.backup();
        {
            let mut i = self.inner.lock().unwrap();
            let mut names = HashMap::new();
            for r in i.snapshot.keys() {
                names.entry(basename(r).to_lowercase()).or_insert_with(|| r.clone());
            }
            i.names = names;
            i.state = "running".into();
        }
        let w = self.clone();
        std::thread::spawn(move || w.watch());
        self.log(&format!("[Hermes Map] Commande : {}", self.cmd.iter().map(|c| shlex::try_quote(c).map(|x| x.to_string()).unwrap_or(c.clone())).collect::<Vec<_>>().join(" ")));
        self.log("[Hermes Map] Hermes travaille… (cela peut prendre plusieurs minutes)\n");
        let cwd = self.cwd.clone().unwrap_or_else(|| if self.target.is_dir() { self.target.clone() } else { self.target.parent().unwrap_or(&self.root).to_path_buf() });
        let mut cmd = Command::new(&self.cmd[0]);
        cmd.args(&self.cmd[1..]).current_dir(&cwd).env("NO_COLOR", "1").env("TERM", "dumb").env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        match cmd.spawn() {
            Err(e) => {
                self.inner.lock().unwrap().state = "error".into();
                if e.kind() == std::io::ErrorKind::NotFound {
                    self.log("[Hermes Map] ❌ Commande « hermes » introuvable. Relance Hermes Map avec --hermes /chemin/vers/hermes (le chemin que donne « which hermes » dans le Terminal).");
                } else {
                    self.log(&format!("[Hermes Map] ❌ Erreur : {}", e));
                }
            }
            Ok(mut child) => {
                if let Some(mut si) = child.stdin.take() {
                    let _ = si.write_all(self.prompt.as_bytes());
                }
                let out = child.stdout.take();
                let err = child.stderr.take();
                *self.child.lock().unwrap() = Some(child);
                let me = self.clone();
                let killer = std::thread::spawn(move || {
                    let t0 = now();
                    while me.running() && now() - t0 < MAX_RUNTIME as f64 {
                        std::thread::sleep(Duration::from_secs(2));
                    }
                    if me.running() {
                        me.stop("durée maximale atteinte");
                    }
                });
                drop(killer);
                let e2 = self.clone();
                let th = err.map(|e| std::thread::spawn(move || {
                    for l in BufReader::new(e).lines().map_while(Result::ok) {
                        e2.log(&ANSI_RE.replace_all(&l, ""));
                    }
                }));
                if let Some(o) = out {
                    for l in BufReader::new(o).split(b'\n').map_while(Result::ok) {
                        self.log(&ANSI_RE.replace_all(&String::from_utf8_lossy(&l), ""));
                    }
                }
                if let Some(t) = th {
                    let _ = t.join();
                }
                let code = loop {
                    let mut g = self.child.lock().unwrap();
                    match g.as_mut().map(|c| c.try_wait()) {
                        Some(Ok(Some(s))) => break s.code().unwrap_or(-1),
                        Some(Ok(None)) => {}
                        _ => break -1,
                    }
                    drop(g);
                    std::thread::sleep(Duration::from_millis(200));
                };
                {
                    let mut i = self.inner.lock().unwrap();
                    i.exit_code = Some(code);
                    if i.state != "stopped" {
                        i.state = if code == 0 { "done".into() } else { "error".into() };
                    }
                }
                self.log(&format!("\n[Hermes Map] Hermes a terminé (code {}).", code));
            }
        }
        self.inner.lock().unwrap().ended = Some(now());
        let c = self.detect_changes();
        let n = |k: &str| c[k].as_array().map(|a| a.len()).unwrap_or(0);
        self.log(&format!("[Hermes Map] Changements : {} modifié(s), {} créé(s), {} supprimé(s), {} autre(s) fichier(s) touché(s).",
            n("modified"), n("created"), n("deleted"), n("other")));
        self.inner.lock().unwrap().changes = Some(c);
        self.save_manifest();
    }

    pub fn stop(&self, reason: &str) {
        let mut g = self.child.lock().unwrap();
        let Some(child) = g.as_mut() else { return };
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        self.inner.lock().unwrap().state = "stopped".into();
        drop(g);
        self.log(&format!("\n[Hermes Map] ⏹ {}, arrêt de Hermes…", reason));
        let pid = match self.child.lock().unwrap().as_ref() {
            Some(c) => c.id(),
            None => return,
        };
        #[cfg(unix)]
        {
            let _ = Command::new("kill").args(["-TERM", &format!("-{}", pid)]).status();
            // 5 s plus tard, s'il tourne encore : arrêt forcé
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(5));
                let _ = Command::new("kill").args(["-KILL", &format!("-{}", pid)]).status();
            });
        }
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &pid.to_string()]).status();
        }
    }

    // ---- 3. changements
    fn detect_changes(&self) -> Value {
        let (snapshot, fps) = {
            let i = self.inner.lock().unwrap();
            (i.snapshot.clone(), i.fps.clone())
        };
        let (mut modified, mut created, mut other) = (vec![], vec![], vec![]);
        let mut seen = HashSet::new();
        let mut by_size: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let want: HashSet<String> = fps.values().flatten().map(|f| f.split(':').next().unwrap_or("").to_string()).collect();
        let missing = self.files.iter().any(|r| !self.root.join(r).exists());
        for p in self.iter() {
            let Ok(md) = fs::metadata(&p) else { continue };
            let rel = self.rel(&p);
            if missing && want.contains(&md.len().to_string()) {
                by_size.entry(md.len().to_string()).or_default().push(p.clone());
            }
            if let Some(s) = snapshot.get(&rel) {
                seen.insert(rel.clone());
                if mtime_of(&md) != s.2 || md.len() != s.1 {
                    if let Ok(d) = fs::read(&p) {
                        if sha(&d) != s.0 {
                            modified.push(rel);
                        }
                    }
                }
            } else if mtime_of(&md) >= self.started - 1.0 {
                if self.is_text(&p) { created.push(rel) } else { other.push(rel) }
            }
        }
        let mut deleted: Vec<String> = snapshot.keys().filter(|r| !seen.contains(*r) && !self.root.join(r).exists()).cloned().collect();
        // reclassement : où sont passés les fichiers ? (même empreinte, nouveau nom / nouveau dossier)
        let mut moves = vec![];
        for rel in &self.files {
            if self.root.join(rel).exists() {
                moves.push(json!({"old": rel, "new": rel, "same": true}));
                continue;
            }
            let fp = fps.get(rel).cloned().flatten();
            let size = fp.as_ref().map(|f| f.split(':').next().unwrap_or("").to_string()).unwrap_or_default();
            let new = by_size.get(&size).and_then(|c| c.iter().find(|p| Self::fingerprint(p) == fp).map(|p| self.rel(p)));
            moves.push(json!({"old": rel, "new": new}));
        }
        let moved: HashSet<String> = moves.iter().filter_map(|m| m["new"].as_str().map(String::from)).collect();
        other.retain(|o| !moved.contains(o));
        modified.sort();
        created.sort();
        deleted.sort();
        other.sort();
        json!({"modified": modified, "created": created, "deleted": deleted, "other": other, "moves": moves})
    }

    pub fn diff(&self, rel: &str) -> Value {
        let in_snap = self.inner.lock().unwrap().snapshot.contains_key(rel);
        let old = if in_snap { fs::read(self.backup_dir.join("files").join(rel)).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default() } else { String::new() };
        let new = fs::read(self.root.join(rel)).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
        json!({"path": rel, "diff": unified_diff(&old, &new), "is_new": !in_snap})
    }

    // ---- 4. annulation
    pub fn undo(&self) -> Value {
        let (changes, ended) = {
            let i = self.inner.lock().unwrap();
            (i.changes.clone(), i.ended)
        };
        let Some(ch) = changes else { return json!({"restored": [], "removed": [], "skipped": []}) };
        let (mut restored, mut removed, mut skipped): (Vec<String>, Vec<String>, Vec<String>) = (vec![], vec![], vec![]);
        let end = ended.unwrap_or_else(now) + 3.0;
        for m in ch["moves"].as_array().cloned().unwrap_or_default() {
            let old = m["old"].as_str().unwrap_or("").to_string();
            if m["same"] == json!(true) {
                continue;
            }
            let Some(newr) = m["new"].as_str() else {
                skipped.push(format!("{} (introuvable)", old));
                continue;
            };
            let (new, oldp) = (self.root.join(newr), self.root.join(&old));
            if new.exists() && !oldp.exists() {
                if let Some(d) = oldp.parent() {
                    let _ = fs::create_dir_all(d);
                }
                if fs::rename(&new, &oldp).is_ok() {
                    restored.push(old);
                    continue;
                }
            }
            skipped.push(newr.to_string());
        }
        let list = |k: &str| ch[k].as_array().cloned().unwrap_or_default().into_iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>();
        for rel in list("modified").into_iter().chain(list("deleted")) {
            let p = self.root.join(&rel);
            if p.exists() && fs::metadata(&p).map(|m| mtime_of(&m) > end).unwrap_or(false) {
                skipped.push(rel); // modifié à nouveau depuis : on ne l'écrase pas
                continue;
            }
            if let Some(d) = p.parent() {
                let _ = fs::create_dir_all(d);
            }
            if fs::copy(self.backup_dir.join("files").join(&rel), &p).is_ok() {
                restored.push(rel);
            } else {
                skipped.push(rel);
            }
        }
        for rel in list("created") {
            let p = self.root.join(&rel);
            if p.exists() && fs::metadata(&p).map(|m| mtime_of(&m) <= end).unwrap_or(false) {
                // mis de côté plutôt que supprimé, au cas où
                let keep = self.backup_dir.join("créés-par-hermes").join(&rel);
                if let Some(d) = keep.parent() {
                    let _ = fs::create_dir_all(d);
                }
                if fs::rename(&p, &keep).or_else(|_| fs::copy(&p, &keep).and_then(|_| fs::remove_file(&p))).is_ok() {
                    removed.push(rel);
                    continue;
                }
            }
            skipped.push(rel);
        }
        self.inner.lock().unwrap().undone = true;
        self.save_manifest();
        json!({"restored": restored, "removed": removed, "skipped": skipped})
    }

    pub fn public(&self, since: usize, esince: usize) -> Value {
        let i = self.inner.lock().unwrap();
        json!({"id": self.id, "state": i.state, "exit_code": i.exit_code, "started": self.started, "ended": i.ended,
            "lines": i.lines.get(since.min(i.lines.len())..).unwrap_or(&[]), "next": i.lines.len(), "changes": i.changes,
            "undone": i.undone, "target": self.target_rel(), "backup": self.backup_dir.to_string_lossy(),
            "events": i.events.get(esince.min(i.events.len())..).unwrap_or(&[]), "enext": i.events.len(), "mode": self.mode})
    }
}

/// Équivalent de difflib.unified_diff (sans les deux lignes d'en-tête).
fn unified_diff(old: &str, new: &str) -> Vec<String> {
    let d = similar::TextDiff::from_lines(old, new);
    let mut out = vec![];
    for group in d.grouped_ops(3) {
        let (first, last) = (group.first().unwrap(), group.last().unwrap());
        let (o1, o2) = (first.old_range().start, last.old_range().end);
        let (n1, n2) = (first.new_range().start, last.new_range().end);
        let rng = |a: usize, b: usize| if b - a == 1 { format!("{}", a + 1) } else if b == a { format!("{},0", a) } else { format!("{},{}", a + 1, b - a) };
        out.push(format!("@@ -{} +{} @@", rng(o1, o2), rng(n1, n2)));
        for op in group {
            for ch in d.iter_changes(&op) {
                let sign = match ch.tag() {
                    similar::ChangeTag::Equal => ' ',
                    similar::ChangeTag::Delete => '-',
                    similar::ChangeTag::Insert => '+',
                };
                out.push(format!("{}{}", sign, ch.value().trim_end_matches('\n')));
            }
        }
    }
    out
}

pub struct Runner {
    pub root: PathBuf,
    pub hermes: Option<PathBuf>,
    extra: Vec<String>,
    job: Mutex<Option<Arc<Job>>>,
    n: Mutex<u32>,
    app: Weak<App>,
}

impl Runner {
    pub fn new(root: PathBuf, cmd: Option<String>, extra_args: &str, app: Weak<App>) -> Self {
        Runner { root, hermes: find_hermes(cmd), extra: shlex::split(extra_args).unwrap_or_default(), job: Mutex::new(None), n: Mutex::new(0), app }
    }

    pub fn busy(&self) -> bool {
        self.job.lock().unwrap().as_ref().map(|j| j.running()).unwrap_or(false)
    }

    pub fn get(&self, jid: &str) -> Option<Arc<Job>> {
        self.job.lock().unwrap().as_ref().filter(|j| j.id == jid).cloned()
    }

    pub fn info(&self, cfg: &Settings) -> Value {
        let mut zones: Vec<String> = fs::read_dir(&self.root)
            .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| cfg.top_allowed(n) && !n.starts_with('.') && Some(n.clone()) != cfg.archive()).collect())
            .unwrap_or_default();
        zones.sort();
        let en = cfg.language() == "en";
        let rp = { let r = cfg.hermes("reclass_profile"); if r.is_empty() { cfg.hermes("profile") } else { r } };
        let last = self.job.lock().unwrap().as_ref().map(|j| {
            let n = j.inner.lock().unwrap().lines.len();
            j.public(n, 0)
        });
        json!({"hermes": self.hermes.as_ref().map(|h| h.to_string_lossy().to_string()), "available": self.hermes.is_some(),
            "running": self.busy(), "last": last,
            "prompt_dir": if en { DEFAULT_PROMPT_DIR_EN } else { DEFAULT_PROMPT_DIR },
            "prompt_file": if en { DEFAULT_PROMPT_FILE_EN } else { DEFAULT_PROMPT_FILE },
            "prompt_reclass": reclass_prompt(cfg, &zones, en), "profiles": config::profiles(),
            "profile": cfg.hermes("profile"), "reclass_profile": rp, "zone_prefix": cfg.zone_prefix(), "archive": cfg.archive()})
    }

    pub fn start(&self, cfg: &Settings, target: &Path, prompt: &str, mode: &str, files: Vec<String>, profile: Option<String>) -> Result<Value, String> {
        let hermes = self.hermes.clone().ok_or("commande hermes introuvable")?;
        if self.busy() {
            return Err("une mise à jour Hermes est déjà en cours".into());
        }
        let n = {
            let mut n = self.n.lock().unwrap();
            *n += 1;
            *n
        };
        let mut cmd = vec![hermes.to_string_lossy().to_string()];
        if let Some(p) = profile.filter(|p| p != "default") {
            cmd.extend(["-p".to_string(), p]);
        }
        cmd.push("chat".into());
        cmd.extend(self.extra.clone());
        cmd.extend(["--query-file".to_string(), "-".to_string()]);
        let reclass = mode == "reclass";
        let cwd = if reclass {
            let wd = cfg.hermes("workdir");
            let wd = wd.trim_matches('/');
            Some(if !wd.is_empty() && self.root.join(wd).is_dir() { self.root.join(wd) } else { self.root.clone() })
        } else {
            None
        };
        let (roots, text_exts, skip): (Vec<PathBuf>, Vec<&'static str>, Vec<String>) = if reclass {
            let mut r: Vec<PathBuf> = fs::read_dir(&self.root)
                .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| {
                    let n = p.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
                    p.is_dir() && cfg.top_allowed(&n) && !n.starts_with('.') && !cfg.ignore().contains(&n)
                }).collect())
                .unwrap_or_default();
            r.sort();
            let mut sk = cfg.hermes_list("backup_skip");
            sk.extend(cfg.ignore());
            (r, vec!["md", "markdown", "yaml", "yml", "jsonl", "json"], sk)
        } else {
            (vec![target.to_path_buf()], vec!["md", "markdown"], vec![])
        };
        let started = now();
        let job = Arc::new(Job {
            id: format!("j{}-{}", n, started as u64),
            root: self.root.clone(),
            target: target.to_path_buf(),
            mode: mode.into(),
            files,
            cwd,
            roots,
            text_exts,
            skip,
            prompt: prompt.into(),
            cmd,
            started,
            backup_dir: backup_root().join(chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string()),
            inner: Mutex::new(Inner { state: "preparing".into(), ..Default::default() }),
            child: Mutex::new(None),
            app: self.app.clone(),
        });
        *self.job.lock().unwrap() = Some(job.clone());
        let j2 = job.clone();
        std::thread::spawn(move || j2.run());
        Ok(job.public(0, 0))
    }
}
