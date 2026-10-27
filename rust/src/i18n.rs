//! Traduction FR -> EN des messages du serveur (même dictionnaire que la page : static/i18n/en.json).

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::RwLock;

pub static LANG: Lazy<RwLock<String>> = Lazy::new(|| RwLock::new(String::new()));

pub struct Dict {
    pub raw: Value,
    exact: HashMap<String, String>,
    pats: Vec<(Regex, String)>,
}

pub static DICT: Lazy<Dict> = Lazy::new(|| {
    let raw: Value = crate::assets::Static::get("i18n/en.json")
        .and_then(|f| serde_json::from_slice(&f.data).ok())
        .unwrap_or(Value::Object(Map::new()));
    let exact = raw
        .get("exact")
        .and_then(|o| o.as_object())
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect())
        .unwrap_or_default();
    let mut pats: Vec<(String, String)> = raw
        .get("patterns")
        .and_then(|o| o.as_object())
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect())
        .unwrap_or_default();
    pats.sort_by(|a, b| b.0.chars().count().cmp(&a.0.chars().count())); // clés longues d'abord
    let ph = Regex::new(r"\\\{(\d+)(#?)\\\}").unwrap();
    let pats = pats
        .into_iter()
        .filter_map(|(fr, en)| {
            let esc = regex::escape(&fr);
            let rx = ph.replace_all(&esc, |c: &regex::Captures| {
                if &c[2] == "#" {
                    format!(r"(?P<g{}>\d[\d\s.,]*)", &c[1])
                } else {
                    format!(r"(?P<g{}>[\s\S]*?)", &c[1])
                }
            });
            Regex::new(&format!("^{}$", rx)).ok().map(|r| (r, en))
        })
        .collect();
    Dict { raw, exact, pats }
});

pub fn lang() -> String {
    LANG.read().map(|l| l.clone()).unwrap_or_default()
}

pub fn set_lang(l: &str) {
    if let Ok(mut w) = LANG.write() {
        *w = l.to_string();
    }
}

fn core(s: &str) -> Option<String> {
    let d = &*DICT;
    if let Some(v) = d.exact.get(s) {
        return Some(v.clone());
    }
    let num = Regex::new(r"\{(\d+)\}").unwrap();
    for (rx, en) in &d.pats {
        if let Some(c) = rx.captures(s) {
            return Some(
                num.replace_all(en, |x: &regex::Captures| {
                    let v = c.name(&format!("g{}", &x[1])).map(|m| m.as_str()).unwrap_or("");
                    d.exact.get(v).cloned().unwrap_or_else(|| v.to_string())
                })
                .to_string(),
            );
        }
    }
    None
}

/// Texte traduit en anglais si l'interface est en anglais (sinon inchangé).
pub fn tr(s: &str) -> String {
    if lang() != "en" || s.trim().is_empty() {
        return s.to_string();
    }
    let lead = s.len() - s.trim_start().len();
    let trail = s.len() - s.trim_end().len();
    let mid = &s[lead..s.len() - trail];
    match core(mid) {
        Some(r) => format!("{}{}{}", &s[..lead], r, &s[s.len() - trail..]),
        None => s.to_string(),
    }
}

const KEYS: [&str; 5] = ["error", "why", "note", "notes", "status"];

/// Traduit les messages d'une réponse JSON (erreurs, statuts, notes, lignes du journal de Hermes Map).
pub fn tr_obj(v: Value) -> Value {
    if lang() != "en" {
        return v;
    }
    walk(v, None)
}

fn walk(v: Value, key: Option<&str>) -> Value {
    match v {
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, x)| { let y = walk(x, Some(&k)); (k, y) }).collect()),
        Value::Array(a) => {
            let k = key.unwrap_or("");
            Value::Array(
                a.into_iter()
                    .map(|x| match &x {
                        Value::String(s) if KEYS.contains(&k) => Value::String(tr(s)),
                        Value::String(s) if k == "lines" && s.contains("[Hermes Map]") => Value::String(tr(s)),
                        _ => walk(x, None),
                    })
                    .collect(),
            )
        }
        Value::String(s) if key.map(|k| KEYS.contains(&k)).unwrap_or(false) => Value::String(tr(&s)),
        other => other,
    }
}
