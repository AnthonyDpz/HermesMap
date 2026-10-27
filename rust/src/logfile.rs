//! Journal de l'app (utile quand elle est lancée par double-clic, sans Terminal).
//! Mac : ~/Library/Logs/Hermes Map.log — Windows / Linux : <données de l'app>/hermes-map.log

use std::io::Write;
use std::path::PathBuf;

pub fn path() -> PathBuf {
    if cfg!(target_os = "macos") {
        crate::config::home().join("Library/Logs/Hermes Map.log")
    } else {
        crate::config::data_dir().join("hermes-map.log")
    }
}

/// Ajoute une ligne datée au journal (et l'affiche dans le Terminal s'il y en a un).
pub fn line(msg: &str) {
    let l = format!("{} : {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
    println!("{}", l);
    let p = path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{}", l);
    }
}

/// Mode app (Mac, Linux) : tout ce qui est affiché part aussi dans le journal.
pub fn redirect_output() {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        extern "C" {
            fn dup2(src: i32, dst: i32) -> i32;
        }
        let p = path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
            unsafe {
                dup2(f.as_raw_fd(), 1);
                dup2(f.as_raw_fd(), 2);
            }
            std::mem::forget(f);
        }
    }
}
