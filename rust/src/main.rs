//! Hermes Map — explorateur visuel (mind map) de dossiers de notes.
//!
//! Lancement :  hermes-map                     -> écran d'accueil au premier lancement, puis le dossier choisi
//!              hermes-map ~/Notes --only '*'  -> dossier imposé, sans écran d'accueil
//!
//! Le serveur n'écoute que sur 127.0.0.1 (cette machine uniquement).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod assets;
mod config;
mod hermes;
mod i18n;
mod logfile;
#[cfg(target_os = "macos")]
mod macos;
mod media;
mod mover;
mod prompts;
mod scan;
mod util;

use app::{App, Opts};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;

static PORT_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn cleanup_and_exit(code: i32) -> ! {
    if let Some(p) = PORT_FILE.lock().ok().and_then(|g| g.clone()) {
        let _ = std::fs::remove_file(p);
    }
    std::process::exit(code)
}

struct Args {
    root: Option<String>,
    only: Option<String>,
    port: u16,
    no_browser: bool,
    port_file: Option<PathBuf>,
    idle_exit: u64,
    opts: Opts,
}

fn usage() -> ! {
    println!(
        "Hermes Map {} — explorateur visuel de dossiers de notes

Usage : hermes-map [DOSSIER] [options]
  --only MOTIFS        dossiers de 1er niveau à afficher (séparés par des virgules, \"*\" = tout)
  --port N             port local (défaut 8765, le suivant s'il est pris)
  --no-browser         ne pas ouvrir le navigateur
  --port-file F        écrit le port utilisé dans ce fichier
  --idle-exit N        s'arrête N s après la fermeture de la dernière page (0 = jamais)
  --all                n'ignorer aucun dossier (code source inclus)
  --hidden             afficher aussi les fichiers cachés
  --reveal-secrets     afficher les clés API en clair
  --hermes CHEMIN      commande hermes à utiliser
  --hermes-args \"…\"    options ajoutées à « hermes chat »
  --no-hermes          désactiver les fonctions Hermes",
        app::VERSION
    );
    std::process::exit(0)
}

fn parse_args() -> Args {
    let mut a = Args { root: None, only: None, port: 8765, no_browser: false, port_file: None, idle_exit: 0, opts: Opts::default() };
    // « -psn_… » : ajouté par d'anciens macOS au lancement d'une app
    let argv: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with("-psn_")).collect();
    let launched_bare = argv.is_empty(); // double-clic : fonctionne comme une app
    let mut it = argv.into_iter();
    while let Some(x) = it.next() {
        match x.as_str() {
            "-h" | "--help" => usage(),
            "--version" => {
                println!("{}", app::VERSION);
                std::process::exit(0)
            }
            "--only" => a.only = it.next(),
            "--port" => a.port = it.next().and_then(|v| v.parse().ok()).unwrap_or(8765),
            "--no-browser" => a.no_browser = true,
            "--port-file" => a.port_file = it.next().map(PathBuf::from),
            "--idle-exit" => a.idle_exit = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--all" => a.opts.all = true,
            "--hidden" => a.opts.hidden = true,
            "--reveal-secrets" => a.opts.reveal = true,
            "--hermes" => a.opts.hermes = it.next(),
            "--hermes-args" => a.opts.hermes_args = it.next().unwrap_or_default(),
            "--no-hermes" => a.opts.no_hermes = true,
            s if !s.starts_with('-') && a.root.is_none() => a.root = Some(s.to_string()),
            s => eprintln!("option inconnue : {}", s),
        }
    }
    if launched_bare {
        a.idle_exit = 120;
        a.port_file = Some(config::config_dir().join("port"));
    }
    a
}

/// Hermes Map déjà lancé ? Renvoie (port, pid, date de démarrage).
fn running_instance(port_file: &PathBuf) -> Option<(u16, u32, u64)> {
    let port: u16 = std::fs::read_to_string(port_file).ok()?.trim().parse().ok()?;
    let mut s = TcpStream::connect_timeout(&format!("127.0.0.1:{}", port).parse().ok()?, Duration::from_secs(2)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    write!(s, "GET /api/ping HTTP/1.0\r\nHost: 127.0.0.1:{}\r\n\r\n", port).ok()?;
    let mut buf = String::new();
    s.read_to_string(&mut buf).ok()?;
    let body = buf.split("\r\n\r\n").nth(1)?;
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    if v["app"] != "hermes-map" {
        return None;
    }
    Some((port, v["pid"].as_u64()? as u32, v["started"].as_u64().unwrap_or(0)))
}

fn kill(pid: u32) {
    #[cfg(unix)]
    let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
    #[cfg(windows)]
    let _ = std::process::Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).status();
}

fn main() {
    // une erreur interne est notée au journal (avec l'endroit du code) au lieu de passer inaperçue
    std::panic::set_hook(Box::new(|info| {
        logfile::line(&format!("ERREUR interne : {}", info));
    }));
    let args = parse_args();
    if args.idle_exit > 0 && args.port_file.is_some() {
        // lancée comme une app (double-clic) : on garde une trace de ce qui se passe
        logfile::redirect_output();
        println!();
        logfile::line(&format!("===== démarrage de Hermes Map {} (moteur Rust)", app::VERSION));
    }

    // déjà lancé : on rouvre la page (sauf si c'est une version plus ancienne que ce programme)
    if let Some(pf) = &args.port_file {
        if let Some((port, pid, started)) = running_instance(pf) {
            let built = std::env::current_exe().ok().and_then(|e| std::fs::metadata(e).ok()).map(|m| media::mtime(&m) as u64).unwrap_or(0);
            if started >= built && pid != std::process::id() {
                media::open_url(&format!("http://127.0.0.1:{}/", port));
                return;
            }
            println!("Ancienne version encore lancée (pid {}) : redémarrage.", pid);
            kill(pid);
            std::thread::sleep(Duration::from_millis(800));
        }
    }

    let mut cfg = config::load().unwrap_or_else(config::Settings::empty);
    if let Some(r) = &args.root {
        cfg = config::Settings::new(&config::merge(&cfg.data, &serde_json::json!({"root": r})));
    }
    if let Some(o) = &args.only {
        cfg.data["include"] = serde_json::json!(o);
    }
    let app = App::new(args.opts.clone());
    match app.configure(cfg.clone()).as_deref() {
        Some("first") => println!("Premier lancement : choisis le dossier à explorer dans la page qui s'ouvre."),
        Some("denied") => eprintln!(
            "Accès refusé par le système à {:?}\nMac : Réglages Système > Confidentialité et sécurité > Accès complet au disque : active Hermes Map.",
            cfg.root()
        ),
        Some("missing") => eprintln!("Dossier introuvable : {:?} (à choisir à nouveau dans la page)", cfg.root()),
        _ => {}
    }
    println!(
        "Vidéos AVI/MKV/WMV… : {}",
        match &app.video.ffmpeg {
            Some(f) => format!("conversion avec {}", f.display()),
            None => "ffmpeg absent ; elles s'ouvriront dans l'app du système".into(),
        }
    );

    // serveur local
    let mut port = args.port;
    let server = loop {
        match tiny_http::Server::http(("127.0.0.1", port)) {
            Ok(s) => break s,
            Err(_) if port < args.port + 20 => port += 1,
            Err(e) => {
                eprintln!("Aucun port libre trouvé : {}", e);
                std::process::exit(1);
            }
        }
    };
    app.port.store(port as usize, Ordering::SeqCst);
    let url = format!("http://127.0.0.1:{}/", port);
    if let Some(pf) = &args.port_file {
        if let Some(d) = pf.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(pf, port.to_string());
        *PORT_FILE.lock().unwrap() = Some(pf.clone());
    }
    if args.idle_exit > 0 {
        // mode app : quand plus aucune page n'est ouverte (et que Hermes ne travaille pas), on s'arrête
        let a = app.clone();
        let idle = args.idle_exit as f64;
        std::thread::spawn(move || {
            let started = util::now();
            loop {
                std::thread::sleep(Duration::from_secs(15));
                let last = *a.last_ping.lock().unwrap();
                let since = util::now() - if last > 0.0 { last } else { started };
                if !a.busy() && since > idle.max(if last > 0.0 { 60.0 } else { 300.0 }) {
                    println!("Plus aucune fenêtre ouverte : arrêt de Hermes Map.");
                    cleanup_and_exit(0);
                }
            }
        });
    }
    println!("Hermes Map ouvert sur {}  (Ctrl+C pour arrêter)", url);
    if !args.no_browser {
        let u = url.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            media::open_url(&u);
        });
    }
    let server = std::sync::Arc::new(server);
    let mut workers = vec![];
    for _ in 0..8 {
        let (s, a) = (server.clone(), app.clone());
        workers.push(std::thread::spawn(move || loop {
            match s.recv() {
                Ok(req) => {
                    let a2 = a.clone();
                    // chaque requête dans son propre fil : une conversion ou une recherche longue ne bloque pas la page
                    std::thread::spawn(move || a2.handle(req));
                }
                Err(_) => break,
            }
        }));
    }
    #[cfg(target_os = "macos")]
    if macos::in_bundle() {
        // app Mac : la boucle d'événements répond à « reopen » (relance depuis Spotlight / Dock) en rouvrant la page
        let u = url.clone();
        macos::run_app(Box::new(move || media::open_url(&u)));
    }
    for w in workers {
        let _ = w.join();
    }
}
