use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::future::Future;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::task::JoinSet;
use tokio::time::timeout;

// ───────────────────────────── Endpoints ─────────────────────────────
/// ARM (mapeo AniList -> IMDb / TVDB)
const ARM_URL: &str = "https://arm.haglund.dev/api/v2/ids";
/// OpenSubtitles v3 (addon público de Stremio)
const OS_BASE: &str = "https://opensubtitles-v3.strem.io";
/// Animetosho: el feed se movió de dominio varias veces, probamos en orden.
const TOSHO_FEEDS: [&str; 2] = [
    "https://feed.animetosho.net/json",
    "https://feed.animetosho.org/json",
];
/// Gestdown (proxy de Addic7ed)
const GESTDOWN_BASE: &str = "https://api.gestdown.info";
/// SubDL (opcional: necesita API key gratuita, ver `set_subdl_key`)
const SUBDL_API: &str = "https://api.subdl.com/api/v1/subtitles";
const SUBDL_DL: &str = "https://dl.subdl.com";

const ARM_TIMEOUT: Duration = Duration::from_secs(4);
/// Máximo de candidatos que se descargan y analizan por búsqueda
const MAX_OPTIONS: usize = 8;
/// Puntaje mínimo para que la selección AUTOMÁTICA use un subtítulo
const MIN_AUTO_SCORE: u8 = 40;

// ==PURE-BEGIN==
// Todo lo de esta sección usa solo `std` (así se puede probar sin compilar el proyecto).

/// Magic bytes de un archivo .xz (Animetosho guarda adjuntos comprimidos así)
const XZ_MAGIC: [u8; 6] = [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00];

fn spanish_sub_priority(lang: &str, label: &str) -> u8 {
    let l = lang.to_lowercase();
    let t = label.to_lowercase();
    if t.contains("spain") || t.contains("españa") || t.contains("castellano") {
        return 40;
    }
    // Nota: antes se usaba `l.contains("la")`, pero eso también coincidía con
    // idiomas como "lav" (letón) o "lao". Ahora solo códigos tipo "es-LA".
    if l.contains("419")
        || l.contains("mx")
        || l.ends_with("-la")
        || l.ends_with("_la")
        || t.contains("latin")
        || t.contains("latino")
    {
        return 100;
    }
    if l == "spa" || l == "es" || t.contains("spanish") || t.contains("español") {
        return 80;
    }
    0
}

// ───────────────────────── Parseo de subtítulos ─────────────────────────

#[derive(Debug, Clone)]
pub struct Cue {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// "HH:MM:SS,mmm" | "HH:MM:SS.mmm" | "MM:SS.mmm" | "H:MM:SS.cc" (ASS)
fn parse_ts(s: &str) -> Option<i64> {
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.len() {
        3 => (parts[0], parts[1], parts[2]),
        2 => ("0", parts[0], parts[1]),
        _ => return None,
    };
    let h: i64 = h.trim().parse().ok()?;
    let m: i64 = m.trim().parse().ok()?;
    let sec: f64 = sec.trim().parse().ok()?;
    Some(((h * 3600 + m * 60) as f64 * 1000.0 + sec * 1000.0).round() as i64)
}

/// Quita etiquetas <i>, {\an8}, saltos \N, etc.
fn strip_markup(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut in_angle = false;
    let mut in_brace = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_brace {
            if c == '}' {
                in_brace = false;
            }
        } else if in_angle {
            if c == '>' {
                in_angle = false;
            }
        } else {
            match c {
                '{' => in_brace = true,
                '<' => in_angle = true,
                '\\' if i + 1 < chars.len() && matches!(chars[i + 1], 'N' | 'n' | 'h') => {
                    out.push(' ');
                    i += 1;
                }
                _ => out.push(c),
            }
        }
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn parse_cues(text: &str, ext: &str) -> Vec<Cue> {
    let mut cues = Vec::new();
    if ext == "ass" {
        for line in text.lines() {
            let Some(rest) = line.trim_start().strip_prefix("Dialogue:") else {
                continue;
            };
            let f: Vec<&str> = rest.splitn(10, ',').collect();
            if f.len() < 10 {
                continue;
            }
            let style = f[3].to_lowercase();
            if style.contains("sign") || style.contains("song") || style.contains("karaoke") {
                continue;
            }
            if f[9].contains("\\p1") {
                continue; // dibujo vectorial
            }
            let (Some(st), Some(en)) = (parse_ts(f[1]), parse_ts(f[2])) else {
                continue;
            };
            let t = strip_markup(f[9]);
            if !t.is_empty() {
                cues.push(Cue {
                    start_ms: st,
                    end_ms: en,
                    text: t,
                });
            }
        }
        return cues;
    }

    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if let Some((a, b)) = lines[i].split_once("-->") {
            let st = parse_ts(a);
            let en = parse_ts(b.split_whitespace().next().unwrap_or(""));
            i += 1;
            let mut buf: Vec<&str> = Vec::new();
            while i < lines.len() && !lines[i].trim().is_empty() {
                buf.push(lines[i]);
                i += 1;
            }
            if let (Some(st), Some(en)) = (st, en) {
                let t = strip_markup(&buf.join(" "));
                if !t.is_empty() {
                    cues.push(Cue {
                        start_ms: st,
                        end_ms: en,
                        text: t,
                    });
                }
            }
        } else {
            i += 1;
        }
    }
    cues
}

// ───────────────────────── Análisis de calidad ─────────────────────────

const ES_WORDS: &[&str] = &[
    "de", "la", "que", "el", "en", "y", "los", "se", "del", "las", "un", "por", "con", "no", "una",
    "su", "para", "es", "al", "lo", "como", "más", "pero", "sí", "me", "te", "qué", "está", "yo",
    "tú", "muy", "ya", "todo", "esto", "eso", "fue", "ser", "hay", "si", "le", "nos", "mi", "tu",
    "estoy", "bien", "porque", "cuando", "aquí", "ahora", "nada", "algo", "también",
];
const EN_WORDS: &[&str] = &[
    "the", "and", "you", "to", "is", "of", "that", "it", "in", "this", "i", "are", "was", "for",
    "not", "with", "he", "she", "we", "they", "have", "what", "my", "your", "do", "don't", "just",
    "be",
];
/// Palabras que existen en portugués pero no en español
const PT_WORDS: &[&str] = &[
    "não", "você", "vocês", "obrigado", "obrigada", "então", "também", "são", "muito", "isso",
    "estou", "pra", "ainda", "mas",
];
const SPAIN_WORDS: &[&str] = &[
    "vosotros", "vosotras", "vale", "tío", "tía", "joder", "coño", "ostras", "mola", "guay",
    "habéis", "tenéis", "sois", "estáis", "os",
];
const LATAM_WORDS: &[&str] = &[
    "ustedes", "ahorita", "órale", "chido", "carajo", "bacán", "pibe", "gringo", "celular",
    "chévere",
];

#[derive(Debug, Clone, Default)]
pub struct TextStats {
    pub words: usize,
    pub es: usize,
    pub en: usize,
    pub pt: usize,
    pub spain: usize,
    pub latam: usize,
    pub mojibake: usize,
}

/// Cuenta secuencias típicas de texto UTF-8 leído como Latin-1 ("Ã±", "â€™", "�")
pub fn count_mojibake(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut n = 0;
    for (i, &c) in chars.iter().enumerate() {
        if c == '\u{FFFD}' {
            n += 1;
        } else if (c == 'Ã' || c == 'Â') && i + 1 < chars.len() {
            let nx = chars[i + 1] as u32;
            if (0x80..=0xBF).contains(&nx) {
                n += 1;
            }
        } else if c == 'â' && i + 2 < chars.len() && chars[i + 1] == '€' {
            n += 1;
        }
    }
    n
}

/// Si el texto es UTF-8 que alguien leyó como Latin-1, lo repara
pub fn fix_mojibake(text: &str) -> Option<String> {
    if !text.chars().all(|c| (c as u32) < 256) {
        return None;
    }
    let bytes: Vec<u8> = text.chars().map(|c| c as u8).collect();
    String::from_utf8(bytes).ok()
}

pub fn text_stats(cues: &[Cue], raw_text: &str) -> TextStats {
    let mut st = TextStats::default();
    st.mojibake = count_mojibake(raw_text);
    for cue in cues.iter().take(1500) {
        let low = cue.text.to_lowercase();
        for w in low.split(|c: char| !(c.is_alphabetic() || c == '\'')) {
            if w.is_empty() {
                continue;
            }
            st.words += 1;
            if ES_WORDS.contains(&w) {
                st.es += 1;
            }
            if EN_WORDS.contains(&w) {
                st.en += 1;
            }
            if PT_WORDS.contains(&w) || w.contains('ç') || w.ends_with("ões") {
                st.pt += 1;
            }
            if SPAIN_WORDS.contains(&w) {
                st.spain += 1;
            }
            if LATAM_WORDS.contains(&w) {
                st.latam += 1;
            }
        }
    }
    st
}

#[derive(Debug, Clone)]
pub struct Judged {
    pub score: u8,
    pub label: String,
    pub notes: Vec<String>,
}

pub fn judge(
    stats: &TextStats,
    cues: &[Cue],
    video_secs: Option<f64>,
    exact: bool,
    prio: u8,
) -> Judged {
    let mut score: i32 = 60;
    let mut notes: Vec<String> = Vec::new();

    if prio >= 100 {
        score += 10;
    } else if prio >= 80 {
        score += 5;
    }

    let words = stats.words.max(1) as f64;
    let es = stats.es as f64 / words;
    let en = stats.en as f64 / words;
    let pt = stats.pt as f64 / words;

    if stats.words < 30 {
        score -= 30;
        notes.push("Casi no tiene texto".to_string());
    }
    if en > es * 1.2 && en > 0.10 {
        score -= 70;
        notes.push("Parece estar en inglés".to_string());
    } else if pt > 0.015 && stats.pt >= 5 {
        score -= 70;
        notes.push("Parece portugués".to_string());
    } else if es < 0.12 {
        score -= 55;
        notes.push("No parece español".to_string());
    }
    if stats.mojibake > 3 {
        score -= 25;
        notes.push("Caracteres rotos (mala codificación)".to_string());
    }

    if let Some(v) = video_secs {
        if v > 60.0 && !cues.is_empty() {
            let last = cues.iter().map(|c| c.end_ms).max().unwrap_or(0) as f64 / 1000.0;
            let ratio = last / v;
            if ratio > 1.08 {
                score -= 30;
                notes.push(format!(
                    "Dura más que tu video ({:.0}%): probable otra versión",
                    ratio * 100.0
                ));
            } else if ratio < 0.5 {
                score -= 20;
                notes.push("Termina muy pronto (¿incompleto o de otra versión?)".to_string());
            } else if ratio >= 0.75 {
                score += 8;
            }
        }
    }

    if exact {
        score += 20;
    }

    let label = if prio == 40 && stats.latam <= stats.spain {
        "Español (España)"
    } else if prio >= 100 || stats.latam > stats.spain {
        "Español Latino"
    } else if stats.spain >= 2 && stats.spain > stats.latam {
        "Español (España)"
    } else {
        "Español"
    };

    Judged {
        score: score.clamp(0, 100) as u8,
        label: label.to_string(),
        notes,
    }
}

/// 3 líneas de diálogo repartidas por el archivo, para reconocerlo de un vistazo
pub fn preview_lines(cues: &[Cue]) -> Vec<String> {
    let n = cues.len();
    if n == 0 {
        return Vec::new();
    }
    let mut idxs = vec![n / 4, n / 2, (3 * n) / 4];
    idxs.dedup();
    idxs.iter()
        .map(|&i| {
            let t = &cues[i.min(n - 1)].text;
            if t.chars().count() > 90 {
                let cut: String = t.chars().take(90).collect();
                format!("{}…", cut)
            } else {
                t.clone()
            }
        })
        .collect()
}

// ───────────────────────── Codificación / formato ─────────────────────────

/// Convierte a UTF-8 si el archivo viene en UTF-16 o Latin-1 / Windows-1252
fn to_utf8(data: Vec<u8>) -> Vec<u8> {
    if data.len() >= 2
        && ((data[0] == 0xFF && data[1] == 0xFE) || (data[0] == 0xFE && data[1] == 0xFF))
    {
        let le = data[0] == 0xFF;
        let units: Vec<u16> = data[2..]
            .chunks_exact(2)
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        return String::from_utf16_lossy(&units).into_bytes();
    }
    if std::str::from_utf8(&data).is_ok() {
        return data;
    }
    data.iter()
        .map(|&b| match b {
            0x85 => '\u{2026}',
            0x91 => '\u{2018}',
            0x92 => '\u{2019}',
            0x93 => '\u{201C}',
            0x94 => '\u{201D}',
            0x96 => '\u{2013}',
            0x97 => '\u{2014}',
            _ => b as char,
        })
        .collect::<String>()
        .into_bytes()
}

/// Bytes crudos -> texto UTF-8 limpio (sin BOM, con mojibake reparado)
pub fn decode_subtitle(content: Vec<u8>) -> String {
    let bytes = to_utf8(content);
    let mut s = String::from_utf8_lossy(&bytes).to_string();
    s = s.trim_start_matches('\u{feff}').to_string();
    if count_mojibake(&s) > 3 {
        if let Some(fixed) = fix_mojibake(&s) {
            s = fixed;
        }
    }
    s
}

/// Detecta el formato por contenido; si no se reconoce, usa la pista (ass / vtt / srt)
fn sniff_ext(content: &[u8], hint: &'static str) -> &'static str {
    let head = String::from_utf8_lossy(&content[..content.len().min(2048)]).to_lowercase();
    if head.contains("[script info]") || head.contains("[events]") {
        "ass"
    } else if head
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("webvtt")
    {
        "vtt"
    } else {
        hint
    }
}

fn looks_like_html(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]).to_lowercase();
    let h = head.trim_start();
    h.starts_with("<!doctype") || h.starts_with("<html")
}

