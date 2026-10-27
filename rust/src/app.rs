//! État de l'application et toutes les routes HTTP (même API que la version Python).

use crate::assets::Static;
use crate::config::{self, Settings};
use crate::hermes::Runner;
use crate::i18n;
use crate::media::{self, icloud_real_name, is_dataless, kind_of, kind_of_name, mtime, VideoConverter};
use crate::mover;
use crate::scan::{href_target, Scanner, MDLINK_RE, WIKILINK_RE, SCAN_TEXT_LIMIT};
use crate::util::{basename, dirname, norm_text, now, rel_of};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tiny_http::{Header, Request, Response, StatusCode};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_TEXT: u64 = 2_000_000;

static INDEX_CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob: https:; media-src 'self' blob:; font-src 'self' data:; connect-src 'self'; \
frame-src 'self'; worker-src 'self' blob:; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
static SECRET_KEY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(key|token|secret|password|passwd|pwd|auth|bearer|cookie|session_id|private)").unwrap());

#[derive(Clone, Default)]
pub struct Opts {
    pub all: bool,
    pub hidden: bool,
    pub hermes: Option<String>,
    pub hermes_args: String,
    pub no_hermes: bool,
    pub reveal: bool,
}

pub struct State {
    pub root: Option<PathBuf>,
    pub scanner: Option<Arc<Scanner>>,
    pub runner: Option<Arc<Runner>>,
    pub cfg: Settings,
    pub setup: Option<Value>,
}

pub struct App {
    pub opts: Opts,
    pub token: String,
    pub started: u64,
    pub port: AtomicUsize,
    pub st: RwLock<State>,
    cache: Mutex<Option<Value>>,
    status: Mutex<String>,
    scanning: AtomicBool,
    gen: AtomicUsize,
    pub last_ping: Mutex<f64>,
    pub video: VideoConverter,
}

pub enum Out {
    Json(Value, u16),
    File(PathBuf, Option<String>),
    Html(String, Vec<(String, String)>),
    Bytes(Vec<u8>, String),
}

fn j(v: Value) -> Out {
    Out::Json(v, 200)
}
fn jerr(msg: &str, code: u16) -> Out {
    Out::Json(json!({"error": msg}), code)
}

impl App {
    pub fn new(opts: Opts) -> Arc<App> {
        Arc::new(App {
            opts,
            token: crate::util::token(),
            started: now() as u64,
            port: AtomicUsize::new(0),
            st: RwLock::new(State { root: None, scanner: None, runner: None, cfg: Settings::empty(), setup: Some(json!({"reason": "first"})) }),
            cache: Mutex::new(None),
            status: Mutex::new("Démarrage…".into()),
            scanning: AtomicBool::new(false),
            gen: AtomicUsize::new(0),
            last_ping: Mutex::new(0.0),
            video: VideoConverter::new(),
        })
    }

    pub fn cfg(&self) -> Settings {
        self.st.read().unwrap().cfg.clone()
    }
    pub fn root(&self) -> Option<PathBuf> {
        self.st.read().unwrap().root.clone()
    }
    pub fn scanner(&self) -> Option<Arc<Scanner>> {
        self.st.read().unwrap().scanner.clone()
    }
    pub fn runner(&self) -> Option<Arc<Runner>> {
        self.st.read().unwrap().runner.clone()
    }
    pub fn busy(&self) -> bool {
        self.runner().map(|r| r.busy()).unwrap_or(false)
    }

