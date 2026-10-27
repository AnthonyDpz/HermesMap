//! Réglages de Hermes Map : un petit fichier config.json, créé par l'écran d'accueil au premier lancement.
//! Même format que la version Python (les deux versions partagent le fichier).

use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const ARCHIVE_NAMES: [&str; 3] = ["archive", "archives", "archivage"];

pub fn config_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/hermes-map")
    } else if cfg!(target_os = "windows") {
        dirs::config_dir().unwrap_or_else(home).join("hermes-map")
    } else {
        dirs::config_dir().unwrap_or_else(|| home().join(".config")).join("hermes-map")
    }
}

pub fn data_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        config_dir()
    } else {
        dirs::data_local_dir().unwrap_or_else(|| home().join(".local/share")).join("hermes-map")
    }
}

pub fn cache_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Caches/hermes-map")
    } else {
        dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("hermes-map")
    }
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.json")
}

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn expand(p: &str) -> PathBuf {
    if p == "~" {
        return home();
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return home().join(rest);
    }
    PathBuf::from(p)
}

pub fn icloud_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        home().join("iCloudDrive")
    } else {
        home().join("Library/Mobile Documents/com~apple~CloudDocs")
    }
}

pub fn hermes_home() -> PathBuf {
    home().join(".hermes")
}

fn defaults() -> Value {
    json!({
        "version": 1,
        "language": "",
        "root": null,
        "include": "*",
        "folders": [],
        "hidden_tops": [],
        "title": "",
        "zone_prefix": "",
        "ignore": [],
        "archive": {"enabled": false, "folder": "Archive", "catalog": "CATALOG.jsonl", "log": "LOG.md"},
        "hermes": {"enabled": true, "command": "", "args": "", "profile": "", "reclass_profile": "",
                    "workdir": "", "conventions": [], "backup_skip": []}
    })
}

pub fn merge(base: &Value, extra: &Value) -> Value {
    let mut out = base.clone();
    if let (Some(o), Some(e)) = (out.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            match (o.get(k), v) {
                (Some(Value::Object(_)), Value::Object(_)) => {
                    let m = merge(&o[k], v);
                    o.insert(k.clone(), m);
                }
                _ => {
                    o.insert(k.clone(), v.clone());
                }
            }
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub data: Value,
}

fn fnmatch(name: &str, pat: &str) -> bool {
    // motifs simples : * et ?
    let n: Vec<char> = name.chars().collect();
    let p: Vec<char> = pat.chars().collect();
    fn rec(n: &[char], p: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|i| rec(&n[i..], &p[1..])),
            Some('?') => !n.is_empty() && rec(&n[1..], &p[1..]),
            Some(c) => n.first() == Some(c) && rec(&n[1..], &p[1..]),
        }
    }
    rec(&n, &p)
}

impl Settings {
    pub fn new(data: &Value) -> Self {
        Settings { data: merge(&defaults(), data) }
    }

    pub fn empty() -> Self {
        Settings::new(&json!({}))
    }

    fn s(&self, k: &str) -> String {
        self.data.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
    }

    fn list(&self, v: Option<&Value>) -> Vec<String> {
        v.and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }

    pub fn root(&self) -> Option<PathBuf> {
        let r = self.s("root");
        if r.is_empty() {
            return None;
        }
        let p = expand(&r);
        Some(fs::canonicalize(&p).unwrap_or(p))
    }

    pub fn language(&self) -> String {
        self.s("language").to_lowercase()
    }

    pub fn include(&self) -> Vec<String> {
        let s = self.s("include");
        let s = s.trim();
        if s.is_empty() || s == "*" {
            return vec![];
        }
        s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
    }

    pub fn title(&self) -> String {
        let t = self.s("title");
        if !t.is_empty() {
            return t;
        }
        self.root()
            .and_then(|r| r.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "Hermes Map".into())
    }

    pub fn zone_prefix(&self) -> String {
        self.s("zone_prefix")
    }

    pub fn ignore(&self) -> Vec<String> {
        self.list(self.data.get("ignore"))
    }

    pub fn zone_name(&self, top: &str) -> String {
        let p = self.zone_prefix();
        if !p.is_empty() && top.starts_with(&p) {
            top[p.len()..].to_string()
        } else {
            top.to_string()
        }
    }

    pub fn top_allowed(&self, name: &str) -> bool {
        if let Some(a) = self.archive() {
            if a == name {
                return true;
            }
        }
        let folders = self.list(self.data.get("folders"));
        if !folders.is_empty() {
            return folders.iter().any(|f| f == name);
        }
        if self.list(self.data.get("hidden_tops")).iter().any(|f| f == name) {
            return false;
        }
        let inc = self.include();
        inc.is_empty() || inc.iter().any(|p| fnmatch(name, p))
    }

    pub fn archive(&self) -> Option<String> {
        let a = &self.data["archive"];
        let on = a.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        let f = a.get("folder").and_then(|v| v.as_str()).unwrap_or("");
        if on && !f.is_empty() {
            Some(f.to_string())
        } else {
            None
        }
    }

