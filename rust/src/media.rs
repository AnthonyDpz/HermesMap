//! Types de fichiers, miniatures, vidéos (conversion ffmpeg en cache), aperçu Word,
//! ouverture dans l'app du système et sélecteur de dossier — pour Mac, Windows et Linux.

use crate::config;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

pub const WEB_VIDEO: &[&str] = &["mp4", "m4v", "webm", "mov", "ogv"];
pub const OTHER_VIDEO: &[&str] = &[
    "avi", "mkv", "wmv", "flv", "mpg", "mpeg", "m2v", "3gp", "3g2", "ts", "mts", "m2ts", "vob", "divx", "xvid", "rm",
    "rmvb", "asf", "f4v",
];
pub const OFFICE: &[&str] = &[
    "doc", "docx", "rtf", "rtfd", "odt", "pages", "xls", "xlsx", "ods", "numbers", "ppt", "pptx", "odp", "key", "csv",
    "epub",
];
pub const TEXTUTIL: &[&str] = &["doc", "docx", "rtf", "rtfd", "odt", "webarchive", "wordml"];
pub const TEXT: &[&str] = &[
    "md", "markdown", "txt", "yaml", "yml", "json", "jsonl", "toml", "ini", "cfg", "conf", "py", "sh", "zsh", "bash",
    "js", "ts", "mjs", "css", "html", "htm", "xml", "csv", "tsv", "log", "env", "rs", "go", "rb", "lua", "sql", "j2",
    "jinja", "tmpl", "rst", "org", "tex", "svg", "gitignore", "lock", "service", "plist", "url", "webloc",
];
pub const IMAGE: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg", "avif", "heic", "tif", "tiff"];
pub const AUDIO: &[&str] = &["mp3", "wav", "ogg", "m4a", "flac", "aac", "opus"];
pub const DB: &[&str] = &["db", "sqlite", "sqlite3"];
pub const SECRET_NAMES: &[&str] = &[".env", "auth.json", "credentials.json", "secrets.json", "tokens.json"];
pub const ACTIVE: &[&str] = &["html", "htm", "xhtml", "shtml", "svg", "svgz", "xml", "xsl", "xslt", "mht", "mhtml"];

/// Extension au sens de Python (pathlib.suffix) : vide pour « .env » ou « Makefile ».
pub fn suffix(name: &str) -> String {
    let n = name.trim_start_matches('.');
    if n.len() < name.len() && !n.contains('.') {
        return String::new();
    }
    match name.rfind('.') {
        Some(i) if i > 0 => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

pub fn icloud_real_name(name: &str) -> Option<String> {
    if name.starts_with('.') && name.ends_with(".icloud") && name.len() > 8 {
        Some(name[1..name.len() - 7].to_string())
    } else {
        None
    }
}

pub fn kind_of_name(name: &str, is_dir: bool) -> String {
    if icloud_real_name(name).is_some() {
        return "icloud".into();
    }
    if is_dir {
        return "dir".into();
    }
    let low = name.to_lowercase();
    let ext = suffix(&low);
    let e = ext.as_str();
    if SECRET_NAMES.contains(&low.as_str()) || low.starts_with(".env") {
        return "secret".into();
    }
    if IMAGE.contains(&e) {
        return "image".into();
    }
    if e == "pdf" {
        return "pdf".into();
    }
    if AUDIO.contains(&e) {
        return "audio".into();
    }
    if WEB_VIDEO.contains(&e) || OTHER_VIDEO.contains(&e) {
        return "video".into();
    }
    if DB.contains(&e) {
        return "db".into();
    }
    if OFFICE.contains(&e) {
        return "office".into();
    }
    if e == "md" || e == "markdown" {
        return "markdown".into();
    }
    if e == "json" || e == "jsonl" {
        return "json".into();
    }
    if ["yaml", "yml", "toml", "ini", "cfg", "conf"].contains(&e) {
        return "config".into();
    }
    if TEXT.contains(&e) || e.is_empty() {
        return "text".into();
    }
    "binary".into()
}

pub fn kind_of(p: &Path) -> String {
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    kind_of_name(&name, p.is_dir())
}

pub fn is_texty(p: &Path) -> bool {
    let k = kind_of(p);
    if ["markdown", "json", "config", "text", "secret"].contains(&k.as_str()) {
        if let Ok(mut f) = fs::File::open(p) {
            use std::io::Read;
            let mut buf = vec![0u8; 4096];
            let n = f.read(&mut buf).unwrap_or(0);
            return !buf[..n].contains(&0);
        }
    }
    false
}

/// macOS : fichier iCloud présent dans le cloud mais pas sur le disque.
pub fn is_dataless(md: &fs::Metadata) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        return md.st_flags() & 0x4000_0000 != 0;
    }
    #[allow(unreachable_code)]
    {
        let _ = md;
        false
    }
}

