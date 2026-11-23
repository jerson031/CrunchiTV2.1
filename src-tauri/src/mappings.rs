use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSourceMapping {
    pub info_hash: String,
    pub magnet: String,
    pub title: String,
    pub spanish_score: u8,
    pub saved_at: u64,
}

fn get_mappings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("No se pudo obtener app_data_dir: {}", e))?;
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("No se pudo crear directorio de datos: {}", e))?;
    }
    Ok(dir.join("mappings.json"))
}

fn load_all(app: &AppHandle) -> HashMap<String, SavedSourceMapping> {
    let Ok(path) = get_mappings_path(app) else {
        return HashMap::new();
    };
    let Ok(content) = fs::read_to_string(&path) else {
        return HashMap::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

/// Letras que pueden ir pegadas antes de un número de episodio ("EP01-24", "E01-24")
fn is_episode_prefix(letters: &str) -> bool {
    matches!(
        letters,
        "" | "e" | "ep" | "eps" | "episode" | "episodes" | "episodio" | "episodios"
    )
}

/// ¿El título trae un rango de episodios tipo "01~25", "(01-24)", "1-12", "EP01-24"?
/// Exige al menos 4 episodios de diferencia (así "05-06", dobles, no cuenta) y descarta
/// resoluciones ("Hi10-720p"), códecs ("x265-10bit") y fechas ("2012-07-25").
pub fn has_episode_range(title: &str) -> bool {
    let c: Vec<char> = title.to_lowercase().chars().collect();
    let n = c.len();
    let mut i = 0;
    while i < n {
        if !c[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && c[i].is_ascii_digit() {
            i += 1;
        }
        let a_len = i - start;
        if a_len > 3 {
            continue;
        }
        // lo que va antes del primer número
        let mut p = start;
        let mut letters = String::new();
        while p > 0 && c[p - 1].is_ascii_alphabetic() {
            letters.insert(0, c[p - 1]);
            p -= 1;
        }
        if !is_episode_prefix(&letters) {
            continue;
        }
        if letters.is_empty() && start >= 2 && c[start - 1] == '-' && c[start - 2].is_ascii_digit()
        {
            continue; // 2012-07-25
        }
        // separador
        let mut j = i;
        while j < n && c[j] == ' ' {
            j += 1;
        }
        if j >= n || !matches!(c[j], '-' | '~' | '–' | '—') {
            continue;
        }
        j += 1;
        while j < n && c[j] == ' ' {
            j += 1;
        }
        let bs = j;
        while j < n && c[j].is_ascii_digit() {
            j += 1;
        }
        let b_len = j - bs;
        if b_len == 0 || b_len > 3 {
            continue;
        }
        // "720-1080p", "10-720p", "4k": resolución, no episodios
        if j < n && (c[j] == 'p' || c[j] == 'k' || c[j] == 'i' || c[j] == 'b' || c[j] == 'x') {
            continue;
        }
        let a: i32 = c[start..i].iter().collect::<String>().parse().unwrap_or(0);
        let b: i32 = c[bs..j].iter().collect::<String>().parse().unwrap_or(0);
        if b > a && b - a >= 3 && b <= 500 {
            return true;
        }
    }
    false
}

pub fn looks_like_season_batch(title: &str) -> bool {
    let low = title.to_lowercase();
    // Palabras que hablan de un pack
    let words = [
        "batch", "complete", "completa", "season 1", "season 2", "season 3", "season 4",
    ];
    if words.iter().any(|m| low.contains(m)) {
        return true;
    }
    if has_episode_range(&low) {
        return true;
    }
    // "S01", "S02"... solo si NO es "S01E05" (eso es un episodio suelto)
    let b = low.as_bytes();
    for k in 0..b.len().saturating_sub(2) {
        if b[k] == b's'
            && b[k + 1] == b'0'
            && (b'1'..=b'4').contains(&b[k + 2])
            && (k == 0 || !b[k - 1].is_ascii_alphanumeric())
        {
            let after = b.get(k + 3).copied().unwrap_or(b' ');
            if after != b'e' && !after.is_ascii_digit() {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod batch_tests {
    use super::*;

    #[test]
    fn detects_packs() {
        for t in [
            "[WZF]Sword_Art_Online_-_01~25[HD][X264-AAC][1280x720][Sub_Esp][MP4]",
            "[Group] Show (01-24) [1080p]",
            "Show 1-12 BD",
            "[G] Show - 01 ~ 12 [720p]",
            "Show EP01-24 1080p",
            "[G] Naruto 001-220 DVDRip",
            "[G] Show [01-12 Batch]",
            "Show Complete Series",
            "Show S01 1080p BDRip",
            "Show - Season 2 [Dual Audio]",
        ] {
            assert!(looks_like_season_batch(t), "debía ser pack: {}", t);
        }
    }

    #[test]
    fn single_episodes_are_not_packs() {
        for t in [
            "[HorribleSubs] Sword Art Online - 02 [1080p].mkv",
            "[G] Show - 05 [1080p]",
            "[G] Show - 05-06 [1080p]",
            "Show x265-10bit",
            "[G] Show Hi10-720p",
            "Show 2012-07-25",
            "Show - 12v2 [1080p]",
            "Show 1920x1080",
            "[Erai-raws] Show - 02 [1080p][Multiple Subtitle]",
            "Show S01E05 1080p",
            "[BD 1080p x265-AAC] Show - 03",
        ] {
            assert!(!looks_like_season_batch(t), "NO debía ser pack: {}", t);
        }
    }
}

pub fn get_saved_mapping(
    app: &AppHandle,
    anilist_id: i32,
    episode: i32,
) -> Option<SavedSourceMapping> {
    if anilist_id <= 0 {
        return None;
    }
    let map = load_all(app);
    let exact_key = format!("{}:{}", anilist_id, episode);

    if let Some(found) = map.get(&exact_key) {
        println!(
            "[Mappings HIT EXACTO] key={} | title='{}' | hash={}",
            exact_key, found.title, found.info_hash
        );
        return Some(found.clone());
    }

    let batch_key = format!("{}:batch", anilist_id);
    if let Some(found_batch) = map.get(&batch_key) {
        println!(
            "[Mappings HIT BATCH] key={} (Ep. {}) | title='{}' | hash={}",
            batch_key, episode, found_batch.title, found_batch.info_hash
        );
        return Some(found_batch.clone());
    }

    println!("[Mappings MISS] No hay fuente guardada para {}", exact_key);
    None
}

pub fn save_mapping(
    app: &AppHandle,
    anilist_id: i32,
    episode: i32,
    mapping: SavedSourceMapping,
) -> Result<(), String> {
    save_mapping_hint(app, anilist_id, episode, mapping, false)
}

/// `is_pack`: TorBox confirmó que el torrent trae varios episodios, aunque el título no lo diga.
pub fn save_mapping_hint(
    app: &AppHandle,
    anilist_id: i32,
    episode: i32,
    mapping: SavedSourceMapping,
    is_pack: bool,
) -> Result<(), String> {
    if anilist_id <= 0 {
        return Ok(());
    }
    let path = get_mappings_path(app)?;
    let mut map = load_all(app);
    let key = format!("{}:{}", anilist_id, episode);

    let batch_key = format!("{}:batch", anilist_id);
    if is_pack || looks_like_season_batch(&mapping.title) {
        println!(
            "[Mappings GUARDADO BATCH] key={} | title='{}'",
            batch_key, mapping.title
        );
        map.insert(batch_key, mapping.clone());
    }
    // Elegir una fuente individual para un episodio ya NO borra el pack guardado:
    // la fuente exacta del episodio tiene prioridad y los demás siguen usando el pack.

    println!(
        "[Mappings GUARDADO EXACTO] key={} | title='{}' | hash={}",
        key, mapping.title, mapping.info_hash
    );
    map.insert(key, mapping);

    let json = serde_json::to_string_pretty(&map)
        .map_err(|e| format!("Error serializando mappings.json: {}", e))?;
    fs::write(&path, &json).map_err(|e| format!("Error guardando mappings.json: {}", e))?;

    println!(
        "[Mappings DISK OK] Ruta: {:?} | Total entradas: {} | Keys: {:?}",
        path,
        map.len(),
        map.keys().collect::<Vec<_>>()
    );
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// Elección manual de subtítulo por episodio (archivo aparte: subtitle_choices.json)
// Se guarda en un archivo propio para no tocar el formato de mappings.json.
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSubtitleChoice {
    pub provider: String,
    pub label: String,
    pub url: String,
    pub path: String,
    pub format: String,
    pub score: u8,
    pub saved_at: u64,
}

fn get_data_file(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("No se pudo obtener app_data_dir: {}", e))?;
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("No se pudo crear directorio de datos: {}", e))?;
    }
    Ok(dir.join(name))
}

fn load_all_subs(app: &AppHandle) -> HashMap<String, SavedSubtitleChoice> {
    let Ok(path) = get_data_file(app, "subtitle_choices.json") else {
        return HashMap::new();
    };
    let Ok(content) = fs::read_to_string(&path) else {
        return HashMap::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

pub fn get_subtitle_choice(
    app: &AppHandle,
    anilist_id: i32,
    episode: i32,
) -> Option<SavedSubtitleChoice> {
    if anilist_id <= 0 {
        return None;
    }
    let key = format!("{}:{}", anilist_id, episode);
    let found = load_all_subs(app).get(&key).cloned();
    if let Some(ref c) = found {
        println!(
            "[Subtítulo ELEGIDO] key={} | {} ({}) | {} pts",
            key, c.label, c.provider, c.score
        );
    }
    found
}

pub fn save_subtitle_choice(
    app: &AppHandle,
    anilist_id: i32,
    episode: i32,
    choice: SavedSubtitleChoice,
) -> Result<(), String> {
    if anilist_id <= 0 {
        return Ok(());
    }
    let path = get_data_file(app, "subtitle_choices.json")?;
    let mut map = load_all_subs(app);
    let key = format!("{}:{}", anilist_id, episode);
    println!(
        "[Subtítulo GUARDADO] key={} | {} ({}) | {}",
        key, choice.label, choice.provider, choice.path
    );
    map.insert(key, choice);
    let json = serde_json::to_string_pretty(&map)
        .map_err(|e| format!("Error serializando subtitle_choices.json: {}", e))?;
    fs::write(&path, &json).map_err(|e| format!("Error guardando subtitle_choices.json: {}", e))
}

pub fn clear_subtitle_choice(app: &AppHandle, anilist_id: i32, episode: i32) -> Result<(), String> {
    let path = get_data_file(app, "subtitle_choices.json")?;
    let mut map = load_all_subs(app);
    let key = format!("{}:{}", anilist_id, episode);
    if map.remove(&key).is_some() {
        println!(
            "[Subtítulo LIMPIADO] key={} (vuelve la selección automática)",
            key
        );
        let json = serde_json::to_string_pretty(&map)
            .map_err(|e| format!("Error serializando subtitle_choices.json: {}", e))?;
        fs::write(&path, &json)
            .map_err(|e| format!("Error guardando subtitle_choices.json: {}", e))?;
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// Último episodio reproducido (en disco).
// El proceso principal de la app renace cada vez que abre el reproductor, así
// que esto NO puede vivir en memoria.
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedLastPlay {
    pub anilist_id: i32,
    pub episode: i32,
    pub stream_url: String,
    pub title: String,
    pub saved_at: u64,
}

pub fn save_last_play(
    app: &AppHandle,
    anilist_id: i32,
    episode: i32,
    stream_url: &str,
    title: &str,
) {
    let Ok(path) = get_data_file(app, "last_play.json") else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let lp = SavedLastPlay {
        anilist_id,
        episode,
        stream_url: stream_url.to_string(),
        title: title.to_string(),
        saved_at: now,
    };
    if let Ok(json) = serde_json::to_string(&lp) {
        let _ = fs::write(&path, json);
    }
}

/// Devuelve el último episodio reproducido si coincide con el pedido
pub fn load_last_play(app: &AppHandle, anilist_id: i32, episode: i32) -> Option<SavedLastPlay> {
    let path = get_data_file(app, "last_play.json").ok()?;
    let content = fs::read_to_string(&path).ok()?;
    let lp: SavedLastPlay = serde_json::from_str(&content).ok()?;
    if lp.anilist_id == anilist_id && lp.episode == episode {
        Some(lp)
    } else {
        None
    }
}
