//! Petites fonctions utilitaires (chemins façon posix, échappement, texte).

use unicode_normalization::UnicodeNormalization;

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// posixpath.normpath
pub fn normpath(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let abs = p.starts_with('/');
    let mut out: Vec<&str> = vec![];
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if out.last().map(|l| *l != "..").unwrap_or(false) {
                    out.pop();
                } else if !abs {
                    out.push("..");
                }
            }
            x => out.push(x),
        }
    }
    let j = out.join("/");
    if abs {
        format!("/{}", j)
    } else if j.is_empty() {
        ".".into()
    } else {
        j
    }
}

/// posixpath.join (deux morceaux)
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') || a.is_empty() {
        b.to_string()
    } else if a.ends_with('/') {
        format!("{}{}", a, b)
    } else {
        format!("{}/{}", a, b)
    }
}

pub fn dirname(p: &str) -> String {
    match p.rfind('/') {
        Some(i) => p[..i].to_string(),
        None => String::new(),
    }
}

pub fn basename(p: &str) -> String {
    match p.rfind('/') {
        Some(i) => p[i + 1..].to_string(),
        None => p.to_string(),
    }
}

/// minuscules sans accents, pour comparer « Été » et « ete »
pub fn norm_text(t: &str) -> String {
    t.to_lowercase().nfkd().filter(|c| !unicode_normalization::char::is_combining_mark(*c)).collect()
}

pub fn unquote(s: &str) -> String {
    percent_encoding::percent_decode_str(s).decode_utf8_lossy().to_string()
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Chemin relatif « a/b/c » (séparateurs /) d'un chemin sous la racine.
pub fn rel_of(root: &std::path::Path, p: &std::path::Path) -> String {
    match p.strip_prefix(root) {
        Ok(r) => {
            let s: Vec<String> = r.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
            s.join("/")
        }
        Err(_) => String::new(),
    }
}

pub fn token() -> String {
    let mut b = [0u8; 18];
    let _ = getrandom::getrandom(&mut b);
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    b.iter().flat_map(|x| [A[(x >> 2) as usize], A[(x & 63) as usize]]).map(|c| c as char).collect()
}

/// JSON sur une ligne au format de Python (json.dumps(..., ensure_ascii=False)) : « {"a": 1, "b": [1, 2]} ».
pub fn py_json(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter().map(|(k, x)| format!("{}: {}", Value::String(k.clone()), py_json(x))).collect::<Vec<_>>().join(", ")
        ),
        Value::Array(a) => format!("[{}]", a.iter().map(py_json).collect::<Vec<_>>().join(", ")),
        x => x.to_string(),
    }
}