pub fn mtime(md: &fs::Metadata) -> f64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn key_of(p: &Path) -> String {
    let md = fs::metadata(p).ok();
    let (size, mt) = md.map(|m| (m.len(), mtime(&m))).unwrap_or((0, 0.0));
    let mut h = sha1_smol::Sha1::new();
    h.update(format!("{}|{}|{}", p.to_string_lossy(), size, mt).as_bytes());
    h.digest().to_string()
}

pub fn which(cmd: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        vec!["".into(), ".exe".into(), ".cmd".into(), ".bat".into()]
    } else {
        vec!["".into()]
    };
    for d in std::env::split_paths(&path) {
        for e in &exts {
            let c = d.join(format!("{}{}", cmd, e));
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

pub fn find_ffmpeg() -> Option<PathBuf> {
    if let Some(p) = which("ffmpeg") {
        return Some(p);
    }
    for c in ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"] {
        if Path::new(c).exists() {
            return Some(PathBuf::from(c));
        }
    }
    let h = config::home().join("bin/ffmpeg");
    if h.exists() {
        return Some(h);
    }
    None
}

// ---------------------------------------------------------------- miniatures

pub const THUMB_SIZE: u32 = 360;
static THUMB_LOCK: once_cell::sync::Lazy<Mutex<()>> = once_cell::sync::Lazy::new(|| Mutex::new(()));

fn quiet(cmd: &mut Command) -> bool {
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // pas de fenêtre de console
    }
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

/// Miniature JPEG/PNG en cache. Renvoie le chemin, ou None si impossible.
pub fn make_thumb(p: &Path, kind: &str) -> Option<PathBuf> {
    let dir = config::cache_dir().join("thumbs");
    let out = dir.join(format!("{}.jpg", key_of(p)));
    let ok = |o: &Path| o.metadata().map(|m| m.len() > 0).unwrap_or(false);
    if ok(&out) {
        return Some(out);
    }
    let _g = THUMB_LOCK.lock().ok();
    if ok(&out) {
        return Some(out);
    }
    let _ = fs::create_dir_all(&dir);
    let ext = suffix(&p.file_name()?.to_string_lossy().to_lowercase());
    if kind == "image" && ["png", "jpg", "jpeg", "gif", "webp", "bmp"].contains(&ext.as_str()) {
        if let Ok(img) = image::open(p) {
            let t = img.thumbnail(THUMB_SIZE, THUMB_SIZE).to_rgb8();
            if t.save_with_format(&out, image::ImageFormat::Jpeg).is_ok() && ok(&out) {
                return Some(out);
            }
        }
    }
    if cfg!(target_os = "macos") {
        if kind == "image" {
            quiet(Command::new("sips").args(["-s", "format", "jpeg", "-Z", &THUMB_SIZE.to_string()]).arg(p).arg("--out").arg(&out));
        } else if ["video", "pdf", "office", "text", "markdown", "json", "config"].contains(&kind) {
            // Quick Look : la même vignette que dans le Finder
            let tmp = std::env::temp_dir().join(format!("hm-{}", key_of(p)));
            let _ = fs::create_dir_all(&tmp);
            quiet(Command::new("qlmanage").args(["-t", "-s", &THUMB_SIZE.to_string(), "-o"]).arg(&tmp).arg(p));
            if let Ok(rd) = fs::read_dir(&tmp) {
                for e in rd.flatten() {
                    if e.path().extension().map(|x| x == "png").unwrap_or(false) {
                        let _ = fs::rename(e.path(), &out).or_else(|_| fs::copy(e.path(), &out).map(|_| ()));
                        break;
                    }
                }
            }
            let _ = fs::remove_dir_all(&tmp);
        }
    }
    if kind == "video" && !ok(&out) {
        if let Some(ff) = find_ffmpeg() {
            quiet(
                Command::new(ff)
                    .args(["-hide_banner", "-loglevel", "error", "-y", "-ss", "1", "-i"])
                    .arg(p)
                    .args(["-frames:v", "1", "-vf", &format!("scale={}:-2", THUMB_SIZE)])
                    .arg(&out),
            );
        }
    }
    if ok(&out) {
        Some(out)
    } else {
        None
    }
}

// ---------------------------------------------------------------- vidéos

pub struct VideoConverter {
    pub ffmpeg: Option<PathBuf>,
    jobs: Mutex<HashMap<String, Arc<Mutex<Value>>>>,
    sem: Arc<Mutex<()>>,
}

impl VideoConverter {
    pub fn new() -> Self {
        VideoConverter { ffmpeg: find_ffmpeg(), jobs: Mutex::new(HashMap::new()), sem: Arc::new(Mutex::new(())) }
    }

    fn cache() -> PathBuf {
        config::cache_dir().join("videos")
    }

    pub fn cached_file(&self, p: &Path) -> Option<PathBuf> {
        let out = Self::cache().join(format!("{}.mp4", key_of(p)));
        if out.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            Some(out)
        } else {
            None
        }
    }

    pub fn status(&self, p: &Path) -> Value {
        let ext = suffix(&p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
        if WEB_VIDEO.contains(&ext.as_str()) {
            return json!({"state": "native"});
        }
        if self.cached_file(p).is_some() {
            return json!({"state": "ready"});
        }
        let ff = match &self.ffmpeg {
            Some(f) => f.clone(),
            None => return json!({"state": "no-ffmpeg"}),
        };
        let key = key_of(p);
        let job = {
            let mut jobs = self.jobs.lock().unwrap();
            if let Some(j) = jobs.get(&key) {
                j.clone()
            } else {
                let j = Arc::new(Mutex::new(json!({"state": "converting", "progress": 0.0})));
                jobs.insert(key.clone(), j.clone());
                let (src, out, jj, sem) = (p.to_path_buf(), Self::cache().join(format!("{}.mp4", key)), j.clone(), self.sem.clone());
                std::thread::spawn(move || convert(ff, src, out, jj, sem));
                j
            }
        };
        let v = job.lock().unwrap().clone();
        v
    }
}

fn duration(ff: &Path, src: &Path) -> Option<f64> {
    let o = Command::new(ff).args(["-hide_banner", "-i"]).arg(src).output().ok()?;
    let s = String::from_utf8_lossy(&o.stderr);
    let re = regex::Regex::new(r"Duration:\s*(\d+):(\d+):(\d+\.?\d*)").ok()?;
    let c = re.captures(&s)?;
    Some(c[1].parse::<f64>().ok()? * 3600.0 + c[2].parse::<f64>().ok()? * 60.0 + c[3].parse::<f64>().ok()?)
}

fn convert(ff: PathBuf, src: PathBuf, out: PathBuf, job: Arc<Mutex<Value>>, sem: Arc<Mutex<()>>) {
    let _g = sem.lock();
    let _ = fs::create_dir_all(out.parent().unwrap());
    let tmp = out.with_extension("part.mp4");
    let dur = duration(&ff, &src);
    let attempts: [&[&str]; 2] = [
        &["-c:v", "copy", "-c:a", "aac", "-b:a", "160k"],
        &["-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "160k"],
    ];
    for args in attempts {
        let child = Command::new(&ff)
            .args(["-hide_banner", "-y", "-i"])
            .arg(&src)
            .args(["-map", "0:v:0", "-map", "0:a:0?"])
            .args(args)
            .args(["-movflags", "+faststart", "-progress", "pipe:1", "-nostats"])
            .arg(&tmp)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                *job.lock().unwrap() = json!({"state": "error", "error": e.to_string()});
                return;
            }
        };
        if let Some(so) = child.stdout.take() {
            for line in BufReader::new(so).lines().map_while(Result::ok) {
                if let (Some(v), Some(d)) = (line.strip_prefix("out_time_ms="), dur) {
                    if let Ok(us) = v.trim().parse::<f64>() {
                        job.lock().unwrap()["progress"] = json!((us / 1e6 / d).min(0.99));
                    }
                }
            }
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        if ok && tmp.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            if args[1] == "copy" && !probe_ok(&ff, &tmp) {
                continue; // codec incompatible avec le navigateur : on réencode
            }
            let _ = fs::rename(&tmp, &out);
            *job.lock().unwrap() = json!({"state": "ready", "progress": 1.0});
            return;
        }
    }
    let _ = fs::remove_file(&tmp);
    *job.lock().unwrap() = json!({"state": "error", "error": "ffmpeg n'a pas pu convertir cette vidéo"});
}