// ───────────────────────── Hash de OpenSubtitles ─────────────────────────

/// Hash estándar de OpenSubtitles: tamaño + suma de u64 (little-endian) de los
/// primeros y últimos 64 KiB del archivo.
pub fn os_hash_from_parts(size: u64, head: &[u8], tail: &[u8]) -> Option<String> {
    if head.len() != 65536 || tail.len() != 65536 {
        return None;
    }
    let mut h: u64 = size;
    for chunk in head.chunks_exact(8).chain(tail.chunks_exact(8)) {
        h = h.wrapping_add(u64::from_le_bytes(chunk.try_into().ok()?));
    }
    Some(format!("{:016x}", h))
}

// ───────────────────────── Lectura del encabezado MKV ─────────────────────────

const EBML_MAGIC: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

#[derive(Debug, Clone)]
pub struct MkvSubTrack {
    pub lang: String,
    pub name: String,
    pub codec: String,
}

#[derive(Debug, Clone, Default)]
pub struct MkvProbe {
    pub tracks: Vec<MkvSubTrack>,
    pub duration_secs: Option<f64>,
}

/// Lee un ID EBML (conserva los bits marcadores). Devuelve (id, bytes usados)
fn read_vint_id(b: &[u8], pos: usize) -> Option<(u32, usize)> {
    let first = *b.get(pos)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 4 || pos + len > b.len() {
        return None;
    }
    let mut v: u32 = 0;
    for i in 0..len {
        v = (v << 8) | b[pos + i] as u32;
    }
    Some((v, len))
}

/// Lee un tamaño EBML. `None` interno = tamaño desconocido.
fn read_vint_size(b: &[u8], pos: usize) -> Option<(Option<u64>, usize)> {
    let first = *b.get(pos)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || pos + len > b.len() {
        return None;
    }
    let mut v: u64 = (first as u64) & ((1u64 << (8 - len)) - 1);
    for i in 1..len {
        v = (v << 8) | b[pos + i] as u64;
    }
    let all_ones = (1u64 << (7 * len)) - 1;
    Some((if v == all_ones { None } else { Some(v) }, len))
}

fn ebml_uint(data: &[u8]) -> u64 {
    data.iter().fold(0u64, |a, &x| (a << 8) | x as u64)
}

fn ebml_str(data: &[u8]) -> String {
    String::from_utf8_lossy(data)
        .trim_end_matches('\0')
        .to_string()
}

fn parse_track_entry(data: &[u8]) -> Option<MkvSubTrack> {
    let mut pos = 0usize;
    let mut ttype = 0u64;
    let mut lang = String::new();
    let mut bcp47 = String::new();
    let mut name = String::new();
    let mut codec = String::new();

    while pos < data.len() {
        let Some((id, il)) = read_vint_id(data, pos) else {
            break;
        };
        let Some((Some(size), sl)) = read_vint_size(data, pos + il) else {
            break;
        };
        let start = pos + il + sl;
        let end = start.saturating_add(size as usize);
        if end > data.len() {
            break;
        }
        let body = &data[start..end];
        match id {
            0x83 => ttype = ebml_uint(body),
            0x22B59C => lang = ebml_str(body),
            0x22B59D => bcp47 = ebml_str(body),
            0x536E => name = ebml_str(body),
            0x86 => codec = ebml_str(body),
            _ => {}
        }
        pos = end;
    }

    if ttype != 17 {
        return None; // 17 = pista de subtítulos
    }
    // Por especificación, sin idioma declarado se asume inglés
    let lang = if !bcp47.is_empty() {
        bcp47
    } else if lang.is_empty() {
        "eng".to_string()
    } else {
        lang
    };
    Some(MkvSubTrack { lang, name, codec })
}

/// Duración en segundos desde el elemento Info (Duration * TimecodeScale)
fn parse_info_duration(body: &[u8]) -> Option<f64> {
    let mut pos = 0usize;
    let mut scale: u64 = 1_000_000;
    let mut dur: Option<f64> = None;
    while pos < body.len() {
        let Some((id, il)) = read_vint_id(body, pos) else {
            break;
        };
        let Some((Some(size), sl)) = read_vint_size(body, pos + il) else {
            break;
        };
        let start = pos + il + sl;
        let end = start.saturating_add(size as usize);
        if end > body.len() {
            break;
        }
        let b = &body[start..end];
        match id {
            0x2AD7B1 => scale = ebml_uint(b),
            0x4489 => {
                dur = match b.len() {
                    4 => Some(f32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64),
                    8 => Some(f64::from_be_bytes([
                        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                    ])),
                    _ => None,
                }
            }
            _ => {}
        }
        pos = end;
    }
    dur.map(|d| d * scale as f64 / 1_000_000_000.0)
        .filter(|d| d.is_finite() && *d > 0.0)
}

/// Busca Info y Tracks dentro del buffer.
/// `None` = no se pudo determinar (archivo no MKV, buffer incompleto, etc.)
pub fn parse_mkv(buf: &[u8]) -> Option<MkvProbe> {
    if !buf.starts_with(&EBML_MAGIC) {
        return None;
    }
    let mut duration: Option<f64> = None;
    let mut pos = 0usize;
    while pos < buf.len() {
        let (id, il) = read_vint_id(buf, pos)?;
        let (size, sl) = read_vint_size(buf, pos + il)?;
        let start = pos + il + sl;
        match id {
            0x18538067 => {
                // Segment: entramos a sus hijos
                pos = start;
            }
            0x1F43B675 => return None, // Cluster: ya hay datos y no vimos Tracks
            0x1549A966 => {
                let end = start.checked_add(size? as usize)?;
                if end > buf.len() {
                    return None;
                }
                duration = parse_info_duration(&buf[start..end]);
                pos = end;
            }
            0x1654AE6B => {
                let end = start.checked_add(size? as usize)?;
                if end > buf.len() {
                    return None;
                }
                let body = &buf[start..end];
                let mut tracks = Vec::new();
                let mut p = 0usize;
                while p < body.len() {
                    let Some((eid, eil)) = read_vint_id(body, p) else {
                        break;
                    };
                    let Some((Some(esz), esl)) = read_vint_size(body, p + eil) else {
                        break;
                    };
                    let es = p + eil + esl;
                    let ee = es.saturating_add(esz as usize);
                    if ee > body.len() {
                        break;
                    }
                    if eid == 0xAE {
                        if let Some(t) = parse_track_entry(&body[es..ee]) {
                            tracks.push(t);
                        }
                    }
                    p = ee;
                }
                return Some(MkvProbe {
                    tracks,
                    duration_secs: duration,
                });
            }
            _ => {
                pos = start.checked_add(size? as usize)?;
            }
        }
    }
    None
}
/// Si un ZIP trae varios subtítulos (pack de temporada), elige el del episodio pedido.
/// `names` son los nombres de archivo de subtítulo dentro del ZIP. Devuelve el índice en `names`.
fn pick_zip_entry(names: &[String], episode: i32) -> Option<usize> {
    if names.is_empty() {
        return None;
    }
    if names.len() == 1 || episode <= 0 {
        return Some(0);
    }
    let two = format!("{:02}", episode);
    let pats = [
        format!("e{}", two),
        format!("ep{}", two),
        format!("x{}", two),
        format!(" {} ", two),
        format!("- {}", two),
        format!("_{}_", two),
        format!(".{}.", two),
    ];
    let mut best: Option<usize> = None;
    for (i, n) in names.iter().enumerate() {
        let low = n.to_lowercase();
        // evitamos confundir E1 con E12: el patrón no debe continuar con otro dígito
        let hit = pats.iter().any(|p| {
            low.match_indices(p.as_str()).any(|(pos, _)| {
                low[pos + p.len()..]
                    .chars()
                    .next()
                    .map_or(true, |c| !c.is_ascii_digit())
            })
        });
        if hit {
            best = Some(i);
            break;
        }
    }
    best.or(Some(0))
}