    /// Applique des réglages : dossier exploré, archive, Hermes. Renvoie None ou la raison d'un échec.
    pub fn configure(self: &Arc<Self>, cfg: Settings) -> Option<String> {
        if self.busy() {
            return Some("busy".into());
        }
        i18n::set_lang(&cfg.language());
        let root = match cfg.root() {
            Some(r) => r,
            None => {
                let mut st = self.st.write().unwrap();
                st.cfg = cfg;
                st.setup = Some(json!({"reason": "first"}));
                return Some("first".into());
            }
        };
        if let Err(e) = fs::read_dir(&root) {
            let denied = e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(1);
            let reason = if denied { "denied" } else { "missing" };
            let mut st = self.st.write().unwrap();
            st.cfg = cfg;
            st.setup = Some(json!({"reason": reason, "path": root.to_string_lossy()}));
            return Some(reason.into());
        }
        let scanner = Arc::new(Scanner::new(root.clone(), self.opts.all, self.opts.hidden, cfg.clone()));
        let runner = if !self.opts.no_hermes && cfg.hermes_enabled() {
            let cmd = self.opts.hermes.clone().or_else(|| Some(cfg.hermes("command")).filter(|s| !s.is_empty()));
            let args = if self.opts.hermes_args.is_empty() { cfg.hermes("args") } else { self.opts.hermes_args.clone() };
            Some(Arc::new(Runner::new(root.clone(), cmd, &args, Arc::downgrade(self))))
        } else {
            None
        };
        {
            let mut st = self.st.write().unwrap();
            st.root = Some(root.clone());
            st.scanner = Some(scanner.clone());
            st.runner = runner.clone();
            st.cfg = cfg;
            st.setup = None;
        }
        *self.cache.lock().unwrap() = None;
        self.gen.fetch_add(1, Ordering::SeqCst);
        let mut tops: Vec<String> = fs::read_dir(&root)
            .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| scanner.top_allowed(n)).collect())
            .unwrap_or_default();
        tops.sort();
        println!("Dossier : {}\nDossiers retenus ({}) : {}", root.display(), tops.len(), if tops.is_empty() { "aucun !".into() } else { tops.join(", ") });
        if let Some(r) = &runner {
            println!("Hermes : {}", r.hermes.as_ref().map(|h| format!("commande {}", h.display())).unwrap_or("commande « hermes » introuvable".into()));
        }
        self.start_scan();
        None
    }

    pub fn start_scan(self: &Arc<Self>) {
        if self.scanning.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = self.clone();
        std::thread::spawn(move || loop {
            let gen = app.gen.load(Ordering::SeqCst);
            if let Some(sc) = app.scanner() {
                let a2 = app.clone();
                let a3 = app.clone();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    sc.scan(
                        &move |res| {
                            if a2.gen.load(Ordering::SeqCst) == gen {
                                *a2.cache.lock().unwrap() = Some(res);
                            }
                        },
                        &move |msg| *a3.status.lock().unwrap() = msg,
                    )
                }));
                *app.status.lock().unwrap() = if r.is_ok() { "Terminé".into() } else { "Erreur pendant l'analyse : panic".into() };
            }
            if app.gen.load(Ordering::SeqCst) == gen {
                app.scanning.store(false, Ordering::SeqCst);
                break; // sinon : réglages changés pendant l'analyse, on recommence avec les nouveaux
            }
        });
    }

    fn graph(self: &Arc<Self>, refresh: bool) -> Value {
        if let Some(s) = &self.st.read().unwrap().setup {
            let mut v = json!({"setup": true});
            if let Some(o) = s.as_object() {
                for (k, x) in o {
                    v[k] = x.clone();
                }
            }
            return v;
        }
        if refresh {
            self.start_scan();
        }
        let status = self.status.lock().unwrap().clone();
        match self.cache.lock().unwrap().as_ref() {
            None => json!({"pending": true, "status": status}),
            Some(c) => {
                let mut v = c.clone();
                v["scanning"] = json!(self.scanning.load(Ordering::SeqCst));
                v["status"] = json!(status);
                v
            }
        }
    }

    /// Liens et tags contenus dans une note (pour les afficher en direct pendant que Hermes écrit).
    pub fn links_of(&self, rel: &str) -> (Vec<String>, Vec<String>) {
        let (Some(root), Some(sc)) = (self.root(), self.scanner()) else { return (vec![], vec![]) };
        let mut raw = vec![];
        if let Ok(f) = fs::File::open(root.join(rel)) {
            let _ = f.take(SCAN_TEXT_LIMIT).read_to_end(&mut raw);
        } else {
            return (vec![], vec![]);
        }
        let txt = String::from_utf8_lossy(&raw);
        let data = sc.data.read().unwrap();
        let mut out = BTreeSet::new();
        for m in WIKILINK_RE.captures_iter(&txt) {
            let t = m[1].trim().to_lowercase();
            let mut c: Vec<String> = data.name_map.get(&t).cloned().unwrap_or_default();
            c.extend(data.name_map.get(&format!("{}.md", t)).cloned().unwrap_or_default());
            for x in c.into_iter().take(3) {
                if x != rel {
                    out.insert(x);
                }
            }
        }
        let parent = dirname(rel);
        for m in MDLINK_RE.captures_iter(&txt) {
            if let Some(c) = href_target(&sc.root_prefix(), &parent, &m[1]) {
                if data.index_paths.contains(&c) && c != rel {
                    out.insert(c);
                }
            }
        }
        let low = rel.to_lowercase();
        let tags = crate::scan::extract_tags(&txt, low.ends_with(".md") || low.ends_with(".markdown"));
        (out.into_iter().take(60).collect(), tags.into_iter().collect())
    }

    /// Chemin réel d'un élément, refusé s'il sort du dossier exploré ou des dossiers suivis.
    pub fn safe(&self, rel: &str) -> Result<PathBuf, String> {
        let (Some(root), Some(sc)) = (self.root(), self.scanner()) else { return Err("aucun dossier configuré".into()) };
        let joined = root.join(rel.trim_start_matches('/'));
        let p = fs::canonicalize(&joined).unwrap_or_else(|_| PathBuf::from(crate::util::normpath(&joined.to_string_lossy())));
        if !p.starts_with(&root) {
            return Err("chemin hors du dossier".into());
        }
        if !sc.allowed(&p) {
            return Err("hors des dossiers autorisés".into());
        }
        Ok(p)
    }

    // ------------------------------------------------------------------ requêtes

    pub fn handle(self: &Arc<Self>, mut req: Request) {
        let url = req.url().to_string();
        let (path, query) = match url.find('?') {
            Some(i) => (url[..i].to_string(), url[i + 1..].to_string()),
            None => (url.clone(), String::new()),
        };
        let q = parse_query(&query);
        let qv = |k: &str| q.iter().find(|(a, _)| a == k).map(|(_, b)| b.clone()).unwrap_or_default();
        let host_ok = self.host_ok(&req);
        let method = req.method().as_str().to_string();
        let out = if !host_ok {
            jerr("adresse refusée", 403)
        } else if method == "POST" {
            let mut body = String::new();
            let _ = req.as_reader().take(50_000_000).read_to_string(&mut body);
            let b: Value = serde_json::from_str(&body).unwrap_or(json!({}));
            let tok = req.headers().iter().find(|h| h.field.equiv("X-HM-Token")).map(|h| h.value.as_str().to_string());
            if tok.as_deref() != Some(self.token.as_str()) {
                jerr("jeton invalide : recharge la page", 403)
            } else {
                self.post(&path, &b)
            }
        } else {
            self.get(&path, &qv)
        };
        self.respond(req, out);
    }

    fn host_ok(&self, req: &Request) -> bool {
        let port = self.port.load(Ordering::SeqCst);
        let allowed = [format!("127.0.0.1:{}", port), format!("localhost:{}", port), format!("[::1]:{}", port)];
        let hv = |n: &'static str| req.headers().iter().find(|h| h.field.equiv(n)).map(|h| h.value.as_str().to_lowercase());
        let host = hv("Host").unwrap_or_default();
        if !allowed.contains(&host) {
            return false;
        }
        if let Some(o) = hv("Origin") {
            if !allowed.iter().any(|a| o == format!("http://{}", a)) {
                return false;
            }
        }
        true
    }

    fn respond(&self, req: Request, out: Out) {
        let h = |k: &str, v: &str| Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap();
        let common = vec![h("X-Content-Type-Options", "nosniff"), h("Referrer-Policy", "no-referrer")];
        match out {
            Out::Json(v, code) => {
                let data = serde_json::to_vec(&i18n::tr_obj(v)).unwrap_or_default();
                let mut r = Response::from_data(data).with_status_code(StatusCode(code));
                for x in common.into_iter().chain([h("Content-Type", "application/json; charset=utf-8"), h("Cache-Control", "no-store")]) {
                    r.add_header(x);
                }
                let _ = req.respond(r);
            }
            Out::Html(s, extra) => {
                let mut r = Response::from_data(s.into_bytes());
                for x in common.into_iter().chain([h("Content-Type", "text/html; charset=utf-8"), h("Cache-Control", "no-store")]) {
                    r.add_header(x);
                }
                for (k, v) in extra {
                    r.add_header(h(&k, &v));
                }
                let _ = req.respond(r);
            }
            Out::Bytes(b, ctype) => {
                let mut r = Response::from_data(b);
                for x in common.into_iter().chain([h("Content-Type", &ctype), h("Cache-Control", "max-age=300")]) {
                    r.add_header(x);
                }
                let _ = req.respond(r);
            }
            Out::File(p, ctype) => {
                let Ok(mut f) = fs::File::open(&p) else {
                    let _ = req.respond(Response::from_string("{\"error\":\"fichier introuvable\"}").with_status_code(404));
                    return;
                };
                let size = f.metadata().map(|m| m.len()).unwrap_or(0);
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let ctype = ctype.unwrap_or_else(|| mime_guess::from_path(&p).first_or_octet_stream().essence_str().to_string());
                let rng = req.headers().iter().find(|x| x.field.equiv("Range")).map(|x| x.value.as_str().to_string());
                let (mut start, mut end, mut code) = (0u64, size.saturating_sub(1), 200);
                if let Some(r) = rng.and_then(|r| r.strip_prefix("bytes=").map(String::from)) {
                    let (a, b) = r.split_once('-').unwrap_or((&r, ""));
                    start = a.parse().unwrap_or(0);
                    end = if b.is_empty() { size.saturating_sub(1) } else { b.parse().unwrap_or(size.saturating_sub(1)) };
                    end = end.min(size.saturating_sub(1));
                    code = 206;
                }
                let len = if size == 0 { 0 } else { end.saturating_sub(start) + 1 };
                let _ = f.seek(SeekFrom::Start(start));
                let mut headers = common;
                headers.push(h("Content-Type", &ctype));
                headers.push(h("Accept-Ranges", "bytes"));
                headers.push(h("X-Frame-Options", "SAMEORIGIN"));
                if media::ACTIVE.contains(&media::suffix(&name.to_lowercase()).as_str()) {
                    // HTML / SVG des notes : affichés sans pouvoir exécuter de script ni piloter l'app
                    headers.push(h("Content-Security-Policy", "sandbox"));
                }
                if code == 206 {
                    headers.push(h("Content-Range", &format!("bytes {}-{}/{}", start, end, size)));
                }
                let r = Response::new(StatusCode(code), headers, f.take(len), Some(len as usize), None);
                let _ = req.respond(r);
            }
        }
    }

    fn index_html(&self) -> Out {
        let html = Static::get("index.html").map(|f| String::from_utf8_lossy(&f.data).to_string()).unwrap_or_default();
        let lang = self.cfg().language();
        let d = &i18n::DICT.raw;
        let dict = json!({"exact": d.get("exact").cloned().unwrap_or(json!({})), "patterns": d.get("patterns").cloned().unwrap_or(json!({}))});
        let html = html
            .replace("__HM_TOKEN__", &self.token)
            .replace("__HM_LANG__", if lang == "fr" || lang == "en" { &lang } else { "" })
            .replace("__HM_I18N__", &dict.to_string().replace("</", "<\\/"));
        Out::Html(html, vec![("Content-Security-Policy".into(), INDEX_CSP.into()), ("X-Frame-Options".into(), "DENY".into())])
    }

    fn get(self: &Arc<Self>, path: &str, qv: &dyn Fn(&str) -> String) -> Out {
        let p = qv("path");
        let r: Result<Out, String> = (|| {
            Ok(match path {
                "/" | "/index.html" => self.index_html(),
                "/api/ping" => {
                    *self.last_ping.lock().unwrap() = now();
                    j(json!({"app": "hermes-map", "version": VERSION, "pid": std::process::id(), "started": self.started, "engine": "rust"}))
                }
                "/api/config" => {
                    let st = self.st.read().unwrap();
                    let mut d = st.cfg.to_public();
                    d["setup"] = st.setup.clone().unwrap_or(Value::Null);
                    d["hermes_found"] = json!(st.runner.as_ref().map(|r| r.hermes.is_some()).unwrap_or(false));
                    d["version"] = json!(VERSION);
                    d["platform"] = json!(std::env::consts::OS);
                    j(d)
                }
                "/api/setup/detect" => {
                    let mut d = config::detect(crate::hermes::find_hermes(self.opts.hermes.clone()).map(|p| p.to_string_lossy().to_string()));
                    d["current"] = self.cfg().to_public();
                    j(d)
                }
                "/api/setup/inspect" => j(config::inspect(&p)),
                "/api/graph" => j(self.graph(qv("refresh") == "1")),
                "/api/node" => j(self.node_info(&p)?),
                "/api/raw" => {
                    let f = self.safe(&p)?;
                    if kind_of(&f) == "secret" && !self.opts.reveal {
                        return Ok(jerr("fichier secret masqué", 403));
                    }
                    if !f.exists() {
                        return Ok(jerr("fichier introuvable", 404));
                    }
                    Out::File(f, None)
                }
                "/api/thumb" => {
                    let f = self.safe(&p)?;
                    let k = kind_of(&f);
                    let t = if ["image", "video", "pdf", "office", "text", "markdown", "json", "config"].contains(&k.as_str()) {
                        media::make_thumb(&f, &k)
                    } else {
                        None
                    };
                    match t {
                        Some(t) => {
                            let png = fs::read(&t).map(|b| b.starts_with(b"\x89PNG")).unwrap_or(false);
                            Out::File(t, Some(if png { "image/png" } else { "image/jpeg" }.into()))
                        }
                        None => jerr("pas de miniature", 404),
                    }
                }
                "/api/search" => {
                    let lim: usize = qv("limit").parse().unwrap_or(1000);
                    j(self.search(&qv("q"), &p, qv("content") == "1", lim.min(5000)))
                }
                "/api/video" => {
                    let f = self.safe(&p)?;
                    let mut s = self.video.status(&f);
                    s["ffmpeg"] = json!(self.video.ffmpeg.is_some());
                    j(s)
                }
                "/api/video/file" => {
                    let f = self.safe(&p)?;
                    let ext = media::suffix(&basename(&p).to_lowercase());
                    if media::WEB_VIDEO.contains(&ext.as_str()) {
                        Out::File(f, None)
                    } else {
                        match self.video.cached_file(&f) {
                            Some(c) => Out::File(c, Some("video/mp4".into())),
                            None => jerr("vidéo pas encore convertie", 404),
                        }
                    }
                }
                "/api/doc" => {
                    let f = self.safe(&p)?;
                    match media::doc_html(&f) {
                        Some(h) => j(json!({"html": h})),
                        None => j(json!({"unsupported": true})),
                    }
                }
                "/api/dirs" => j(json!({"dirs": self.scanner().map(|s| mover::suggest_dirs(&s, &qv("q"))).unwrap_or_default()})),
                "/api/db" => j(self.db_table(&p, &qv("table"), qv("offset").parse().unwrap_or(0))?),
                "/api/hermes/info" => match self.runner() {
                    None => j(json!({"available": false, "disabled": true})),
                    Some(r) => {
                        let mut info = r.info(&self.cfg());
                        if !p.is_empty() {
                            info["target"] = self.hermes_target(&p);
                        }
                        j(info)
                    }
                },
                "/api/hermes/status" => {
                    let r = self.runner().ok_or("introuvable")?;
                    let job = r.get(&qv("job")).ok_or("tâche inconnue")?;
                    j(job.public(qv("since").parse().unwrap_or(0), qv("esince").parse().unwrap_or(0)))
                }
                "/api/hermes/diff" => {
                    let r = self.runner().ok_or("introuvable")?;
                    let job = r.get(&qv("job")).ok_or("tâche inconnue")?;
                    j(job.diff(&p))
                }
                _ if path.starts_with("/static/") => {
                    let rel = &path["/static/".len()..];
                    match Static::get(rel) {
                        Some(f) => Out::Bytes(f.data.to_vec(), mime_guess::from_path(rel).first_or_octet_stream().essence_str().to_string()),
                        None => jerr("introuvable", 404),
                    }
                }
                _ => jerr("introuvable", 404),
            })
        })();
        r.unwrap_or_else(|e| {
            let code = if e.contains("hors") || e.contains("aucun dossier") { 403 } else if e.contains("introuvable") || e.contains("inconnue") { 404 } else { 500 };
            jerr(&e, code)
        })
    }

    fn post(self: &Arc<Self>, path: &str, b: &Value) -> Out {
        let s = |k: &str| b.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let r: Result<Out, String> = (|| {
            Ok(match path {
                "/api/quit" => {
                    if self.busy() {
                        return Ok(jerr("Hermes travaille encore : arrête-le d'abord ou attends la fin.", 409));
                    }
                    std::thread::spawn(|| {
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        crate::cleanup_and_exit(0);
                    });
                    j(json!({"ok": true}))
                }
                "/api/lang" => {
                    let lg = s("lang");
                    if lg != "fr" && lg != "en" {
                        return Ok(jerr("langue inconnue", 400));
                    }
                    self.st.write().unwrap().cfg.data["language"] = json!(lg);
                    i18n::set_lang(&lg);
                    let _ = config::save_language(&lg);
                    j(json!({"ok": true, "lang": lg}))
                }
                "/api/setup/choose" => {
                    let pr = s("prompt");
                    j(media::choose_folder(if pr.is_empty() { "Choisis le dossier à explorer" } else { &pr }))
                }
                "/api/os/privacy" => {
                    media::open_privacy_settings(std::env::var("HM_APP").ok());
                    j(json!({"ok": true}))
                }
                "/api/setup/save" => j(self.setup_save(b)),
                "/api/open" => {
                    let f = self.safe(&s("path"))?;
                    media::open_in_app(&f, b.get("reveal").and_then(|v| v.as_bool()).unwrap_or(false))?;
                    j(json!({"ok": true}))
                }
                "/api/move" | "/api/move/plan" | "/api/move/undo" => {
                    let mv = mover::Mover::new(self.clone())?;
                    let items = b.get("items").cloned().unwrap_or(json!([]));
                    let create = b.get("create_dirs").and_then(|v| v.as_bool()).unwrap_or(true);
                    match path {
                        "/api/move/plan" => j(json!({"plan": mv.plan(&items, create)})),
                        "/api/move/undo" => j(mv.undo(&s("id"))),
                        _ => j(mv.run(&items, create, b.get("update_index").and_then(|v| v.as_bool()).unwrap_or(true), &s("note"))),
                    }
                }
                _ if path.starts_with("/api/hermes/") => {
                    let r = match self.runner() {
                        Some(r) => r,
                        None => return Ok(jerr("bouton Hermes désactivé (--no-hermes)", 403)),
                    };
                    let profile = Some(s("profile")).filter(|x| !x.trim().is_empty());
                    let prompt = s("prompt").trim().to_string();
                    match path {
                        "/api/hermes/run" if s("mode") == "reclass" => {
                            let mut files = vec![];
                            for f in b.get("files").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                                if let Some(fs_) = f.as_str() {
                                    let fp = self.safe(fs_)?;
                                    if fp.is_file() {
                                        files.push(rel_of(&self.root().unwrap(), &fp));
                                    }
                                }
                            }
                            if files.is_empty() {
                                return Ok(jerr("aucun fichier à reclasser", 400));
                            }
                            if prompt.is_empty() {
                                return Ok(jerr("consigne vide", 400));
                            }
                            let root = self.root().unwrap();
                            match r.start(&self.cfg(), &root, &prompt, "reclass", files, profile) {
                                Ok(v) => j(v),
                                Err(e) => jerr(&e, 409),
                            }
                        }
                        "/api/hermes/run" => {
                            let t = self.hermes_target(&s("path"));
                            if t["ok"] != json!(true) {
                                return Ok(jerr(t["why"].as_str().unwrap_or(""), 400));
                            }
                            if prompt.is_empty() {
                                return Ok(jerr("consigne vide", 400));
                            }
                            let target = PathBuf::from(t["abs"].as_str().unwrap_or(""));
                            match r.start(&self.cfg(), &target, &prompt, "update", vec![], profile) {
                                Ok(v) => j(v),
                                Err(e) => jerr(&e, 409),
                            }
                        }
                        "/api/hermes/stop" => {
                            let job = r.get(&s("job")).ok_or("tâche inconnue")?;
                            job.stop("arrêt demandé");
                            j(json!({"ok": true}))
                        }
                        "/api/hermes/undo" => {
                            let job = r.get(&s("job")).ok_or("tâche inconnue")?;
                            if job.running() {
                                return Ok(jerr("attends la fin (ou arrête) avant d'annuler", 400));
                            }
                            j(job.undo())
                        }
                        _ => jerr("introuvable", 404),
                    }
                }
                _ => jerr("introuvable", 404),
            })
        })();
        r.unwrap_or_else(|e| {
            let code = if e.contains("hors") { 403 } else if e.contains("introuvable") || e.contains("inconnue") { 404 } else { 500 };
            jerr(&e, code)
        })
    }

    fn setup_save(self: &Arc<Self>, body: &Value) -> Value {
        let mut data = self.cfg().data;
        for k in ["root", "include", "folders", "hidden_tops", "title", "zone_prefix", "ignore"] {
            if let Some(v) = body.get(k) {
                data[k] = v.clone();
            }
        }
        for k in ["archive", "hermes"] {
            if let Some(v) = body.get(k).filter(|v| v.is_object()) {
                data[k] = config::merge(&data[k], v);
            }
        }
        let root = data["root"].as_str().unwrap_or("").to_string();
        if root.is_empty() {
            return json!({"error": "choisis un dossier"});
        }
        let info = config::inspect(&root);
        match info["access"].as_str() {
            Some("missing") => return json!({"error": format!("dossier introuvable : {}", root)}),
            Some("denied") => return json!({"error": "denied", "path": root}),
            _ => {}
        }
        let cfg = Settings::new(&data);
        let made = if cfg.archive().is_some() {
            match config::init_archive(&cfg) {
                Ok(m) => m,
                Err(e) => return json!({"error": format!("impossible de créer le dossier d'archivage : {}", e)}),
            }
        } else {
            vec![]
        };
        match self.configure(cfg.clone()).as_deref() {
            Some("busy") => return json!({"error": "Hermes travaille encore : attends la fin avant de changer les réglages."}),
            Some(w) => return json!({"error": w, "path": root}),
            None => {}
        }
        let _ = cfg.save();
        json!({"ok": true, "made": made, "config": cfg.to_public()})
    }

    fn search(&self, query: &str, scope: &str, content: bool, limit: usize) -> Value {
        let Some(sc) = self.scanner() else { return json!({"results": [], "total": 0}) };
        let data = sc.data.read().unwrap();
        let Some(idx) = data.index.as_ref() else {
            return json!({"pending": true, "status": self.status.lock().unwrap().clone()});
        };
        let words: Vec<String> = norm_text(query).split_whitespace().map(String::from).collect();
        if words.is_empty() {
            return json!({"results": [], "total": 0});
        }
        let mut scope = scope.trim_matches('/').to_string();
        if scope.starts_with("#tag:") {
            scope.clear();
        }
        let prefix = if scope.is_empty() { String::new() } else { format!("{}/", scope) };
        let mut res = vec![];
        let mut total = 0usize;
        for e in idx {
            if !prefix.is_empty() && !e.rel.starts_with(&prefix) {
                continue;
            }
            let n_name = norm_text(&e.name);
            let n_rel = norm_text(&e.rel[prefix.len()..]);
            let (m, rank) = if words.iter().all(|w| n_name.contains(w.as_str())) {
                ("nom", 0)
            } else if words.iter().all(|w| n_rel.contains(w.as_str())) {
                ("chemin", 1) // trouvé via un dossier parent
            } else {
                continue;
            };
            total += 1;
            res.push((rank, !e.is_dir, e.rel.matches('/').count(), e, m));
        }
        res.sort_by(|a, b| (a.0, a.1, a.2, &a.3.rel).cmp(&(b.0, b.1, b.2, &b.3.rel)));
        let mut out: Vec<Value> = res
            .iter()
            .take(limit)
            .map(|(_, _, _, e, m)| json!({"path": e.rel, "name": e.name, "kind": if e.is_dir { "dir".into() } else { kind_of_name(&basename(&e.rel), false) },
                "size": e.size, "mtime": e.mtime, "cloud": e.cloud, "match": m, "where": dirname(&e.rel)}))
            .collect();
        let (mut hits, mut done) = (0usize, true);
        if content {
            let seen: std::collections::HashSet<String> = out.iter().map(|o| o["path"].as_str().unwrap_or("").to_string()).collect();
            let t0 = std::time::Instant::now();
            let root = sc.root.clone();
            for e in idx {
                if e.is_dir || e.cloud || e.size > 2_000_000 || seen.contains(&e.rel) {
                    continue;
                }
                if !prefix.is_empty() && !e.rel.starts_with(&prefix) {
                    continue;
                }
                let k = kind_of_name(&e.name, false);
                if !["markdown", "text", "config", "json"].contains(&k.as_str()) {
                    continue;
                }
                if t0.elapsed().as_secs() > 8 || out.len() >= limit {
                    done = false;
                    break;
                }
                let Ok(raw) = fs::read(root.join(&e.rel)) else { continue };
                if raw[..raw.len().min(2048)].contains(&0) {
                    continue;
                }
                let txt = String::from_utf8_lossy(&raw);
                let ntxt = norm_text(&txt);
                if words.iter().all(|w| ntxt.contains(w.as_str())) {
                    let chars: Vec<char> = txt.chars().collect();
                    let first = ntxt.find(words[0].as_str()).map(|b| ntxt[..b].chars().count()).unwrap_or(0).min(chars.len());
                    let snippet: String = chars[first.saturating_sub(60)..(first + 100).min(chars.len())].iter().collect::<String>().replace('\n', " ");
                    out.push(json!({"path": e.rel, "name": e.name, "kind": k, "size": e.size, "mtime": e.mtime, "cloud": false,
                        "match": "contenu", "snippet": snippet, "where": dirname(&e.rel)}));
                    hits += 1;
                    total += 1;
                }
            }
        }
        let n = out.len();
        json!({"results": out, "total": total, "truncated": total > n, "content_hits": hits, "content_done": done})
    }

    /// Cible de la mise à jour : le dossier ouvert, ou le fichier .md ouvert (sinon son dossier).
    pub fn hermes_target(&self, rel: &str) -> Value {
        let rel = rel.trim_matches('/');
        if rel.is_empty() || rel.starts_with("#tag:") {
            return json!({"ok": false, "why": "Ouvre d'abord un dossier (ou un sous-dossier, ou une note .md) : la mise à jour part de l'endroit où tu es."});
        }
        let Ok(mut p) = self.safe(rel) else { return json!({"ok": false, "why": "chemin hors du dossier"}) };
        let low = p.to_string_lossy().to_lowercase();
        if p.is_file() && !(low.ends_with(".md") || low.ends_with(".markdown")) {
            p = p.parent().unwrap().to_path_buf();
        }
        let root = self.root().unwrap();
        if p == root {
            return json!({"ok": false, "why": "Ouvre un dossier ou un de ses sous-dossiers."});
        }
        let kind = if p.is_dir() { "dir" } else { "file" };
        let mut n_md = 0usize;
        if kind == "dir" {
            let mut stack = vec![p.clone()];
            while let Some(d) = stack.pop() {
                for e in fs::read_dir(&d).into_iter().flatten().flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    let ft = match e.file_type() { Ok(f) => f, Err(_) => continue };
                    if ft.is_dir() {
                        if !name.starts_with('.') {
                            stack.push(e.path());
                        }
                    } else if name.to_lowercase().ends_with(".md") || name.to_lowercase().ends_with(".markdown") {
                        n_md += 1;
                    }
                }
                if n_md > 20000 {
                    break;
                }
            }
        }
        json!({"ok": true, "path": rel_of(&root, &p), "abs": p.to_string_lossy(), "name": p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
               "kind": kind, "parent": rel_of(&root, p.parent().unwrap_or(&root)), "md_count": if kind == "dir" { n_md } else { 1 }})
    }

    fn node_info(&self, rel: &str) -> Result<Value, String> {
        let sc = self.scanner().ok_or("aucun dossier configuré")?;
        let root = self.root().unwrap();
        if let Some(t) = rel.strip_prefix("#tag:") {
            let files = sc.data.read().unwrap().tag_index.get(t).cloned().unwrap_or_default();
            let mut children = vec![];
            for f in files {
                let Ok(fp) = self.safe(&f) else { continue };
                let Ok(md) = fs::metadata(&fp) else { continue };
                children.push(json!({"name": basename(&f), "path": f, "kind": kind_of(&fp), "size": md.len(), "mtime": mtime(&md),
                                     "where": dirname(&f), "cloud": is_dataless(&md)}));
            }
            let n = children.len();
            return Ok(json!({"path": rel, "name": format!("#{}", t), "kind": "tag", "size": 0, "mtime": 0,
                             "children": children, "children_total": n}));
        }
        let p = if rel.is_empty() || rel == "/" { root.clone() } else { self.safe(rel)? };
        let md = fs::metadata(&p).map_err(|_| "fichier introuvable".to_string())?;
        let k = if md.is_dir() { "dir".to_string() } else { kind_of(&p) };
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let here = if p == root { String::new() } else { rel_of(&root, &p) };
        let mut info = json!({"path": here, "name": name, "kind": k, "size": md.len(), "mtime": mtime(&md),
                              "cloud": is_dataless(&md) && k != "dir"});
        if k == "icloud" {
            info["name"] = json!(icloud_real_name(&name));
            info["note"] = json!("Ce fichier est dans iCloud mais pas encore téléchargé sur ce Mac. Dans le Finder : clic droit → « Télécharger maintenant », puis ↻.");
        } else if k == "dir" {
            let depth = if p == root { 1 } else { 2 };
            let mut ents: Vec<(bool, String, PathBuf)> = fs::read_dir(&p)
                .map_err(|e| e.to_string())?
                .flatten()
                .map(|e| (e.path().is_dir(), e.file_name().to_string_lossy().to_string(), e.path()))
                .collect();
            ents.sort_by(|a, b| (!a.0, a.1.to_lowercase()).cmp(&(!b.0, b.1.to_lowercase())));
            let cat = sc.catalog();
            let mut children = vec![];
            for (is_dir, n, ep) in ents {
                if sc.skip(&n, depth) {
                    continue;
                }
                let Ok(em) = fs::metadata(&ep) else { continue };
                let crel = rel_of(&root, &ep);
                let mut c = json!({"name": icloud_real_name(&n).unwrap_or(n.clone()), "path": crel,
                    "kind": if is_dir { "dir".into() } else { kind_of_name(&n, false) }, "size": em.len(), "mtime": mtime(&em),
                    "cloud": is_dataless(&em) && !is_dir});
                if let Some(d) = cat.get(&crel) {
                    c["desc"] = Value::Object(d.clone());
                }
                children.push(c);
            }
            let total = children.len();
            children.truncate(2000);
            info["children"] = json!(children);
            info["children_total"] = json!(total);
            if let Some(mirror) = sc.mirror_of(&here) {
                let data = sc.data.read().unwrap();
                if let Some(idx) = &data.index {
                    let pre = format!("{}/", mirror);
                    let mut items: Vec<Value> = idx
                        .iter()
                        .filter(|e| e.rel.starts_with(&pre))
                        .map(|e| {
                            let mut it = json!({"name": e.name, "path": e.rel, "kind": if e.is_dir { "dir".into() } else { kind_of_name(&e.name, false) },
                                "size": e.size, "mtime": e.mtime, "cloud": e.cloud, "archive": true, "where": dirname(&e.rel)});
                            if let Some(d) = cat.get(&e.rel) {
                                it["desc"] = Value::Object(d.clone());
                            }
                            it
                        })
                        .collect();
                    items.sort_by(|a, b| {
                        let ka = (a["kind"] != "dir", a["path"].as_str().unwrap_or("").to_lowercase());
                        let kb = (b["kind"] != "dir", b["path"].as_str().unwrap_or("").to_lowercase());
                        ka.cmp(&kb)
                    });
                    let described = items.iter().filter(|x| x.get("desc").is_some()).count();
                    let total = items.len();
                    items.truncate(5000);
                    info["mirror"] = json!({"path": mirror, "total": total, "items": items, "described": described});
                }
            }
            if let Some(b) = sc.brain_of(&here) {
                info["brain"] = json!(b);
            }
            for readme in ["SKILL.md", "README.md", "index.md", "SOUL.md"] {
                let rp = p.join(readme);
                if rp.is_file() {
                    let t = read_text(&rp, MAX_TEXT);
                    info["readme"] = json!({"name": readme, "text": t});
                    break;
                }
            }
        } else if k == "secret" {
            let mut txt = read_text(&p, MAX_TEXT);
            if !self.opts.reveal {
                if name.to_lowercase().ends_with(".json") {
                    txt = match serde_json::from_str::<Value>(&txt) {
                        Ok(v) => serde_json::to_string_pretty(&mask_json(v)).unwrap_or_default(),
                        Err(_) => mask_secret_text(&txt),
                    };
                } else {
                    txt = mask_secret_text(&txt);
                }
                info["masked"] = json!(true);
            }
            info["text"] = json!(txt);
        } else if k == "db" {
            info["tables"] = db_tables(&p).unwrap_or(json!([]));
        } else if ["markdown", "json", "config", "text"].contains(&k.as_str()) {
            if media::is_texty(&p) || ["markdown", "json", "config"].contains(&k.as_str()) {
                info["text"] = json!(read_text(&p, MAX_TEXT));
                info["truncated"] = json!(md.len() > MAX_TEXT);
            } else {
                info["kind"] = json!("binary");
            }
        }
        Ok(info)
    }

    fn db_table(&self, rel: &str, table: &str, offset: i64) -> Result<Value, String> {
        let p = self.safe(rel)?;
        let con = open_ro(&p).map_err(|e| e.to_string())?;
        let names: Vec<String> = con
            .prepare("SELECT name FROM sqlite_master")
            .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0)).map(|it| it.flatten().collect()))
            .map_err(|e| e.to_string())?;
        if !names.iter().any(|n| n == table) {
            return Ok(json!({"error": "table inconnue"}));
        }
        let mut stmt = con.prepare(&format!("SELECT * FROM \"{}\" LIMIT 100 OFFSET ?", table.replace('"', "\"\""))).map_err(|e| e.to_string())?;
        let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let nc = cols.len();
        let rows: Vec<Value> = stmt
            .query_map([offset.max(0)], |r| {
                let mut row = vec![];
                for i in 0..nc {
                    use rusqlite::types::ValueRef;
                    row.push(match r.get_ref(i)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(x) => json!(x),
                        ValueRef::Real(x) => json!(x),
                        ValueRef::Text(t) => {
                            let s = String::from_utf8_lossy(t).to_string();
                            if s.chars().count() > 4000 { json!(format!("{}…", s.chars().take(4000).collect::<String>())) } else { json!(s) }
                        }
                        ValueRef::Blob(b) => json!(format!("<{} octets>", b.len())),
                    });
                }
                Ok(Value::Array(row))
            })
            .map_err(|e| e.to_string())?
            .flatten()
            .collect();
        Ok(json!({"columns": cols, "rows": rows, "offset": offset}))
    }
}