fn probe_ok(ff: &Path, mp4: &Path) -> bool {
    Command::new(ff)
        .args(["-hide_banner", "-i"])
        .arg(mp4)
        .output()
        .map(|o| {
            let s = String::from_utf8_lossy(&o.stderr);
            regex::Regex::new(r"Video:\s*(h264|hevc)").unwrap().is_match(&s)
        })
        .unwrap_or(false)
}

// ---------------------------------------------------------------- documents

/// Word / RTF / ODT -> HTML. Mac : textutil. Ailleurs : texte brut extrait des .docx / .odt.
pub fn doc_html(p: &Path) -> Option<String> {
    let ext = suffix(&p.file_name()?.to_string_lossy().to_lowercase());
    if !TEXTUTIL.contains(&ext.as_str()) {
        return None;
    }
    let dir = config::cache_dir().join("docs");
    let _ = fs::create_dir_all(&dir);
    let out = dir.join(format!("{}.html", key_of(p)));
    if !out.exists() {
        if cfg!(target_os = "macos") {
            quiet(Command::new("textutil").args(["-convert", "html", "-output"]).arg(&out).arg(p));
        } else if ext == "docx" || ext == "odt" {
            let html = office_text_html(p, &ext)?;
            let _ = fs::write(&out, html);
        }
    }
    fs::read_to_string(&out).ok()
}