// ───────────────────────── IA para la TV: lógica pura ─────────────────────────

/// Un subtítulo en inglés ya descargado y analizado
#[derive(Debug, Clone)]
pub struct EnCand {
    pub exact: bool,
    pub cues: usize,
    pub last_end_ms: i64,
    /// El texto realmente parece inglés
    pub english_ok: bool,
}

/// Elige el mejor subtítulo en inglés: que sea inglés, con diálogos suficientes y, si se conoce
/// la duración del video, que termine cerca del final (si no, es de otra versión y se desincroniza).
pub fn pick_english_candidate(c: &[EnCand], video_secs: Option<f64>) -> Option<usize> {
    let mut best: Option<(usize, i64)> = None;
    for (i, x) in c.iter().enumerate() {
        if !x.english_ok || x.cues < 60 {
            continue;
        }
        let mut score: i64 = 0;
        if let Some(v) = video_secs {
            if v > 60.0 {
                let ratio = x.last_end_ms as f64 / 1000.0 / v;
                if ratio > 1.10 || ratio < 0.60 {
                    continue; // otra versión del video: se desincronizaría
                }
                if ratio >= 0.80 {
                    score += 50;
                }
            }
        }
        if x.exact {
            score += 100;
        }
        score += (x.cues as i64 / 10).min(30);
        if best.map_or(true, |(_, b)| score > b) {
            best = Some((i, score));
        }
    }
    best.map(|(i, _)| i)
}

/// Prompt de traducción: líneas numeradas + las 3 anteriores como contexto (no se traducen)
pub fn build_translate_prompt(
    anime: &str,
    episode: i32,
    lines: &[(usize, String)],
    context: &[String],
) -> String {
    let mut p = format!(
        "Eres un traductor profesional de anime para Crunchyroll Latinoamérica.\n\
Serie: \"{}\" (episodio {}).\n\
Traduce los diálogos del inglés al Español Latino neutro, natural y conciso, como subtítulos.\n\
REGLAS ESTRICTAS:\n\
1. Devuelve UNA línea por cada línea numerada, con el MISMO prefijo [n]. No unas ni dividas líneas.\n\
2. Mantén los nombres propios de personajes y lugares; no traduzcas nombres de técnicas si son nombres propios.\n\
3. Textos entre paréntesis como (suspira) tradúcelos igual, entre paréntesis.\n\
4. Sin explicaciones, sin comillas, sin comentarios. Solo las líneas traducidas.\n",
        anime, episode
    );
    if !context.is_empty() {
        p.push_str("\nCONTEXTO (líneas anteriores, NO las traduzcas):\n");
        for c in context {
            p.push_str(&format!("- {}\n", c));
        }
    }
    p.push_str("\nTRADUCE ESTAS LÍNEAS:\n");
    for (i, t) in lines {
        p.push_str(&format!("[{}] {}\n", i, t));
    }
    p
}

/// Lee "[n] texto" de la respuesta. Devuelve un Option por cada índice 0..n.
pub fn parse_numbered_lines(output: &str, n: usize) -> Vec<Option<String>> {
    let mut res: Vec<Option<String>> = vec![None; n];
    for line in output.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('[') {
            if let Some((idx, txt)) = rest.split_once(']') {
                if let Ok(i) = idx.trim().parse::<usize>() {
                    let txt = txt.trim();
                    if i < n && !txt.is_empty() && res[i].is_none() {
                        res[i] = Some(txt.to_string());
                    }
                }
            }
        }
    }
    res
}

/// Parte en 2 líneas equilibradas si es larga (pantalla de TV)
pub fn wrap_for_tv(text: &str) -> String {
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= 44 {
        return clean;
    }
    let words: Vec<&str> = clean.split(' ').collect();
    if words.len() < 2 {
        return clean;
    }
    let total = clean.chars().count();
    let mut acc = 0usize;
    let mut split = words.len() - 1;
    for (i, w) in words.iter().enumerate() {
        acc += w.chars().count() + 1;
        if acc >= total / 2 {
            split = (i + 1).min(words.len() - 1);
            break;
        }
    }
    format!("{}\n{}", words[..split].join(" "), words[split..].join(" "))
}

fn fmt_srt_ts(ms: i64) -> String {
    let ms = ms.max(0);
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        ms % 1000
    )
}

/// Arma el .srt con los tiempos originales (así queda sincronizado igual que el inglés)
pub fn cues_to_srt(cues: &[Cue], texts: &[String]) -> String {
    let mut out = String::new();
    let mut n = 0;
    for (c, t) in cues.iter().zip(texts.iter()) {
        if t.trim().is_empty() {
            continue;
        }
        n += 1;
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            n,
            fmt_srt_ts(c.start_ms),
            fmt_srt_ts(c.end_ms.max(c.start_ms + 300)),
            wrap_for_tv(t)
        ));
    }
    out
}

#[cfg(test)]
mod ai_tests {
    use super::*;

    fn en(exact: bool, cues: usize, last_s: i64) -> EnCand {
        EnCand {
            exact,
            cues,
            last_end_ms: last_s * 1000,
            english_ok: true,
        }
    }

    #[test]
    fn prefers_exact_and_matching_duration() {
        let c = vec![
            en(false, 400, 1400),
            en(true, 380, 1420),
            en(false, 500, 1900),
        ];
        assert_eq!(pick_english_candidate(&c, Some(1440.0)), Some(1));
    }

    #[test]
    fn rejects_other_versions_by_duration() {
        let c = vec![en(false, 400, 2400), en(false, 300, 600)];
        assert_eq!(pick_english_candidate(&c, Some(1440.0)), None);
    }

    #[test]
    fn rejects_non_english_and_tiny() {
        let mut a = en(true, 400, 1400);
        a.english_ok = false;
        let b = en(true, 20, 1400);
        assert_eq!(pick_english_candidate(&[a, b], Some(1440.0)), None);
    }

    #[test]
    fn unknown_duration_still_picks() {
        let c = vec![en(false, 100, 100), en(false, 200, 100)];
        assert_eq!(pick_english_candidate(&c, None), Some(1));
    }

    #[test]
    fn parses_numbered_output() {
        let out = "[0] Hola\n[2] Adiós\n  [1]   ¿Qué?  \nbasura\n[9] fuera de rango\n[0] repetida";
        let r = parse_numbered_lines(out, 3);
        assert_eq!(
            r,
            vec![
                Some("Hola".into()),
                Some("¿Qué?".into()),
                Some("Adiós".into())
            ]
        );
    }

    #[test]
    fn missing_lines_are_none() {
        let r = parse_numbered_lines("[0] a\n[2] c", 3);
        assert_eq!(r[1], None);
    }

    #[test]
    fn wraps_long_lines_only() {
        assert_eq!(wrap_for_tv("Hola"), "Hola");
        let w = wrap_for_tv("Esta es una frase bastante larga que necesita partirse en dos líneas");
        assert_eq!(w.lines().count(), 2);
        assert!(w.lines().all(|l| l.chars().count() <= 44), "{}", w);
    }

    #[test]
    fn srt_has_original_timing() {
        let cues = vec![
            Cue {
                start_ms: 1500,
                end_ms: 3200,
                text: "Hi".into(),
            },
            Cue {
                start_ms: 3_661_005,
                end_ms: 3_662_000,
                text: "Bye".into(),
            },
        ];
        let srt = cues_to_srt(&cues, &["Hola".into(), "Adiós".into()]);
        assert!(srt.contains("1\n00:00:01,500 --> 00:00:03,200\nHola\n"));
        assert!(srt.contains("2\n01:01:01,005 --> 01:01:02,000\nAdiós\n"));
    }

    #[test]
    fn prompt_contains_numbers_and_context() {
        let p = build_translate_prompt(
            "Naruto",
            3,
            &[(0, "Hi".into()), (1, "Bye".into())],
            &["Before".into()],
        );
        assert!(
            p.contains("[0] Hi")
                && p.contains("[1] Bye")
                && p.contains("- Before")
                && p.contains("Naruto")
        );
    }
}

// ==PURE-END==

// ───────────────────────────── Red y utilidades ─────────────────────────────

#[derive(Default)]
struct ResolvedIds {
    imdb: Option<String>,
    tvdb: Option<i64>,
}

/// Convierte un AniList ID a IMDb / TVDB mediante la API abierta ARM
async fn resolve_ids(client: &Client, anilist_id: i32) -> ResolvedIds {
    let mut out = ResolvedIds::default();
    if anilist_id <= 0 {
        return out;
    }
    let url = format!(
        "{}?source=anilist&id={}&include=imdb,thetvdb",
        ARM_URL, anilist_id
    );
    let Some(body) = get_json(client, &url).await else {
        return out;
    };
    out.imdb = body["imdb"]
        .as_str()
        .filter(|s| s.starts_with("tt"))
        .map(|s| s.to_string());
    out.tvdb = body["thetvdb"].as_i64();
    out
}

/// GET que devuelve JSON solo si la respuesta fue 2xx y es JSON válido
async fn get_json(client: &Client, url: &str) -> Option<Value> {
    let res = client.get(url).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    res.json::<Value>().await.ok()
}

/// Pide varias URLs en paralelo. Devuelve (índice original, JSON) ordenado por índice.
async fn fetch_json_many(client: &Client, urls: Vec<String>) -> Vec<(usize, Option<Value>)> {
    let mut set = JoinSet::new();
    for (i, u) in urls.into_iter().enumerate() {
        let c = client.clone();
        set.spawn(async move { (i, get_json(&c, &u).await) });
    }
    let mut out = Vec::new();
    while let Some(r) = set.join_next().await {
        if let Ok(pair) = r {
            out.push(pair);
        }
    }
    out.sort_by_key(|p| p.0);
    out
}

/// Candidato crudo (todavía sin descargar)
#[derive(Debug, Clone)]
struct RawCand {
    provider: &'static str,
    prio: u8,
    url: String,
    /// Hecho para el archivo exacto del usuario (coincidencia por hash)
    exact: bool,
}

