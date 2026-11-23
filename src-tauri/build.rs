use std::{env, fs};

const KEYS: [&str; 2] = ["TORBOX_API_KEY", "GEMINI_API_KEY"];

/// Lee un archivo .env sencillo: KEY=valor, con o sin comillas, `export` opcional, # comentarios.
fn parse_env(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            let v = if v.len() >= 2
                && ((v.starts_with('"') && v.ends_with('"'))
                    || (v.starts_with('\'') && v.ends_with('\'')))
            {
                &v[1..v.len() - 1]
            } else {
                v
            };
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

fn main() {
    // .env junto a src-tauri (raíz del proyecto) o dentro de src-tauri
    let mut vars: Vec<(String, String)> = Vec::new();
    for path in ["../.env", ".env"] {
        println!("cargo:rerun-if-changed={}", path);
        if let Ok(text) = fs::read_to_string(path) {
            vars.extend(parse_env(&text));
        }
    }
    for key in KEYS {
        println!("cargo:rerun-if-env-changed={}", key);
        let from_file = vars
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone());
        let value = from_file
            .filter(|v| !v.is_empty())
            .or_else(|| env::var(key).ok().filter(|v| !v.trim().is_empty()));
        match value {
            Some(v) => println!("cargo:rustc-env={}={}", key, v),
            None => println!(
                "cargo:warning={} no está en .env: esa función no funcionará",
                key
            ),
        }
    }
    tauri_build::build()
}