/// Lecture minimale d'un .docx / .odt (archive zip) : paragraphes en texte brut.
fn office_text_html(p: &Path, ext: &str) -> Option<String> {
    let data = fs::read(p).ok()?;
    let inner = if ext == "docx" { "word/document.xml" } else { "content.xml" };
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(data)).ok()?;
    let mut xml = Vec::new();
    {
        use std::io::Read;
        z.by_name(inner).ok()?.read_to_end(&mut xml).ok()?;
    }
    let xml = String::from_utf8_lossy(&xml);
    let para = if ext == "docx" { regex::Regex::new(r"</w:p>").unwrap() } else { regex::Regex::new(r"</text:p>|</text:h>").unwrap() };
    let tag = regex::Regex::new(r"<[^>]+>").unwrap();
    let mut out = String::from("<html><body>");
    for chunk in para.split(&xml) {
        let t = tag.replace_all(chunk, "");
        let t = t.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'");
        if !t.trim().is_empty() {
            out.push_str("<p>");
            out.push_str(&crate::util::html_escape(t.trim()));
            out.push_str("</p>");
        }
    }
    out.push_str("</body></html>");
    Some(out)
}

// ---------------------------------------------------------------- système

/// Ouvre le fichier avec l'app par défaut (ou le montre dans le Finder / l'Explorateur).
pub fn open_in_app(p: &Path, reveal: bool) -> Result<(), String> {
    let r = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        if reveal {
            c.arg("-R");
        }
        c.arg(p).spawn()
    } else if cfg!(windows) {
        if reveal {
            Command::new("explorer").arg(format!("/select,{}", p.to_string_lossy())).spawn()
        } else {
            Command::new("cmd").args(["/C", "start", ""]).arg(p).spawn()
        }
    } else {
        let target = if reveal { p.parent().unwrap_or(p).to_path_buf() } else { p.to_path_buf() };
        Command::new("xdg-open").arg(target).spawn()
    };
    r.map(|_| ()).map_err(|e| e.to_string())
}

pub fn open_url(url: &str) {
    let _ = if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else if cfg!(windows) {
        Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };
}

/// Fenêtre « Choisir un dossier » du système. Renvoie {"path": …} ou {"cancel": true}.
pub fn choose_folder(prompt: &str) -> Value {
    let safe = prompt.replace(['\\', '"', '\''], " ");
    let out = if cfg!(target_os = "macos") {
        Command::new("osascript")
            .args(["-e", &format!("activate\nPOSIX path of (choose folder with prompt \"{}\")", safe)])
            .output()
    } else if cfg!(windows) {
        let ps = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
             $d.Description = '{}'; if ($d.ShowDialog() -eq 'OK') {{ Write-Output $d.SelectedPath }} else {{ exit 1 }}",
            safe
        );
        Command::new("powershell").args(["-NoProfile", "-STA", "-Command", &ps]).output()
    } else if which("zenity").is_some() {
        Command::new("zenity").args(["--file-selection", "--directory", "--title", &safe]).output()
    } else {
        return json!({"error": "sélecteur de dossier indisponible : colle le chemin à la main"});
    };
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let s = if s.len() > 1 { s.trim_end_matches(['/', '\\']).to_string() } else { s };
            if s.is_empty() {
                json!({"cancel": true})
            } else {
                json!({"path": s})
            }
        }
        Ok(_) => json!({"cancel": true}),
        Err(e) => json!({"error": e.to_string()}),
    }
}

/// Mac : ouvre Réglages Système > Accès complet au disque (et montre l'app dans le Finder).
pub fn open_privacy_settings(app_path: Option<String>) {
    if !cfg!(target_os = "macos") {
        return;
    }
    if let Some(a) = app_path {
        if Path::new(&a).exists() {
            let _ = Command::new("open").args(["-R", &a]).spawn();
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    }
    let _ = Command::new("open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles").spawn();
}