/// Mejor subtítulo español dentro de una respuesta de Stremio
fn spanish_in_stremio(body: &Value) -> Vec<(u8, String)> {
    let mut out = Vec::new();
    if let Some(subs) = body["subtitles"].as_array() {
        for s in subs {
            let lang = s["lang"].as_str().unwrap_or("");
            let id_label = s["id"].as_str().unwrap_or("");
            let prio = spanish_sub_priority(lang, id_label);
            if prio > 0 {
                if let Some(u) = s["url"].as_str() {
                    out.push((prio, u.to_string()));
                }
            }
        }
    }
    out
}

fn filename_from_url(u: &str) -> Option<String> {
    let path = u.split('?').next()?;
    let last = path.rsplit('/').next()?;
    let decoded = urlencoding::decode(last).ok()?.to_string();
    let low = decoded.to_lowercase();
    if low.ends_with(".mkv") || low.ends_with(".mp4") || low.ends_with(".avi") {
        Some(decoded)
    } else {
        None
    }
}

/// Calcula el hash de OpenSubtitles de un archivo remoto con 2 peticiones Range (128 KiB)
async fn compute_os_hash(stream_url: &str) -> Option<(String, u64)> {
    let client = crate::build_http_client(8).ok()?;
    let resp = client
        .get(stream_url)
        .header("Range", "bytes=0-65535")
        .send()
        .await
        .ok()?;
    if resp.status().as_u16() != 206 {
        return None;
    }
    let size: u64 = resp
        .headers()
        .get("content-range")?
        .to_str()
        .ok()?
        .rsplit('/')
        .next()?
        .trim()
        .parse()
        .ok()?;
    if size < 131_072 {
        return None;
    }
    let head = resp.bytes().await.ok()?;
    let resp2 = client
        .get(stream_url)
        .header("Range", format!("bytes={}-{}", size - 65536, size - 1))
        .send()
        .await
        .ok()?;
    if resp2.status().as_u16() != 206 {
        return None;
    }
    let tail = resp2.bytes().await.ok()?;
    let h = os_hash_from_parts(size, &head, &tail)?;
    Some((h, size))
}

// ───────────────────────────── Proveedores ─────────────────────────────