fn open_ro(p: &Path) -> rusqlite::Result<rusqlite::Connection> {
    let c = rusqlite::Connection::open_with_flags(p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    c.busy_timeout(std::time::Duration::from_secs(2))?;
    Ok(c)
}

fn db_tables(p: &Path) -> Option<Value> {
    let con = open_ro(p).ok()?;
    let names: Vec<String> = con
        .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .ok()?
        .query_map([], |r| r.get::<_, String>(0))
        .ok()?
        .flatten()
        .collect();
    Some(Value::Array(
        names
            .into_iter()
            .map(|n| {
                let c: Option<i64> = con.query_row(&format!("SELECT COUNT(*) FROM \"{}\"", n.replace('"', "\"\"")), [], |r| r.get(0)).ok();
                json!({"name": n, "rows": c})
            })
            .collect(),
    ))
}

pub fn read_text(p: &Path, max: u64) -> String {
    let mut raw = vec![];
    if let Ok(f) = fs::File::open(p) {
        let _ = f.take(max).read_to_end(&mut raw);
    }
    String::from_utf8_lossy(&raw).to_string()
}

fn mask_secret_text(txt: &str) -> String {
    let re = Regex::new(r"^(\s*(?:export\s+)?[\w.\-]+\s*[=:]\s*)(.*)$").unwrap();
    txt.lines()
        .map(|line| match re.captures(line) {
            Some(m) if !m[2].trim().is_empty() => {
                let v = m[2].trim().trim_matches(|c| c == '"' || c == '\'');
                let n = v.chars().count();
                let tail: String = v.chars().skip(n.saturating_sub(4)).collect();
                format!("{}{}", &m[1], if n > 8 { format!("{}{}", "•".repeat(8), tail) } else { "•".repeat(8) })
            }
            _ => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn mask_json(v: Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(
            o.into_iter()
                .map(|(k, x)| {
                    let hide = SECRET_KEY_RE.is_match(&k) && (x.is_string() || x.is_number());
                    (k, if hide { json!("••••••••") } else { mask_json(x) })
                })
                .collect::<Map<String, Value>>(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(mask_json).collect()),
        x => x,
    }
}

pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            let dec = |s: &str| percent_encoding::percent_decode_str(&s.replace('+', " ")).decode_utf8_lossy().to_string();
            (dec(k), dec(v))
        })
        .collect()
}