    pub fn archive_catalog(&self) -> Option<String> {
        let c = self.data["archive"].get("catalog").and_then(|v| v.as_str()).unwrap_or("CATALOG.jsonl").to_string();
        self.archive().map(|a| format!("{}/{}", a, if c.is_empty() { "CATALOG.jsonl".into() } else { c }))
    }

    pub fn archive_log(&self) -> Option<String> {
        let c = self.data["archive"].get("log").and_then(|v| v.as_str()).unwrap_or("LOG.md").to_string();
        self.archive().map(|a| format!("{}/{}", a, if c.is_empty() { "LOG.md".into() } else { c }))
    }

    pub fn hermes(&self, k: &str) -> String {
        self.data["hermes"].get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
    }

    pub fn hermes_list(&self, k: &str) -> Vec<String> {
        self.list(self.data["hermes"].get(k))
    }

    pub fn hermes_enabled(&self) -> bool {
        self.data["hermes"].get("enabled").and_then(|v| v.as_bool()).unwrap_or(true)
    }

    pub fn to_public(&self) -> Value {
        let mut d = self.data.clone();
        if let Some(o) = d.as_object_mut() {
            o.insert("configured".into(), json!(self.root().is_some()));
            o.insert("config_file".into(), json!(config_file().to_string_lossy()));
            o.insert("archive_dir".into(), json!(self.archive()));
        }
        d
    }

    pub fn save(&self) -> std::io::Result<()> {
        write_json(&config_file(), &self.data)
    }
}

pub fn write_json(path: &Path, v: &Value) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_string_pretty(v).unwrap_or_default())?;
    fs::rename(&tmp, path)
}

pub fn load() -> Option<Settings> {
    let txt = fs::read_to_string(config_file()).ok()?;
    let v: Value = serde_json::from_str(&txt).ok()?;
    Some(Settings::new(&v))
}

pub fn save_language(lg: &str) -> std::io::Result<()> {
    let mut data: Value = fs::read_to_string(config_file())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    if !data.is_object() {
        data = json!({});
    }
    data["language"] = json!(lg);
    write_json(&config_file(), &data)
}

// ---------------------------------------------------------------- détection (écran d'accueil)

pub fn profiles() -> Vec<String> {
    let base = hermes_home().join("profiles");
    let marks = ["config.yaml", ".env", "SOUL.md", "profile.yaml", "auth.json", "state.db"];
    let mut out: Vec<String> = fs::read_dir(base)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir() && marks.iter().any(|m| e.path().join(m).exists()))
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

pub fn inspect(path: &str) -> Value {
    let p = expand(path);
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string_lossy().to_string());
    let mut out = Map::new();
    out.insert("path".into(), json!(p.to_string_lossy()));
    out.insert("name".into(), json!(name));
    if !p.is_dir() {
        out.insert("access".into(), json!("missing"));
        return Value::Object(out);
    }
    let rd = match fs::read_dir(&p) {
        Ok(r) => r,
        Err(e) => {
            let denied = e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(1);
            out.insert("access".into(), json!(if denied { "denied" } else { "missing" }));
            return Value::Object(out);
        }
    };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    out.insert("access".into(), json!("ok"));
    out.insert("dirs".into(), json!(names.len()));
    // préfixe partagé par plusieurs dossiers (ex. N_Pro, N_Perso… -> « N_ »)
    let re = regex::Regex::new(r"^([A-Za-z0-9]{1,4}[_\-.])\S").unwrap();
    let mut groups: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for n in &names {
        if let Some(m) = re.captures(n) {
            groups.entry(m[1].to_string()).or_default().push(n.clone());
        }
    }
    let (mut prefix, mut members) = (String::new(), Vec::new());
    if let Some((k, v)) = groups.iter().max_by_key(|(_, v)| v.len()) {
        prefix = k.clone();
        members = v.clone();
        if members.len() < 3 || ((members.len() as f64) < 0.15 * names.len() as f64 && names.len() > 20) {
            prefix.clear();
            members.clear();
        }
    }
    let shown = if members.is_empty() { names.clone() } else { members.clone() };
    out.insert("all".into(), json!(names.iter().take(500).collect::<Vec<_>>()));
    out.insert("tops".into(), json!(shown.iter().take(24).collect::<Vec<_>>()));
    out.insert("more".into(), json!(shown.len().saturating_sub(24)));
    out.insert(
        "suggest".into(),
        json!({"include": if prefix.is_empty() { "*".to_string() } else { format!("{}*", prefix) }, "zone_prefix": prefix}),
    );
    let arch = shown.iter().find(|n| {
        let z = if !prefix.is_empty() && n.starts_with(&prefix) { &n[prefix.len()..] } else { n.as_str() };
        ARCHIVE_NAMES.contains(&z.to_lowercase().as_str())
    });
    out.insert("archive_found".into(), json!(arch));
    out.insert("archive_default".into(), json!(arch.cloned().unwrap_or_else(|| format!("{}Archive", prefix))));
    out.insert("obsidian".into(), json!(p.join(".obsidian").is_dir()));
    let hh = hermes_home();
    out.insert(
        "hermes_home".into(),
        json!(hh.exists() && fs::canonicalize(&p).ok() == fs::canonicalize(&hh).ok()),
    );
    Value::Object(out)
}