/// Proveedor 1: OpenSubtitles v3 (Stremio). Devuelve TODOS los subtítulos en español.
/// Con hash del archivo, los hechos para esa copia exacta se marcan como `exact`.
async fn collect_opensubtitles(
    client: &Client,
    imdb_id: &str,
    episode: i32,
    hash: Option<&(String, u64)>,
    filename: Option<&str>,
) -> Vec<RawCand> {
    let mut urls: Vec<String> = Vec::new();
    for season in 1..=6 {
        urls.push(format!(
            "{}/subtitles/series/{}:{}:{}.json",
            OS_BASE, imdb_id, season, episode
        ));
    }
    let plain_n = urls.len();
    if let Some((h, size)) = hash {
        let mut extra = format!("videoHash={}&videoSize={}", h, size);
        if let Some(f) = filename {
            extra.push_str(&format!("&filename={}", urlencoding::encode(f)));
        }
        for season in 1..=6 {
            urls.push(format!(
                "{}/subtitles/series/{}:{}:{}/{}.json",
                OS_BASE, imdb_id, season, episode, extra
            ));
        }
    }

    let results = fetch_json_many(client, urls).await;

    let mut plain: Vec<(u8, String)> = Vec::new();
    let mut hashed: Vec<(u8, String)> = Vec::new();
    for (i, body) in results {
        if let Some(b) = body {
            if i < plain_n {
                plain.extend(spanish_in_stremio(&b));
            } else {
                hashed.extend(spanish_in_stremio(&b));
            }
        }
    }

    // Película / OVA único (episodio 1): probamos también el endpoint /movie/
    if plain.is_empty() && hashed.is_empty() && episode == 1 {
        let movie_url = format!("{}/subtitles/movie/{}.json", OS_BASE, imdb_id);
        if let Some(b) = get_json(client, &movie_url).await {
            plain.extend(spanish_in_stremio(&b));
        }
    }

    let plain_set: HashSet<String> = plain.iter().map(|p| p.1.clone()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (prio, url) in hashed {
        // Solo aparece con hash y no en la lista normal => hecho para tu archivo
        let exact = !plain_set.contains(&url);
        if seen.insert(url.clone()) {
            out.push(RawCand {
                provider: "OpenSubtitles",
                prio,
                url,
                exact,
            });
        }
    }
    for (prio, url) in plain {
        if seen.insert(url.clone()) {
            out.push(RawCand {
                provider: "OpenSubtitles",
                prio,
                url,
                exact: false,
            });
        }
    }
    out
}

fn clean_title(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Recorre un JSON de Animetosho buscando adjuntos de subtítulo (.srt/.ass/.ssa/.vtt)
/// con una URL de descarga. Es tolerante con la forma exacta del JSON.
fn collect_tosho_subs(v: &Value, depth: u8, found: &mut Vec<(u8, String)>) {
    if depth > 8 {
        return;
    }
    match v {
        Value::Object(map) => {
            let fname = map
                .get("filename")
                .or_else(|| map.get("name"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_lowercase();
            let url = map
                .get("url")
                .or_else(|| map.get("link"))
                .or_else(|| map.get("download_url"))
                .and_then(|x| x.as_str());

            if let Some(u) = url {
                let is_sub = fname.ends_with(".srt")
                    || fname.ends_with(".ass")
                    || fname.ends_with(".ssa")
                    || fname.ends_with(".vtt");
                // Los .xz no los podemos descomprimir (ver fetch_subtitle_content)
                if is_sub && !u.to_lowercase().ends_with(".xz") {
                    let lang = map
                        .get("lang")
                        .and_then(|x| x.as_str())
                        .or_else(|| {
                            map.get("info")
                                .and_then(|i| i.get("lang"))
                                .and_then(|x| x.as_str())
                        })
                        .unwrap_or("");
                    let extra = map
                        .get("info")
                        .and_then(|i| i.get("name").or_else(|| i.get("title")))
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    let label = format!("{} {}", fname, extra);
                    let prio = spanish_sub_priority(lang, &label);
                    if prio > 0 {
                        found.push((prio, u.to_string()));
                    }
                }
            }
            for child in map.values() {
                collect_tosho_subs(child, depth + 1, found);
            }
        }
        Value::Array(arr) => {
            for child in arr {
                collect_tosho_subs(child, depth + 1, found);
            }
        }
        _ => {}
    }
}

/// Pide JSON al feed de Animetosho. Si se da `host_hint` solo usa ese host.
async fn tosho_json(
    client: &Client,
    query: &str,
    host_hint: Option<&'static str>,
) -> Option<(&'static str, Value)> {
    let hosts: Vec<&'static str> = match host_hint {
        Some(h) => vec![h],
        None => TOSHO_FEEDS.to_vec(),
    };
    for host in hosts {
        let url = format!("{}?{}", host, query);
        if let Some(v) = get_json(client, &url).await {
            return Some((host, v));
        }
    }
    None
}

/// Proveedor 2: Animetosho JSON (pistas extraídas de torrents Multi-Sub reales)
async fn collect_animetosho(client: &Client, raw_titles: &[String], episode: i32) -> Vec<RawCand> {
    let ep_str = if episode >= 100 {
        format!("{}", episode)
    } else {
        format!("{:02}", episode)
    };

    let mut found: Vec<(u8, String)> = Vec::new();
    for raw in raw_titles.iter().take(2) {
        let clean = clean_title(raw);
        if clean.len() < 3 {
            continue;
        }
        let query = format!("{} {}", clean, ep_str);
        let q = format!("q={}", urlencoding::encode(&query));

        let Some((host, body)) = tosho_json(client, &q, None).await else {
            continue;
        };
        let Some(entries) = body.as_array() else {
            continue;
        };

        // 1) Por si la búsqueda ya trae adjuntos
        for e in entries.iter().take(8) {
            collect_tosho_subs(&e["attachments"], 0, &mut found);
        }

        // 2) Paso extra: el detalle del torrent es donde viven los adjuntos (en paralelo)
        if found.is_empty() {
            let urls: Vec<String> = entries
                .iter()
                .take(5)
                .filter_map(|e| e["id"].as_i64())
                .map(|id| format!("{}?show=torrent&id={}", host, id))
                .collect();
            for (_, v) in fetch_json_many(client, urls).await {
                if let Some(v) = v {
                    collect_tosho_subs(&v, 0, &mut found);
                }
            }
        }
        if !found.is_empty() {
            break;
        }
    }

    let mut seen = HashSet::new();
    found
        .into_iter()
        .filter(|(_, u)| seen.insert(u.clone()))
        .map(|(prio, url)| RawCand {
            provider: "Animetosho",
            prio,
            url,
            exact: false,
        })
        .collect()
}

/// Proveedor 3: Gestdown API (Espejo abierto JSON de Addic7ed sin CAPTCHA).
/// Gestdown busca por TVDB ID (no por IMDb).
async fn collect_gestdown(client: &Client, tvdb_id: i64, episode: i32) -> Vec<RawCand> {
    let show_url = format!("{}/shows/external/tvdb/{}", GESTDOWN_BASE, tvdb_id);
    let Some(body) = get_json(client, &show_url).await else {
        return Vec::new();
    };
    let Some(show_id) = body["shows"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|s| s["id"].as_str())
    else {
        return Vec::new();
    };

    let urls: Vec<String> = (1..=4)
        .map(|season| {
            format!(
                "{}/subtitles/get/{}/{}/{}/es",
                GESTDOWN_BASE, show_id, season, episode
            )
        })
        .collect();

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (_, v) in fetch_json_many(client, urls).await {
        let Some(b) = v else { continue };
        let Some(arr) = b["matchingSubtitles"].as_array() else {
            continue;
        };
        for sub in arr.iter().take(3) {
            if let Some(dl) = sub["downloadUri"].as_str() {
                let full = if dl.starts_with("http") {
                    dl.to_string()
                } else {
                    format!("{}{}", GESTDOWN_BASE, dl)
                };
                if seen.insert(full.clone()) {
                    out.push(RawCand {
                        provider: "Gestdown",
                        prio: 75,
                        url: full,
                        exact: false,
                    });
                }
            }
        }
    }
    out
}

static SUBDL_KEY: OnceLock<String> = OnceLock::new();

/// Activa SubDL con tu API key (gratis en subdl.com). Se llama una vez al iniciar la app.
pub fn set_subdl_key(key: &str) {
    let k = key.trim();
    if !k.is_empty() {
        let _ = SUBDL_KEY.set(k.to_string());
    }
}

/// SubDL: busca por IMDb + temporada + episodio, idioma español.
async fn collect_subdl(client: &Client, imdb: &str, episode: i32) -> Vec<RawCand> {
    let Some(key) = SUBDL_KEY.get() else {
        return Vec::new();
    };
    let urls: Vec<String> = (1..=3)
        .map(|season| {
            format!(
                "{}?api_key={}&imdb_id={}&type=tv&season_number={}&episode_number={}&languages=ES",
                SUBDL_API, key, imdb, season, episode
            )
        })
        .collect();

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (_, v) in fetch_json_many(client, urls).await {
        let Some(b) = v else { continue };
        let Some(arr) = b["subtitles"].as_array() else {
            continue;
        };
        for sub in arr.iter().take(4) {
            let Some(path) = sub["url"].as_str() else {
                continue;
            };
            let full = if path.starts_with("http") {
                path.to_string()
            } else {
                format!("{}{}", SUBDL_DL, path)
            };
            let label = format!(
                "{} {}",
                sub["release_name"].as_str().unwrap_or(""),
                sub["name"].as_str().unwrap_or("")
            );
            let prio = spanish_sub_priority("es", &label).max(60);
            if seen.insert(full.clone()) {
                out.push(RawCand {
                    provider: "SubDL",
                    prio,
                    url: full,
                    exact: false,
                });
            }
        }
    }
    out
}

/// Ejecuta la búsqueda de un proveedor con tope de tiempo y deja rastro en el log
async fn with_timeout_vec<F>(name: &str, dur: Duration, fut: F) -> Vec<RawCand>
where
    F: Future<Output = Vec<RawCand>>,
{
    match timeout(dur, fut).await {
        Ok(v) => {
            println!("[Subtítulos] {}: {} candidato(s)", name, v.len());
            v
        }
        Err(_) => {
            println!("[Subtítulos] {}: tiempo agotado ({}s)", name, dur.as_secs());
            Vec::new()
        }
    }
}

// ───────────────────────────── Sondeo del MKV ─────────────────────────────
// El `spanish_score` sale solo del título del release (p. ej. "multisub" => 95),
// no del archivo real de cada capítulo. Aquí leemos el encabezado del MKV para
// saber qué subtítulos trae de verdad y cuánto dura.

/// Cuántos bytes del inicio del archivo leemos como máximo
const PROBE_BYTES: usize = 1_500_000;

pub async fn probe_mkv(url: &str) -> Option<MkvProbe> {
    let client = crate::build_http_client(10).ok()?;
    let mut resp = client
        .get(url)
        .header("Range", format!("bytes=0-{}", PROBE_BYTES - 1))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }

    let mut buf: Vec<u8> = Vec::with_capacity(256 * 1024);
    while buf.len() < PROBE_BYTES {
        match timeout(Duration::from_secs(6), resp.chunk()).await {
            Ok(Ok(Some(chunk))) => buf.extend_from_slice(&chunk),
            _ => break,
        }
        if buf.len() >= 4 && !buf.starts_with(&EBML_MAGIC) {
            return None; // no es MKV (p. ej. MP4): no podemos verificar
        }
        if let Some(p) = parse_mkv(&buf) {
            return Some(p);
        }
    }
    parse_mkv(&buf)
}

// ───────────────────── Duración de un MP4 (caja mvhd dentro de moov) ─────────────────────
// Un MP4 es una lista de cajas [tamaño][tipo][contenido]. La duración está en `mvhd`, primer hijo
// de `moov`. `moov` puede ir al inicio (faststart) o después de `mdat` (al final del archivo), así que
// se salta de caja en caja con peticiones Range pequeñas, sin bajar el video.

/// Bytes que se piden en cada salto (de sobra para una cabecera de caja + mvhd)
const MP4_FETCH: u64 = 4096;

#[derive(Debug, PartialEq)]
enum Mp4Step {
    Duration(f64),
    /// Salta tantos bytes desde el inicio de esta caja hasta la siguiente
    Skip(u64),
    Fail,
}

fn be_u32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn be_u64(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 8)
        .map(|s| u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

/// Contenido de `mvhd` (empieza en versión+flags) -> segundos
fn parse_mvhd_secs(c: &[u8]) -> Option<f64> {
    let (timescale, duration) = match *c.first()? {
        0 => (be_u32(c, 12)? as u64, be_u32(c, 16)? as u64),
        1 => (be_u32(c, 20)? as u64, be_u64(c, 24)?),
        _ => return None,
    };
    if timescale == 0 || duration == 0 || duration == u32::MAX as u64 {
        return None;
    }
    Some(duration as f64 / timescale as f64)
}

/// Lee la caja que empieza en `buf[0]`. `first` = es el inicio del archivo (valida que parezca MP4).
fn mp4_scan_step(buf: &[u8], first: bool) -> Mp4Step {
    let (Some(size32), Some(kind)) = (be_u32(buf, 0), buf.get(4..8)) else {
        return Mp4Step::Fail;
    };
    if first
        && !matches!(
            kind,
            b"ftyp" | b"moov" | b"free" | b"mdat" | b"wide" | b"skip"
        )
    {
        return Mp4Step::Fail;
    }
    let (size, header) = if size32 == 1 {
        match be_u64(buf, 8) {
            Some(s) => (s, 16usize),
            None => return Mp4Step::Fail,
        }
    } else {
        (size32 as u64, 8usize)
    };
    if size < header as u64 {
        return Mp4Step::Fail; // tamaño 0 ("hasta el final") o corrupto
    }
    if kind == b"moov" {
        let body = buf.get(header..).unwrap_or(&[]);
        if body.get(4..8) != Some(b"mvhd".as_slice()) {
            return Mp4Step::Fail;
        }
        return match parse_mvhd_secs(&body[8..]) {
            Some(d) => Mp4Step::Duration(d),
            None => Mp4Step::Fail,
        };
    }
    Mp4Step::Skip(size)
}

/// Duración en segundos de un MP4 remoto; `None` si no es MP4 o no se pudo leer.
pub async fn probe_mp4_duration(url: &str) -> Option<f64> {
    let client = crate::build_http_client(10).ok()?;
    let mut offset: u64 = 0;
    for _ in 0..16 {
        let resp = client
            .get(url)
            .header(
                "Range",
                format!("bytes={}-{}", offset, offset + MP4_FETCH - 1),
            )
            .send()
            .await
            .ok()?;
        if resp.status().as_u16() != 206 {
            return None;
        }
        let buf = timeout(Duration::from_secs(6), resp.bytes())
            .await
            .ok()?
            .ok()?;
        match mp4_scan_step(&buf, offset == 0) {
            Mp4Step::Duration(d) => return Some(d),
            Mp4Step::Skip(n) => offset = offset.checked_add(n)?,
            Mp4Step::Fail => return None,
        }
    }
    None
}

/// Duración del video: del encabezado MKV y, si no hay (MP4, etc.), de la caja mvhd.
async fn video_duration_secs(probe: &Option<MkvProbe>, stream_url: Option<&str>) -> Option<f64> {
    if let Some(d) = probe.as_ref().and_then(|p| p.duration_secs) {
        return Some(d);
    }
    let u = stream_url?;
    timeout(Duration::from_secs(7), probe_mp4_duration(u))
        .await
        .ok()
        .flatten()
}

/// Compatibilidad: solo las pistas de subtítulo del MKV
pub async fn probe_mkv_subtitle_tracks(url: &str) -> Option<Vec<MkvSubTrack>> {
    probe_mkv(url).await.map(|p| p.tracks)
}

/// ¿Alguna pista del MKV es en español?
pub fn mkv_has_spanish_sub(tracks: &[MkvSubTrack]) -> bool {
    tracks
        .iter()
        .any(|t| spanish_sub_priority(&t.lang, &t.name) > 0)
}

// ───────────────────────────── Descarga y guardado ─────────────────────────────

/// Opción de subtítulo ya descargada y analizada (se muestra en el selector)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubOption {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub format: String,
    pub url: String,
    /// Archivo local ya descargado (UTF-8)
    pub path: String,
    pub score: u8,
    /// Hecho para la copia exacta de tu video (coincidencia por hash)
    pub exact_file: bool,
    pub cues: u32,
    pub preview: Vec<String>,
    pub notes: Vec<String>,
}

fn opt_prefix(anilist_id: i32, episode: i32) -> String {
    format!("crunchy_subopt_{}_ep{}_", anilist_id, episode)
}

fn clear_option_files(dir: &Path, anilist_id: i32, episode: i32) {
    let prefix = opt_prefix(anilist_id, episode);
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

/// Caché de la selección AUTOMÁTICA (prefijo v2: ignora cachés viejos, posiblemente malos)
fn auto_cache_path(dir: &Path, anilist_id: i32, episode: i32, ext: &str) -> PathBuf {
    dir.join(format!(
        "crunchy_sub_v2_{}_ep{}.{}",
        anilist_id, episode, ext
    ))
}

fn find_auto_cache(dir: &Path, anilist_id: i32, episode: i32) -> Option<String> {
    for ext in ["srt", "ass", "vtt"] {
        let p = auto_cache_path(dir, anilist_id, episode, ext);
        if let Ok(meta) = fs::metadata(&p) {
            if meta.is_file() && meta.len() >= 40 {
                return Some(p.to_string_lossy().to_string());
            }
        }
    }
    None
}

/// Descarga el archivo y devuelve su contenido crudo + extensión probable.
/// Resuelve ZIP; rechaza HTML y .xz.
async fn fetch_subtitle_content(
    client: &Client,
    url: &str,
    episode: i32,
) -> Result<(Vec<u8>, &'static str), String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Error descargando subtítulo: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("El servidor respondió {}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Error leyendo bytes: {}", e))?;

    if bytes.len() < 40 {
        return Err("Archivo de subtítulo vacío o inválido.".to_string());
    }
    if bytes.starts_with(&XZ_MAGIC) {
        return Err("Adjunto comprimido en .xz (no soportado todavía).".to_string());
    }
    if looks_like_html(&bytes) {
        return Err("La respuesta es una página HTML, no un subtítulo.".to_string());
    }

    let low = url.to_lowercase();
    let url_hint: &'static str = if low.contains(".ass") {
        "ass"
    } else if low.contains(".vtt") {
        "vtt"
    } else {
        "srt"
    };

    if bytes.len() > 4 && &bytes[0..2] == b"PK" {
        let cursor = Cursor::new(bytes);
        let mut archive =
            zip::ZipArchive::new(cursor).map_err(|e| format!("Error abriendo ZIP: {}", e))?;

        // Nombres de los subtítulos del ZIP; si hay varios (pack), se elige el del episodio
        let mut names: Vec<String> = Vec::new();
        let mut idxs: Vec<usize> = Vec::new();
        for i in 0..archive.len() {
            if let Ok(f) = archive.by_index(i) {
                let n = f.name().unwrap_or_default().to_lowercase();
                if n.ends_with(".srt") || n.ends_with(".ass") || n.ends_with(".vtt") {
                    names.push(n);
                    idxs.push(i);
                }
            }
        }
        if let Some(pos) = pick_zip_entry(&names, episode) {
            let name = names[pos].clone();
            let mut file = archive
                .by_index(idxs[pos])
                .map_err(|e| format!("Error leyendo entrada ZIP: {}", e))?;
            let mut content = Vec::new();
            file.read_to_end(&mut content)
                .map_err(|e| format!("Error extrayendo archivo: {}", e))?;
            let hint: &'static str = if name.ends_with(".ass") {
                "ass"
            } else if name.ends_with(".vtt") {
                "vtt"
            } else {
                "srt"
            };
            return Ok((content, hint));
        }
        return Err("ZIP sin archivos de subtítulos válidos.".to_string());
    }

    Ok((bytes.to_vec(), url_hint))
}

/// Descarga, analiza y guarda un candidato. `None` si no sirve (HTML, vacío, sin diálogos…)
async fn build_option(
    client: Client,
    rc: RawCand,
    idx: usize,
    dir: PathBuf,
    anilist_id: i32,
    episode: i32,
    video_secs: Option<f64>,
) -> Option<SubOption> {
    let work = async {
        let (content, hint) = fetch_subtitle_content(&client, &rc.url, episode).await?;
        let text = decode_subtitle(content);
        let ext = sniff_ext(text.as_bytes(), hint);
        let cues = parse_cues(&text, ext);
        if cues.len() < 5 {
            return Err("sin diálogos reconocibles".to_string());
        }
        let stats = text_stats(&cues, &text);
        let judged = judge(&stats, &cues, video_secs, rc.exact, rc.prio);

        let path = dir.join(format!(
            "{}{}.{}",
            opt_prefix(anilist_id, episode),
            idx,
            ext
        ));
        fs::write(&path, text.as_bytes()).map_err(|e| format!("Error guardando: {}", e))?;

        Ok::<SubOption, String>(SubOption {
            id: format!("opt{}", idx),
            provider: rc.provider.to_string(),
            label: judged.label,
            format: ext.to_string(),
            url: rc.url.clone(),
            path: path.to_string_lossy().to_string(),
            score: judged.score,
            exact_file: rc.exact,
            cues: cues.len() as u32,
            preview: preview_lines(&cues),
            notes: judged.notes,
        })
    };

    match timeout(Duration::from_secs(7), work).await {
        Ok(Ok(opt)) => Some(opt),
        Ok(Err(e)) => {
            println!(
                "[Subtítulos] Descartado {} ({}): {}",
                rc.provider, rc.url, e
            );
            None
        }
        Err(_) => {
            println!(
                "[Subtítulos] Descartado {} ({}): tiempo agotado",
                rc.provider, rc.url
            );
            None
        }
    }
}

// ───────────────────────────── API pública ─────────────────────────────

/// Busca en los 3 proveedores, descarga los candidatos y los puntúa (mejor primero).
/// - `stream_url`: si se da, se usa para el hash del archivo y la duración del video.
/// - `thorough`: espera más a Animetosho (selector manual); en automático no se le espera.
pub async fn search_subtitle_options(
    anilist_id: i32,
    raw_titles: &[String],
    episode: i32,
    out_dir: &Path,
    stream_url: Option<&str>,
    thorough: bool,
) -> Result<Vec<SubOption>, String> {
    let client = crate::build_http_client(12)?;
    fs::create_dir_all(out_dir)
        .map_err(|e| format!("No se pudo crear la carpeta de subtítulos: {}", e))?;
    clear_option_files(out_dir, anilist_id, episode);

    // IDs externos, duración del video y hash del archivo, todo en paralelo
    let (ids, probe, hash) = tokio::join!(
        async {
            timeout(ARM_TIMEOUT, resolve_ids(&client, anilist_id))
                .await
                .unwrap_or_default()
        },
        async {
            match stream_url {
                Some(u) => timeout(Duration::from_secs(7), probe_mkv(u))
                    .await
                    .ok()
                    .flatten(),
                None => None,
            }
        },
        async {
            match stream_url {
                Some(u) => timeout(Duration::from_secs(7), compute_os_hash(u))
                    .await
                    .ok()
                    .flatten(),
                None => None,
            }
        },
    );

    let video_secs = video_duration_secs(&probe, stream_url).await;
    println!(
        "[Subtítulos] AniList {} -> IMDb {:?} | TVDB {:?} | duración video: {:?} s | hash: {:?}",
        anilist_id,
        ids.imdb,
        ids.tvdb,
        video_secs.map(|d| d.round()),
        hash.as_ref().map(|h| h.0.clone())
    );

    let filename = stream_url.and_then(filename_from_url);

    // Animetosho corre aparte: es el más lento y no siempre vale la espera
    let tosho_client = client.clone();
    let tosho_titles: Vec<String> = raw_titles.to_vec();
    let mut tosho_task =
        tokio::spawn(
            async move { collect_animetosho(&tosho_client, &tosho_titles, episode).await },
        );

    let (os_list, gest_list, subdl_list) = tokio::join!(
        async {
            match ids.imdb.as_deref() {
                Some(id) => {
                    with_timeout_vec(
                        "OpenSubtitles-v3",
                        Duration::from_secs(6),
                        collect_opensubtitles(
                            &client,
                            id,
                            episode,
                            hash.as_ref(),
                            filename.as_deref(),
                        ),
                    )
                    .await
                }
                None => Vec::new(),
            }
        },
        async {
            match ids.tvdb {
                Some(t) => {
                    with_timeout_vec(
                        "Gestdown/Addic7ed",
                        Duration::from_secs(5),
                        collect_gestdown(&client, t, episode),
                    )
                    .await
                }
                None => Vec::new(),
            }
        },
        async {
            match ids.imdb.as_deref() {
                Some(id) if SUBDL_KEY.get().is_some() => {
                    with_timeout_vec(
                        "SubDL",
                        Duration::from_secs(6),
                        collect_subdl(&client, id, episode),
                    )
                    .await
                }
                _ => Vec::new(),
            }
        },
    );

    let have = os_list.len() + gest_list.len() + subdl_list.len();
    let wait = if thorough {
        Duration::from_secs(6)
    } else if have > 0 {
        Duration::from_millis(1500)
    } else {
        Duration::from_secs(5)
    };
    let tosho_list = match timeout(wait, &mut tosho_task).await {
        Ok(Ok(v)) => {
            println!("[Subtítulos] Animetosho: {} candidato(s)", v.len());
            v
        }
        _ => {
            println!(
                "[Subtítulos] Animetosho: sin respuesta a tiempo ({}ms)",
                wait.as_millis()
            );
            tosho_task.abort();
            Vec::new()
        }
    };

    // Unimos, quitamos duplicados y priorizamos: hechos para tu archivo > Latino > resto
    let mut all: Vec<RawCand> = Vec::new();
    all.extend(os_list);
    all.extend(gest_list);
    all.extend(subdl_list);
    all.extend(tosho_list);
    let mut seen = HashSet::new();
    all.retain(|c| seen.insert(c.url.clone()));
    all.sort_by(|a, b| b.exact.cmp(&a.exact).then(b.prio.cmp(&a.prio)));
    all.truncate(MAX_OPTIONS);

    if all.is_empty() {
        return Ok(Vec::new());
    }

    let mut set = JoinSet::new();
    for (idx, rc) in all.into_iter().enumerate() {
        set.spawn(build_option(
            client.clone(),
            rc,
            idx,
            out_dir.to_path_buf(),
            anilist_id,
            episode,
            video_secs,
        ));
    }
    let mut options: Vec<SubOption> = Vec::new();
    while let Some(r) = set.join_next().await {
        if let Ok(Some(o)) = r {
            options.push(o);
        }
    }
    options.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(b.exact_file.cmp(&a.exact_file))
            .then(a.id.cmp(&b.id))
    });

    for o in &options {
        println!(
            "[Subtítulos] Opción {} | {} | {} | {} pts | exacto={} | {} líneas | avisos={:?}",
            o.id, o.provider, o.label, o.score, o.exact_file, o.cues, o.notes
        );
    }
    Ok(options)
}

/// Selección AUTOMÁTICA: usa la mejor opción puntuada (o la caché de este episodio).
pub async fn download_best_subtitle(
    anilist_id: i32,
    raw_titles: &[String],
    episode: i32,
    out_dir: &Path,
    stream_url: Option<&str>,
) -> Result<String, String> {
    fs::create_dir_all(out_dir)
        .map_err(|e| format!("No se pudo crear la carpeta de subtítulos: {}", e))?;
    if let Some(cached) = find_auto_cache(out_dir, anilist_id, episode) {
        println!("[Subtítulos] Reutilizando subtítulo en caché: {}", cached);
        return Ok(cached);
    }

    let options =
        search_subtitle_options(anilist_id, raw_titles, episode, out_dir, stream_url, false)
            .await?;

    match options.iter().find(|o| o.score >= MIN_AUTO_SCORE) {
        Some(best) => {
            let ext = Path::new(&best.path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("srt");
            let dest = auto_cache_path(out_dir, anilist_id, episode, ext);
            fs::copy(&best.path, &dest).map_err(|e| format!("Error guardando subtítulo: {}", e))?;
            println!(
                "[Subtítulos OK] Elegido {} de {} ({} pts): {}",
                best.label,
                best.provider,
                best.score,
                dest.display()
            );
            Ok(dest.to_string_lossy().to_string())
        }
        None => Err(format!(
            "No se encontró un subtítulo en español confiable ({} candidato(s) descartados por calidad).",
            options.len()
        )),
    }
}

/// Versión de escritorio (sin video conocido ni carpeta propia): guarda en la carpeta temporal
pub async fn download_spanish_subtitle(
    anilist_id: i32,
    raw_titles: &[String],
    episode: i32,
) -> Result<String, String> {
    let dir = env::temp_dir();
    download_best_subtitle(anilist_id, raw_titles, episode, &dir, None).await
}

/// Copia la opción elegida a un nombre estable (las opciones se borran en la próxima búsqueda)
pub fn persist_choice(
    src: &Path,
    dir: &Path,
    anilist_id: i32,
    episode: i32,
) -> Result<String, String> {
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("srt");
    let dest = dir.join(format!(
        "crunchy_subchoice_{}_ep{}.{}",
        anilist_id, episode, ext
    ));
    // Si había otra elección con otra extensión, la quitamos
    for e in ["srt", "ass", "vtt"] {
        let old = dir.join(format!(
            "crunchy_subchoice_{}_ep{}.{}",
            anilist_id, episode, e
        ));
        if old != dest {
            let _ = fs::remove_file(old);
        }
    }
    fs::copy(src, &dest).map_err(|e| format!("Error guardando la elección: {}", e))?;
    Ok(dest.to_string_lossy().to_string())
}

/// Vuelve a bajar una elección guardada cuyo archivo local ya no existe (caché limpiada)
pub async fn restore_subtitle(
    url: &str,
    dir: &Path,
    anilist_id: i32,
    episode: i32,
) -> Result<String, String> {
    if url.starts_with("ai://") {
        return Err(
            "Traducción de IA no disponible en caché: vuelve a generarla desde el selector."
                .to_string(),
        );
    }
    let client = crate::build_http_client(12)?;
    fs::create_dir_all(dir).map_err(|e| format!("No se pudo crear la carpeta: {}", e))?;
    let (content, hint) = fetch_subtitle_content(&client, url, episode).await?;
    let text = decode_subtitle(content);
    let ext = sniff_ext(text.as_bytes(), hint);
    let dest = dir.join(format!(
        "crunchy_subchoice_{}_ep{}.{}",
        anilist_id, episode, ext
    ));
    fs::write(&dest, text.as_bytes()).map_err(|e| format!("Error guardando: {}", e))?;
    Ok(dest.to_string_lossy().to_string())
}

// ───────────────────────────── IA para la TV ─────────────────────────────
// Flujo: subtítulo en inglés (OpenSubtitles por hash) -> Gemini -> .srt en español con los
// mismos tiempos. El resultado se guarda para siempre (por anime y episodio).

static GEMINI_KEY: OnceLock<String> = OnceLock::new();
static AI_GEN: AtomicU64 = AtomicU64::new(0);

/// Clave de Gemini (se llama al iniciar la app)
pub fn set_gemini_key(key: &str) {
    let k = key.trim();
    if !k.is_empty() {
        let _ = GEMINI_KEY.set(k.to_string());
    }
}

/// Cancela cualquier traducción en curso (se llama al empezar otra)
pub fn cancel_ai_translation() {
    AI_GEN.fetch_add(1, Ordering::SeqCst);
}

fn english_in_stremio(body: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(subs) = body["subtitles"].as_array() {
        for s in subs {
            let lang = s["lang"].as_str().unwrap_or("").to_lowercase();
            if lang == "eng" || lang == "en" || lang.starts_with("en-") {
                if let Some(u) = s["url"].as_str() {
                    out.push(u.to_string());
                }
            }
        }
    }
    out
}

/// (exacto, url): primero los hechos para tu archivo (hash), luego el resto
async fn collect_english_opensubtitles(
    client: &Client,
    imdb_id: &str,
    episode: i32,
    hash: Option<&(String, u64)>,
    filename: Option<&str>,
) -> Vec<(bool, String)> {
    let mut urls: Vec<String> = Vec::new();
    for season in 1..=6 {
        urls.push(format!(
            "{}/subtitles/series/{}:{}:{}.json",
            OS_BASE, imdb_id, season, episode
        ));
    }
    let plain_n = urls.len();
    if let Some((h, size)) = hash {
        let mut extra = format!("videoHash={}&videoSize={}", h, size);
        if let Some(f) = filename {
            extra.push_str(&format!("&filename={}", urlencoding::encode(f)));
        }
        for season in 1..=6 {
            urls.push(format!(
                "{}/subtitles/series/{}:{}:{}/{}.json",
                OS_BASE, imdb_id, season, episode, extra
            ));
        }
    }
    let results = fetch_json_many(client, urls).await;
    let mut plain: Vec<String> = Vec::new();
    let mut hashed: Vec<String> = Vec::new();
    for (i, body) in results {
        if let Some(b) = body {
            if i < plain_n {
                plain.extend(english_in_stremio(&b));
            } else {
                hashed.extend(english_in_stremio(&b));
            }
        }
    }
    let plain_set: HashSet<String> = plain.iter().cloned().collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for u in hashed {
        let exact = !plain_set.contains(&u);
        if seen.insert(u.clone()) {
            out.push((exact, u));
        }
    }
    for u in plain {
        if seen.insert(u.clone()) {
            out.push((false, u));
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.truncate(5);
    out
}

/// Pide a Gemini la traducción de un lote (reintenta con otros modelos si hay cuota/errores)
async fn gemini_translate_text(
    client: &Client,
    key: &str,
    prompt: &str,
    my_gen: u64,
) -> Result<String, String> {
    let url = format!(
        "https://{}.{}/v1beta/interactions",
        "generativelanguage", "googleapis.com"
    );
    let attempts: [(&str, u64); 5] = [
        ("gemini-3.5-flash-lite", 4),
        ("gemini-3.1-flash-lite", 6),
        ("gemini-3.6-flash", 10),
        ("gemini-3.8-flash", 12),
        ("gemma-4-31b-it", 15),
    ];
    let mut last_err = String::new();
    for (model, backoff) in attempts.iter() {
        if AI_GEN.load(Ordering::SeqCst) != my_gen {
            return Err("Traducción cancelada.".to_string());
        }
        let body = json!({
            "model": model,
            "input": prompt,
            "store": false,
            "generation_config": { "thinking_level": "low", "temperature": 0.2 }
        });
        let res = match client
            .post(&url)
            .header("x-goog-api-key", key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("Error de red: {}", e);
                tokio::time::sleep(Duration::from_secs(*backoff)).await;
                continue;
            }
        };
        let status = res.status().as_u16();
        let val: Value = match res.json().await {
            Ok(v) => v,
            Err(e) => {
                last_err = format!("Respuesta inválida: {}", e);
                tokio::time::sleep(Duration::from_secs(*backoff)).await;
                continue;
            }
        };
        // Mensaje real de Gemini (para saber por qué falla)
        let api_msg: String = val["error"]["message"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(200)
            .collect();
        if status == 429 || status >= 500 {
            last_err = format!("Gemini ocupado (HTTP {}) {}", status, api_msg);
            println!(
                "[IA TV] {} en espera (HTTP {}): {} | siguiente intento en {}s",
                model, status, api_msg, backoff
            );
            tokio::time::sleep(Duration::from_secs(*backoff)).await;
            continue;
        }
        if !(200..300).contains(&status) {
            println!("[IA TV] {} respondió HTTP {}: {}", model, status, api_msg);
            // Una clave inválida falla igual con cualquier modelo: no tiene sentido seguir
            let key_problem = status == 401
                || status == 403
                || api_msg.to_lowercase().contains("api key")
                || api_msg.to_lowercase().contains("permission");
            if key_problem {
                return Err(format!(
                    "Gemini rechazó la clave de API (HTTP {}): {}",
                    status, api_msg
                ));
            }
            last_err = format!(
                "Gemini respondió HTTP {} con {}: {}",
                status, model, api_msg
            );
            continue;
        }
        let mut text = String::new();
        if let Some(steps) = val["steps"].as_array() {
            for st in steps {
                if st["type"].as_str() == Some("model_output") {
                    if let Some(cs) = st["content"].as_array() {
                        for c in cs {
                            if let Some(t) = c["text"].as_str() {
                                text.push_str(t);
                            }
                        }
                    }
                }
            }
        }
        if !text.is_empty() {
            return Ok(text);
        }
        last_err = "Gemini devolvió una respuesta vacía".to_string();
    }
    Err(last_err)
}

/// Traduce un lote. Las líneas que la IA no devuelva se reintentan una vez.
async fn translate_batch(
    client: &Client,
    key: &str,
    anime: &str,
    episode: i32,
    cues: &[Cue],
    range: std::ops::Range<usize>,
    my_gen: u64,
) -> Result<Vec<Option<String>>, String> {
    let ctx_start = range.start.saturating_sub(3);
    let context: Vec<String> = cues[ctx_start..range.start]
        .iter()
        .map(|c| c.text.clone())
        .collect();
    let lines: Vec<(usize, String)> = range
        .clone()
        .enumerate()
        .map(|(i, idx)| (i, cues[idx].text.clone()))
        .collect();
    let n = lines.len();
    let prompt = build_translate_prompt(anime, episode, &lines, &context);
    let out = gemini_translate_text(client, key, &prompt, my_gen).await?;
    let mut parsed = parse_numbered_lines(&out, n);

    let missing: Vec<(usize, String)> = lines
        .iter()
        .filter(|(i, _)| parsed[*i].is_none())
        .cloned()
        .collect();
    if !missing.is_empty() && missing.len() < n {
        println!(
            "[IA TV] {} línea(s) sin traducir en el lote, reintentando",
            missing.len()
        );
        let p2 = build_translate_prompt(anime, episode, &missing, &context);
        if let Ok(o2) = gemini_translate_text(client, key, &p2, my_gen).await {
            let r2 = parse_numbered_lines(&o2, n);
            for (i, _) in &missing {
                if parsed[*i].is_none() {
                    parsed[*i] = r2[*i].clone();
                }
            }
        }
    }
    Ok(parsed)
}

const AI_BATCH: usize = 50;

/// Busca el subtítulo en inglés de ESTE archivo, lo traduce con Gemini y devuelve una opción
/// lista para aplicar. `perm_dir` guarda la traducción para siempre; `cache_dir` es donde
/// vive lo que el reproductor puede usar.
pub async fn ai_translate_subtitle(
    anilist_id: i32,
    anime_title: &str,
    episode: i32,
    cache_dir: &Path,
    perm_dir: &Path,
    stream_url: Option<&str>,
    progress: &(dyn Fn(&str, usize, usize) + Send + Sync),
) -> Result<SubOption, String> {
    let key = GEMINI_KEY
        .get()
        .cloned()
        .ok_or_else(|| "Falta la clave de Gemini (gemini_api_key.txt).".to_string())?;
    fs::create_dir_all(cache_dir).map_err(|e| format!("No se pudo crear la carpeta: {}", e))?;
    fs::create_dir_all(perm_dir).map_err(|e| format!("No se pudo crear la carpeta: {}", e))?;

    let perm = perm_dir.join(format!("crunchy_ai_{}_ep{}.srt", anilist_id, episode));
    let cached_copy = cache_dir.join(format!("{}ai.srt", opt_prefix(anilist_id, episode)));
    let make_option = |path: &Path, cues_es: &[Cue], exact: bool, note: String| SubOption {
        id: "ai".to_string(),
        provider: "IA (Gemini)".to_string(),
        label: "Español Latino (traducido con IA)".to_string(),
        format: "srt".to_string(),
        url: format!("ai://{}/{}", anilist_id, episode),
        path: path.to_string_lossy().to_string(),
        score: 85,
        exact_file: exact,
        cues: cues_es.len() as u32,
        preview: preview_lines(cues_es),
        notes: vec![note],
    };

    // Ya traducido antes: instantáneo
    if let Ok(text) = fs::read_to_string(&perm) {
        let cues = parse_cues(&text, "srt");
        if cues.len() >= 20 {
            fs::write(&cached_copy, text.as_bytes())
                .map_err(|e| format!("Error guardando: {}", e))?;
            println!(
                "[IA TV] Reutilizando traducción guardada ({} líneas)",
                cues.len()
            );
            return Ok(make_option(
                &cached_copy,
                &cues,
                true,
                "Traducción ya guardada".to_string(),
            ));
        }
    }

    let my_gen = AI_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let client = crate::build_http_client(12)?;

    progress("Buscando el subtítulo en inglés...", 0, 0);
    let (ids, probe, hash) = tokio::join!(
        async {
            timeout(ARM_TIMEOUT, resolve_ids(&client, anilist_id))
                .await
                .unwrap_or_default()
        },
        async {
            match stream_url {
                Some(u) => timeout(Duration::from_secs(7), probe_mkv(u))
                    .await
                    .ok()
                    .flatten(),
                None => None,
            }
        },
        async {
            match stream_url {
                Some(u) => timeout(Duration::from_secs(7), compute_os_hash(u))
                    .await
                    .ok()
                    .flatten(),
                None => None,
            }
        },
    );
    let imdb = ids.imdb.ok_or_else(|| {
        "No pude identificar la serie para buscar subtítulos en inglés.".to_string()
    })?;
    let video_secs = video_duration_secs(&probe, stream_url).await;
    let filename = stream_url.and_then(filename_from_url);
    println!(
        "[IA TV] imdb={} ep={} hash={} duración_video={:?} archivo={:?}",
        imdb,
        episode,
        hash.is_some(),
        video_secs.map(|d| d.round()),
        filename
    );

    let urls = timeout(
        Duration::from_secs(10),
        collect_english_opensubtitles(&client, &imdb, episode, hash.as_ref(), filename.as_deref()),
    )
    .await
    .unwrap_or_default();
    println!("[IA TV] Candidatos en inglés encontrados: {}", urls.len());
    if urls.is_empty() {
        return Err("OpenSubtitles no tiene subtítulos en inglés para este episodio (revisa el log: [IA TV]).".to_string());
    }

    // Descargamos y analizamos los candidatos
    let mut cands: Vec<(EnCand, Vec<Cue>)> = Vec::new();
    for (exact, url) in urls.iter().take(4) {
        let got = timeout(
            Duration::from_secs(8),
            fetch_subtitle_content(&client, url, episode),
        )
        .await;
        let Ok(Ok((content, hint))) = got else {
            println!(
                "[IA TV] No se pudo descargar un candidato en inglés: {}",
                url
            );
            continue;
        };
        let text = decode_subtitle(content);
        let ext = sniff_ext(text.as_bytes(), hint);
        let cues = parse_cues(&text, ext);
        if cues.len() < 20 {
            println!("[IA TV] Candidato descartado: solo {} líneas", cues.len());
            continue;
        }
        let st = text_stats(&cues, &text);
        let words = st.words.max(1) as f64;
        let english_ok = st.en as f64 / words > 0.10 && st.en >= st.es;
        println!(
            "[IA TV] Candidato: exacto={} líneas={} fin={}s inglés={}",
            exact,
            cues.len(),
            cues.iter().map(|c| c.end_ms).max().unwrap_or(0) / 1000,
            english_ok
        );
        cands.push((
            EnCand {
                exact: *exact,
                cues: cues.len(),
                last_end_ms: cues.iter().map(|c| c.end_ms).max().unwrap_or(0),
                english_ok,
            },
            cues,
        ));
    }
    let meta: Vec<EnCand> = cands.iter().map(|c| c.0.clone()).collect();
    let pick = pick_english_candidate(&meta, video_secs).ok_or_else(|| {
        "Los subtítulos en inglés encontrados no coinciden con tu video (otra versión).".to_string()
    })?;
    let (chosen, cues) = cands.swap_remove(pick);
    println!(
        "[IA TV] Inglés elegido: {} líneas, exacto={}, duración video={:?}",
        cues.len(),
        chosen.exact,
        video_secs.map(|d| d.round())
    );

    // Traducción por lotes, 2 a la vez
    let total = cues.len().div_ceil(AI_BATCH);
    let ranges: Vec<std::ops::Range<usize>> = (0..total)
        .map(|b| (b * AI_BATCH)..(((b + 1) * AI_BATCH).min(cues.len())))
        .collect();
    let ai_client = crate::build_http_client(90)?;
    let mut texts: Vec<Option<String>> = vec![None; cues.len()];
    progress("Traduciendo con IA...", 0, total);

    let mut b = 0usize;
    while b < total {
        if AI_GEN.load(Ordering::SeqCst) != my_gen {
            return Err("Traducción cancelada.".to_string());
        }
        let r1 = ranges[b].clone();
        let r2 = ranges.get(b + 1).cloned();
        let (a1, a2) = tokio::join!(
            translate_batch(
                &ai_client,
                &key,
                anime_title,
                episode,
                &cues,
                r1.clone(),
                my_gen
            ),
            async {
                match r2.clone() {
                    Some(r) => Some(
                        translate_batch(&ai_client, &key, anime_title, episode, &cues, r, my_gen)
                            .await,
                    ),
                    None => None,
                }
            },
        );
        for (rng, res) in [(Some(r1), Some(a1)), (r2, a2)] {
            if let (Some(rng), Some(res)) = (rng, res) {
                let parsed = res?;
                for (i, t) in parsed.into_iter().enumerate() {
                    texts[rng.start + i] = t;
                }
            }
        }
        b += 2;
        progress("Traduciendo con IA...", b.min(total), total);
    }
    if AI_GEN.load(Ordering::SeqCst) != my_gen {
        return Err("Traducción cancelada.".to_string());
    }

    let missing = texts.iter().filter(|t| t.is_none()).count();
    if missing * 100 > cues.len() * 25 {
        return Err(format!(
            "La IA devolvió la traducción incompleta ({} de {} líneas sin traducir). Intenta de nuevo.",
            missing,
            cues.len()
        ));
    }
    // Líneas que faltaron: se deja el original en inglés (mejor eso que un hueco)
    let final_texts: Vec<String> = texts
        .into_iter()
        .zip(cues.iter())
        .map(|(t, c)| t.unwrap_or_else(|| c.text.clone()))
        .collect();
    let srt = cues_to_srt(&cues, &final_texts);
    fs::write(&perm, srt.as_bytes())
        .map_err(|e| format!("Error guardando la traducción: {}", e))?;
    fs::write(&cached_copy, srt.as_bytes()).map_err(|e| format!("Error guardando: {}", e))?;

    let cues_es = parse_cues(&srt, "srt");
    println!(
        "[IA TV] Traducción lista: {} líneas ({} sin traducir)",
        cues_es.len(),
        missing
    );
    let note = format!(
        "Traducido del inglés{}",
        if chosen.exact {
            " (hecho para tu archivo)"
        } else {
            ""
        }
    );
    Ok(make_option(&cached_copy, &cues_es, chosen.exact, note))
}

#[cfg(test)]
mod mp4_tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn mvhd_v0(timescale: u32, duration: u32) -> Vec<u8> {
        let mut c = vec![0u8; 4]; // versión 0 + flags
        c.extend_from_slice(&[0; 8]); // creación + modificación
        c.extend_from_slice(&timescale.to_be_bytes());
        c.extend_from_slice(&duration.to_be_bytes());
        c.extend_from_slice(&[0; 80]);
        bx(b"mvhd", &c)
    }

    fn mvhd_v1(timescale: u32, duration: u64) -> Vec<u8> {
        let mut c = vec![1u8, 0, 0, 0];
        c.extend_from_slice(&[0; 16]);
        c.extend_from_slice(&timescale.to_be_bytes());
        c.extend_from_slice(&duration.to_be_bytes());
        c.extend_from_slice(&[0; 80]);
        bx(b"mvhd", &c)
    }

    /// Recorre el archivo como lo hace probe_mp4_duration, pero en memoria
    fn walk(file: &[u8]) -> Option<f64> {
        let mut off: usize = 0;
        for _ in 0..16 {
            if off >= file.len() {
                return None;
            }
            let end = (off + MP4_FETCH as usize).min(file.len());
            match mp4_scan_step(&file[off..end], off == 0) {
                Mp4Step::Duration(d) => return Some(d),
                Mp4Step::Skip(n) => off += n as usize,
                Mp4Step::Fail => return None,
            }
        }
        None
    }

    #[test]
    fn moov_first_faststart() {
        let mut f = bx(b"ftyp", b"isom0000");
        f.extend(bx(b"moov", &mvhd_v0(1000, 1_440_000)));
        f.extend(bx(b"mdat", &[0u8; 5000]));
        assert_eq!(walk(&f), Some(1440.0));
    }

    #[test]
    fn moov_after_mdat() {
        let mut f = bx(b"ftyp", b"isom0000");
        f.extend(bx(b"free", &[0u8; 100]));
        f.extend(bx(b"mdat", &vec![7u8; 20_000]));
        f.extend(bx(b"moov", &mvhd_v0(600, 864_000)));
        assert_eq!(walk(&f), Some(1440.0));
    }

    #[test]
    fn version_1_with_64bit_duration() {
        let mut f = bx(b"ftyp", b"isom0000");
        f.extend(bx(b"moov", &mvhd_v1(90_000, 129_600_000)));
        assert_eq!(walk(&f), Some(1440.0));
    }

    #[test]
    fn large_size_mdat_header_is_skipped() {
        let mut f = bx(b"ftyp", b"isom0000");
        let body = vec![1u8; 3000];
        let mut big = 1u32.to_be_bytes().to_vec();
        big.extend_from_slice(b"mdat");
        big.extend_from_slice(&((body.len() + 16) as u64).to_be_bytes());
        big.extend(body);
        f.extend(big);
        f.extend(bx(b"moov", &mvhd_v0(1000, 60_000)));
        assert_eq!(walk(&f), Some(60.0));
    }

    #[test]
    fn rejects_non_mp4_and_bad_values() {
        assert_eq!(walk(b"\x1A\x45\xDF\xA3 esto es un mkv....."), None);
        assert_eq!(walk(&[0u8; 64]), None);
        let mut f = bx(b"ftyp", b"isom0000");
        f.extend(bx(b"moov", &mvhd_v0(0, 1000)));
        assert_eq!(walk(&f), None);
        let mut g = bx(b"ftyp", b"isom0000");
        g.extend(bx(b"moov", &mvhd_v0(1000, 0)));
        assert_eq!(walk(&g), None);
    }

    #[test]
    fn moov_without_mvhd_first_fails() {
        let mut f = bx(b"ftyp", b"isom0000");
        f.extend(bx(b"moov", &bx(b"udta", &[0u8; 100])));
        assert_eq!(walk(&f), None);
    }
}