pub fn obsidian_vaults() -> Vec<String> {
    let f = if cfg!(target_os = "macos") {
        home().join("Library/Application Support/obsidian/obsidian.json")
    } else {
        dirs::config_dir().unwrap_or_else(home).join("obsidian/obsidian.json")
    };
    let v: Value = match fs::read_to_string(f).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(v) => v,
        None => return vec![],
    };
    v.get("vaults")
        .and_then(|x| x.as_object())
        .map(|o| o.values().filter_map(|x| x.get("path").and_then(|p| p.as_str()).map(String::from)).collect())
        .unwrap_or_default()
}

pub fn detect(hermes_cmd: Option<String>) -> Value {
    let hh = hermes_home();
    let mut cands: Vec<(String, String, String)> = vec![];
    let ic = icloud_dir();
    if ic.exists() {
        cands.push(("icloud".into(), "iCloud Drive".into(), ic.to_string_lossy().to_string()));
    }
    for v in obsidian_vaults() {
        let n = Path::new(&v).file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
        cands.push(("obsidian".into(), format!("Coffre Obsidian « {} »", n), v));
    }
    if hh.is_dir() {
        cands.push(("hermes".into(), "Dossier de Hermes (~/.hermes)".into(), hh.to_string_lossy().to_string()));
    }
    let docs = dirs::document_dir().unwrap_or_else(|| home().join("Documents"));
    if docs.is_dir() {
        cands.push(("documents".into(), "Documents".into(), docs.to_string_lossy().to_string()));
    }
    let mut seen = std::collections::HashSet::new();
    let mut list = vec![];
    for (kind, label, path) in cands {
        if !seen.insert(path.clone()) {
            continue;
        }
        let mut info = inspect(&path);
        info["kind"] = json!(kind);
        info["label"] = json!(label);
        list.push(info);
    }
    json!({
        "hermes": {"command": hermes_cmd, "home": if hh.is_dir() { Some(hh.to_string_lossy().to_string()) } else { None },
                    "profiles": profiles()},
        "candidates": list,
        "platform": std::env::consts::OS,
        "can_choose": true
    })
}

const ARCHIVE_README: &str = "# Archive

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
";

const ARCHIVE_README_EN: &str = "# Archive

This folder is managed with **Hermes Map**: it gathers the “heavy” files (images, videos, PDFs,
documents…) so that the other folders only contain notes.

## Filing rule

- Notes (.md) stay in their folder, for example `{exemple_note}`.
- Their files are stored in the archive's **mirror** folder:
  `{exemple_miroir}` (same path, under the zone name in lowercase).
- Hermes Map shows both together: opening the note's folder also shows its archived files.

## Tracking files

- `{catalog}`: one JSON entry per line (`rel` = path from this folder, `note`, `title`…).
- `{log}`: log of moves, as a table
  `| date ISO | action | from | to | ancien nom | nouveau nom | bytes | note |`.
";

pub const LOG_HEADER: &str = "# Journal des déplacements\n\n| date ISO | action | from | to | ancien nom | nouveau nom | bytes | note |\n|---|---|---|---|---|---|---|---|\n";

/// Crée le dossier d'archivage et ses fichiers de suivi s'il n'existe pas (ne remplace rien).
pub fn init_archive(s: &Settings) -> std::io::Result<Vec<String>> {
    let (arch, root) = match (s.archive(), s.root()) {
        (Some(a), Some(r)) => (a, r),
        _ => return Ok(vec![]),
    };
    let d = root.join(&arch);
    if d.exists() {
        return Ok(vec![]);
    }
    fs::create_dir_all(&d)?;
    let mut made = vec![arch.clone()];
    let mut tops: Vec<String> = fs::read_dir(&root)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| s.top_allowed(n) && *n != arch)
        .collect();
    tops.sort();
    let ex = tops.first().cloned().unwrap_or_else(|| "Projets".into());
    let zone = s.zone_name(&ex).to_lowercase();
    let en = s.language() == "en";
    let txt = if en { ARCHIVE_README_EN } else { ARCHIVE_README }
        .replace("{exemple_note}", &format!("{}/{}.md", ex, if en { "trips/japan" } else { "voyages/japon" }))
        .replace("{exemple_miroir}", &format!("{}/{}/{}/photo.jpg", arch, zone, if en { "trips" } else { "voyages" }))
        .replace("{catalog}", s.data["archive"]["catalog"].as_str().unwrap_or("CATALOG.jsonl"))
        .replace("{log}", s.data["archive"]["log"].as_str().unwrap_or("LOG.md"));
    fs::write(d.join("README.md"), txt)?;
    made.push(format!("{}/README.md", arch));
    if let Some(l) = s.archive_log() {
        fs::write(root.join(&l), LOG_HEADER)?;
        made.push(l);
    }
    if let Some(c) = s.archive_catalog() {
        fs::write(root.join(&c), "")?;
        made.push(c);
    }
    Ok(made)
}
