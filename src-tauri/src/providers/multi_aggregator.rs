use crate::providers::nekobt::search_nekobt;
use crate::providers::nyaa::{
    TorrentCandidate, calculate_resolution_score, search_anime_generic, title_matches_episode,
    title_matches_season_context,
};
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Similitud mínima (Jaccard sobre bigramas) para aceptar un torrent como "el mismo anime".
const MIN_TITLE_SIMILARITY: f64 = 0.42;
/// Tope de candidatos que se conservan por proveedor secundario.
const MAX_PER_PROVIDER: usize = 50;
/// Timeout HTTP (segundos) de los proveedores secundarios.
const HTTP_TIMEOUT_SECS: u64 = 12;
/// Timeout (segundos) por petición de Animetosho y Knaben. Es corto a propósito: la búsqueda
/// espera al proveedor más lento y estos dos son opcionales.
const SECONDARY_TIMEOUT_SECS: u64 = 6;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoredCandidate {
    pub candidate: TorrentCandidate,
    pub source: String,
    pub total_pct: u8,
}

// ---------------------------------------------------------------------------
// Marcadores de idioma / grupos (una sola fuente de verdad)
// ---------------------------------------------------------------------------

const ES_MARKERS: &[&str] = &[
    "spa-la",
    "es-la",
    "es-419",
    "es-mx",
    "latino",
    "latin",
    "español",
    "espanol",
    "sub esp",
    "sub. esp",
    "sub.esp",
    "subesp",
    "castellano",
    "[spa]",
    "(spa)",
    "puyasubs",
];

const MULTISUB_MARKERS: &[&str] = &[
    "multisub",
    "multi-sub",
    "multi sub",
    "multiple subtitle",
    "[msub]",
    "(msub)",
    "[m-sub]",
    "(m-sub)",
];

const DUAL_MARKERS: &[&str] = &["dual audio", "dual-audio", "dual sub", "dual-sub"];

const ENG_SUB_MARKERS: &[&str] = &[
    "subsplease",
    "horriblesubs",
    "eng sub",
    "english sub",
    "varyg",
    "judas",
    "ember",
];

const CLEAN_GROUPS: &[&str] = &[
    "erai-raws",
    "subsplease",
    "puyasubs",
    "varyg",
    "crunchyroll",
    "[cr]",
    "netflix",
    "b-global",
    "horriblesubs",
];

fn contains_any(low: &str, markers: &[&str]) -> bool {
    markers.iter().any(|m| low.contains(m))
}

/// Puntaje de español inferido solo por el título (Animetosho / Knaben).
fn spanish_score_from_title(low: &str) -> u8 {
    if contains_any(low, ES_MARKERS) {
        100
    } else if contains_any(low, MULTISUB_MARKERS) {
        95
    } else if contains_any(low, DUAL_MARKERS) {
        75
    } else {
        40
    }
}

// ---------------------------------------------------------------------------
// Similitud de títulos (bigramas precalculados una sola vez por búsqueda)
// ---------------------------------------------------------------------------

fn compact_alnum(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn char_bigrams(s: &str) -> HashSet<(char, char)> {
    let chars: Vec<char> = s.chars().collect();
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Compara títulos de torrents contra los títulos pedidos.
/// Quita los tags entre corchetes [...] del torrent pero conserva los paréntesis (...).
struct TitleMatcher {
    requested: Vec<(String, HashSet<(char, char)>)>,
}

impl TitleMatcher {
    fn new(titles: &[String]) -> Self {
        let requested = titles
            .iter()
            .map(|t| compact_alnum(t))
            .filter(|c| c.len() >= 3)
            .map(|c| {
                let bg = char_bigrams(&c);
                (c, bg)
            })
            .collect();
        Self { requested }
    }

    fn score(&self, torrent_title: &str) -> f64 {
        let clean_torrent = torrent_title
            .split(|c| c == '[' || c == ']')
            .enumerate()
            .filter(|(idx, _)| idx % 2 == 0)
            .map(|(_, part)| part)
            .collect::<Vec<_>>()
            .join(" ");

        let t_compact = compact_alnum(&clean_torrent);
        if t_compact.is_empty() {
            return 0.0;
        }
        let t_bigrams = char_bigrams(&t_compact);

        let mut best: f64 = 0.0;
        for (r_compact, r_bigrams) in &self.requested {
            if t_compact.contains(r_compact.as_str()) {
                return 1.0;
            }
            let inter = t_bigrams.intersection(r_bigrams).count();
            let union = t_bigrams.len() + r_bigrams.len() - inter;
            if union > 0 {
                best = best.max(inter as f64 / union as f64);
            }
        }
        best
    }
}

// ---------------------------------------------------------------------------
// Precisión de título: ¿cuánto del título del torrent explican mis títulos?
// ---------------------------------------------------------------------------
//
// La similitud anterior solo mide cobertura ("¿mi título está dentro del suyo?"), por eso
// "Sword Art Online Alternative: Gun Gale Online II" pasaba como "Sword Art Online".
// Aquí se mide lo contrario, la precisión (métrica estándar de recuperación de información):
//
//   P = |C ∩ U| / |C|
//
//   C = palabras del TÍTULO del torrent: lo que va antes de la primera marca técnica
//       (episodio, temporada, año, calidad, códec, fuente, idioma) o de un paréntesis
//   U = palabras de TODOS los títulos conocidos del anime (romaji, inglés, sinónimos)
//
// Si el torrent trae palabras de título que no están en ninguno de mis títulos, es otra obra
// (derivada, película, spin-off). Se descarta con P < 0.55 y, si pasa, el puntaje se multiplica
// por F² con F = F_0.5 (precisión pesa el doble que cobertura): F = 1.25·P / (0.25·P + 1).

const MIN_TITLE_PRECISION: f64 = 0.55;

fn fold_char(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'ā' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'ē' => 'e',
        'í' | 'ì' | 'î' | 'ï' | 'ī' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ō' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'ū' => 'u',
        'ñ' => 'n',
        'ç' => 'c',
        _ => c,
    }
}

fn word_tokens(s: &str) -> Vec<String> {
    let folded: String = s
        .chars()
        .flat_map(|c| c.to_lowercase())
        .map(fold_char)
        .collect();
    folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// Quita los grupos entre paréntesis (idiomas, calidad, año...), incluso si no se cierran
fn strip_parens(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                if depth > 0 {
                    depth -= 1;
                } else {
                    out.push(' ');
                }
            }
            _ => {
                if depth == 0 {
                    out.push(c);
                }
            }
        }
    }
    out
}

/// 2nd, 3rd, 4th...
fn is_ordinal_word(t: &str) -> bool {
    let d = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    d >= 1 && matches!(&t[d..], "st" | "nd" | "rd" | "th")
}

/// Palabras que NO son parte del título: calidad, códecs, idioma, tipo de release, grupos
fn is_noise_word(t: &str) -> bool {
    if t.len() >= 3 && t.ends_with('p') && t[..t.len() - 1].chars().all(|c| c.is_ascii_digit()) {
        return true; // 1080p
    }
    if t.len() == 4 && t.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(y) = t.parse::<u32>() {
            if (1950..=2035).contains(&y) {
                return true; // año
            }
        }
    }
    if is_ordinal_word(t) {
        return true;
    }
    matches!(
        t,
        "x264"
            | "x265"
            | "h264"
            | "h265"
            | "264"
            | "265"
            | "hevc"
            | "avc"
            | "av1"
            | "aac"
            | "flac"
            | "opus"
            | "ac3"
            | "dts"
            | "ddp"
            | "10bit"
            | "8bit"
            | "hi10"
            | "hi10p"
            | "bd"
            | "bdrip"
            | "bluray"
            | "blu"
            | "ray"
            | "web"
            | "webrip"
            | "dl"
            | "dvd"
            | "dvdrip"
            | "tv"
            | "remux"
            | "raw"
            | "raws"
            | "dual"
            | "audio"
            | "multi"
            | "multiple"
            | "sub"
            | "subs"
            | "subtitle"
            | "subtitles"
            | "subbed"
            | "dubbed"
            | "dub"
            | "esp"
            | "spa"
            | "lat"
            | "latino"
            | "castellano"
            | "spanish"
            | "english"
            | "eng"
            | "jpn"
            | "jap"
            | "japanese"
            | "mkv"
            | "mp4"
            | "avi"
            | "batch"
            | "complete"
            | "completa"
            | "season"
            | "series"
            | "temporada"
            | "part"
            | "cour"
            | "uncut"
            | "uncensored"
            | "proper"
            | "repack"
            | "weekly"
            | "horriblesubs"
            | "subsplease"
            | "erai"
            | "judas"
            | "ember"
            | "animerg"
            | "reaktor"
            | "anime"
            | "time"
            | "hd"
            | "fhd"
            | "uhd"
            | "4k"
            | "full"
            | "hybrid"
            | "remastered"
            | "remaster"
            | "integral"
            | "integrale"
            | "edition"
            | "collection"
            | "vostfr"
            | "vose"
            | "vo"
            | "vos"
            | "french"
            | "fr"
            | "pt"
            | "br"
            | "ptbr"
            | "es"
            | "en"
            | "ca"
            | "de"
            | "it"
            | "ru"
            | "jp"
            | "ja"
            | "cat"
            | "catala"
            | "catalan"
            | "castella"
            | "frances"
            | "aleman"
            | "italiano"
            | "portugues"
            | "ingles"
            | "ruso"
            | "arabe"
            | "euskera"
            | "gallego"
            | "udf"
            | "iso"
            | "rip"
            | "hdtv"
            | "bdmv"
            | "v2"
            | "v3"
            | "v4"
    )
}

/// ¿Esta palabra marca dónde empieza el número de episodio / temporada? Lo que sigue ya no es título.
fn is_episode_marker(t: &str, known: &std::collections::HashSet<String>) -> bool {
    if matches!(
        t,
        "episode" | "episodio" | "capitulo" | "ep" | "eps" | "cap"
    ) {
        return true;
    }
    let b = t.as_bytes();
    // s1, s01 (temporada) y s01e05
    if b.len() >= 2 && b[0] == b's' && b[1].is_ascii_digit() {
        let rest = &t[1..];
        let d = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let tail = &rest[d..];
        if tail.is_empty()
            || (tail.starts_with('e')
                && tail.len() > 1
                && tail[1..].chars().all(|c| c.is_ascii_digit()))
        {
            return true;
        }
    }
    // e05 / ep05
    if let Some(rest) = t.strip_prefix("ep").or_else(|| t.strip_prefix('e')) {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    // 05 / 05v2 (sin ser un número del propio título)
    let digits_end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    if digits_end >= 1 && digits_end <= 4 && !known.contains(t) {
        let tail = &t[digits_end..];
        if tail.is_empty()
            || (tail.starts_with('v') && tail[1..].chars().all(|c| c.is_ascii_digit()))
        {
            return !(digits_end == 4 && is_noise_word(&t[..digits_end]));
        }
    }
    false
}

/// (precisión, palabras de más). `aliases` = todos los títulos conocidos del anime.
fn title_precision(torrent_title: &str, aliases: &[String]) -> (f64, Vec<String>) {
    let known: std::collections::HashSet<String> =
        aliases.iter().flat_map(|a| word_tokens(a)).collect();
    // sin etiquetas [..]
    let no_brackets: String = torrent_title
        .split(|c| c == '[' || c == ']')
        .enumerate()
        .filter(|(i, _)| i % 2 == 0)
        .map(|(_, p)| p)
        .collect::<Vec<_>>()
        .join(" ");
    // sin grupos (..): son idiomas, calidad, año o el grupo
    let no_parens = strip_parens(&no_brackets);
    let text = if word_tokens(&no_parens).is_empty() {
        no_brackets
    } else {
        no_parens
    };

    let mut cand: Vec<String> = Vec::new();
    for t in word_tokens(&text) {
        if is_episode_marker(&t, &known) {
            break;
        }
        if known.contains(&t) {
            if !cand.contains(&t) {
                cand.push(t);
            }
            continue;
        }
        let noise = is_noise_word(&t) || t.chars().count() == 1;
        if noise {
            if cand.is_empty() {
                continue; // grupo o etiqueta ANTES del título
            }
            break; // primera marca técnica después del título: ahí termina
        }
        if !cand.contains(&t) {
            cand.push(t);
        }
    }
    if cand.is_empty() {
        return (1.0, Vec::new());
    }
    let explained = cand.iter().filter(|t| known.contains(*t)).count();
    let extras: Vec<String> = cand
        .iter()
        .filter(|t| !known.contains(*t))
        .cloned()
        .collect();
    (explained as f64 / cand.len() as f64, extras)
}

/// F_0.5 con cobertura = 1: F = 1.25·P / (0.25·P + 1); el puntaje se multiplica por F²
fn precision_factor(p: f64) -> f64 {
    if p >= 0.85 {
        return 1.0;
    }
    let f = 1.25 * p / (0.25 * p + 1.0);
    f * f
}

// ---------------------------------------------------------------------------
// Puntaje estadístico (0 - 100)
// ---------------------------------------------------------------------------

/// Combina Idioma (45%), Grupo limpio (20%), Calidad (20%) y Salud de seeders (hasta 6 pts).
pub fn calculate_statistical_score(cand: &TorrentCandidate, source: &str) -> u8 {
    let low = cand.title.to_lowercase();

    // 1. Idioma / subtítulos (hasta 45 pts)
    let mut lang_pts: f64 = 18.0;
    if cand.spanish_score >= 100 || contains_any(&low, ES_MARKERS) {
        lang_pts = 45.0;
    } else if cand.spanish_score >= 90 || contains_any(&low, MULTISUB_MARKERS) {
        lang_pts = 43.0;
    } else if contains_any(&low, ENG_SUB_MARKERS) || contains_any(&low, DUAL_MARKERS) {
        lang_pts = 26.0;
    } else if low.contains("[raw]")
        || low.contains("(raw)")
        || low.contains(" raws ")
        || low.ends_with("raw")
    {
        lang_pts = 0.0;
    }

    // Penaliza películas sueltas y archivos enormes que no son batch
    if low.contains("(movie)") || low.contains(" movie ") || low.contains("movie 01") {
        lang_pts -= 30.0;
    }
    let is_batch = low.contains("batch")
        || low.contains("complete")
        || low.contains("season")
        || low.contains("s01-")
        || low.contains("01 ~")
        || low.contains("01-")
        || low.contains("1-");
    if !is_batch && cand.size_mb > 2300 {
        lang_pts -= 45.0;
    }

    // 2. Grupo limpio (hasta 20 pts)
    let group_pts: f64 = if contains_any(&low, CLEAN_GROUPS) {
        20.0
    } else if low.contains("judas")
        || low.contains("ember")
        || low.contains("anime time")
        || low.contains("animerg")
        || low.contains("reaktor")
    {
        14.0
    } else {
        7.0
    };

    // 3. Calidad visual y eficiencia de peso (hasta 20 pts)
    let quality_pts: f64 = match cand.resolution_score {
        3 if cand.size_mb > 0 && cand.size_mb <= 1900 => 20.0,
        3 => 16.0,
        2 => 14.0,
        _ => 8.0,
    };

    // 4. Salud de seeders para Debrid/TorBox (con >= 5 ya está sano)
    let health_pts: f64 = match cand.seeders {
        0 => 0.0,
        1..=2 => 3.0,
        3..=4 => 5.0,
        _ => 6.0,
    };

    let source_bonus: f64 = if source == "Animetosho" { 2.0 } else { 0.0 };

    (lang_pts + group_pts + quality_pts + health_pts + source_bonus)
        .round()
        .clamp(0.0, 100.0) as u8
}

// ---------------------------------------------------------------------------
// Utilidades de búsqueda
// ---------------------------------------------------------------------------

fn extract_btih_from_magnet(magnet: &str) -> String {
    let low = magnet.to_lowercase();
    if let Some((_, rest)) = low.split_once("xt=urn:btih:") {
        return rest.split('&').next().unwrap_or("").trim().to_string();
    }
    String::new()
}

/// Quita lo que va entre paréntesis ("Hunter x Hunter (2011)" -> "Hunter x Hunter"),
/// porque los buscadores de torrents suelen fallar con ellos.
fn strip_parentheses(t: &str) -> String {
    let mut out = String::new();
    let mut depth = 0u32;
    for c in t.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Amplía la lista de títulos con variantes útiles (versión compacta y versión corta).
fn expand_titles(titles: &[String]) -> Vec<String> {
    let mut expanded = titles.to_vec();
    let Some(first) = titles.first().cloned() else {
        return expanded;
    };

    let word_count = first.split_whitespace().count();
    let compact = first.replace(' ', "");
    if word_count <= 3
        && (4..=16).contains(&compact.len())
        && !expanded.iter().any(|t| t.eq_ignore_ascii_case(&compact))
    {
        expanded.insert(1.min(expanded.len()), compact);
    }

    let short_clean: String = first
        .split(':')
        .next()
        .unwrap_or(&first)
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join(" ");
    if short_clean.len() >= 5
        && !expanded
            .iter()
            .any(|t| t.eq_ignore_ascii_case(&short_clean))
    {
        expanded.push(short_clean);
    }
    expanded
}

/// Lista para consultar proveedores: si el título principal lleva paréntesis,
/// pone primero su versión limpia y conserva el original justo detrás.
fn build_query_titles(expanded: &[String]) -> Vec<String> {
    let mut out = expanded.to_vec();
    if let Some(first) = expanded.first() {
        let cleaned = strip_parentheses(first);
        if cleaned.len() >= 3 && !cleaned.eq_ignore_ascii_case(first) {
            out.insert(0, cleaned);
        }
    }
    out
}

/// Con episodio: "Título 05" y luego "Título". Sin episodio (packs): solo "Título".
fn episode_queries(title: &str, episode: i32) -> Vec<String> {
    if episode > 0 {
        vec![format!("{} {:02}", title, episode), title.to_string()]
    } else {
        vec![title.to_string()]
    }
}

/// Acepta solo respuestas 2xx; registra el motivo si no lo son.
fn accept_response(
    res: Result<Response, reqwest::Error>,
    tag: &str,
    query: &str,
) -> Option<Response> {
    match res {
        Ok(r) if r.status().is_success() => Some(r),
        Ok(r) => {
            println!("[{}] HTTP {} para '{}'", tag, r.status(), query);
            None
        }
        Err(e) => {
            // reqwest solo muestra "error sending request"; la causa real está en la cadena de fuentes
            use std::error::Error;
            let mut causes = String::new();
            let mut src = e.source();
            while let Some(s) = src {
                causes.push_str(&format!(" <- {}", s));
                src = s.source();
            }
            println!("[{}] error de red para '{}': {}{}", tag, query, e, causes);
            None
        }
    }
}

async fn read_json(resp: Response, tag: &str) -> Option<Value> {
    match resp.json::<Value>().await {
        Ok(v) => Some(v),
        Err(e) => {
            println!("[{}] JSON inválido: {}", tag, e);
            None
        }
    }
}

/// Construye un candidato con puntajes derivados del título.
fn build_candidate(
    title: String,
    magnet: String,
    info_hash: String,
    size_bytes: u64,
    seeders: u64,
) -> TorrentCandidate {
    let low = title.to_lowercase();
    TorrentCandidate {
        resolution_score: calculate_resolution_score(&title),
        spanish_score: spanish_score_from_title(&low),
        size_mb: size_bytes / (1024 * 1024),
        seeders: seeders as u32,
        title,
        magnet,
        info_hash,
    }
}

// ---------------------------------------------------------------------------
// Proveedores
// ---------------------------------------------------------------------------

/// Direcciones del feed JSON de Animetosho, en orden de prueba.
/// El sitio cerró en mayo y reabrió en animetosho.net; la primera es la dirección más probable
/// por analogía con la anterior (feed.animetosho.org) y NO está verificada: ajústala si hace falta.
const ANIMETOSHO_FEEDS: &[&str] = &["https://feed.animetosho.net/json"];

/// Animetosho JSON feed (espejo de Nyaa con metadatos de archivos reales).
async fn search_animetosho(
    client: &Client,
    title: &str,
    episode: i32,
    is_classic: bool,
) -> Vec<TorrentCandidate> {
    const TAG: &str = "Animetosho";
    let mut queries = episode_queries(title, episode);
    if is_classic && episode > 0 {
        queries.push(format!("{} batch", title));
    }

    'feeds: for base in ANIMETOSHO_FEEDS {
        for (i, q) in queries.iter().enumerate() {
            let url = format!("{}?q={}", base, urlencoding::encode(q));

            let Some(resp) = accept_response(
                client
                    .get(&url)
                    .timeout(Duration::from_secs(SECONDARY_TIMEOUT_SECS))
                    .send()
                    .await,
                TAG,
                q,
            ) else {
                // Si ni la primera consulta conecta, esta dirección no sirve: pasa a la siguiente
                if i == 0 {
                    continue 'feeds;
                }
                continue;
            };
            let Some(parsed) = read_json(resp, TAG).await else {
                continue;
            };
            let Some(arr) = parsed.as_array() else {
                println!(
                    "[{}] respuesta inesperada (no es una lista) para '{}'",
                    TAG, q
                );
                continue;
            };

            let mut out = Vec::new();
            for item in arr {
                if out.len() >= MAX_PER_PROVIDER {
                    break;
                }
                let t_title = item["title"].as_str().unwrap_or("").trim().to_string();
                let magnet = item["magnet_uri"].as_str().unwrap_or("").trim().to_string();
                let mut info_hash = item["info_hash"]
                    .as_str()
                    .unwrap_or("")
                    .trim()
                    .to_lowercase();
                if info_hash.is_empty() && !magnet.is_empty() {
                    info_hash = extract_btih_from_magnet(&magnet);
                }

                if t_title.is_empty() || info_hash.is_empty() || magnet.is_empty() {
                    continue;
                }
                if episode > 0 && !title_matches_episode(&t_title, episode) {
                    continue;
                }

                out.push(build_candidate(
                    t_title,
                    magnet,
                    info_hash,
                    item["total_size"].as_u64().unwrap_or(0),
                    item["seeders"].as_u64().unwrap_or(1),
                ));
            }

            if !out.is_empty() {
                println!(
                    "[{} OK] {} '{}' devolvió {} candidatos",
                    TAG,
                    base,
                    q,
                    out.len()
                );
                return out;
            }
        }
    }
    Vec::new()
}

/// Knaben API JSON (agrega 1337x, RuTracker y trackers globales sin Cloudflare).
async fn search_knaben(client: &Client, title: &str, episode: i32) -> Vec<TorrentCandidate> {
    const TAG: &str = "Knaben";
    const URL: &str = "https://api.knaben.org/v1";

    for q in episode_queries(title, episode) {
        let body = json!({
            "query": q,
            "size": MAX_PER_PROVIDER,
            "order_by": "seeders",
            "order_direction": "desc"
        });

        let Some(resp) = accept_response(
            client
                .post(URL)
                .json(&body)
                .timeout(Duration::from_secs(SECONDARY_TIMEOUT_SECS))
                .send()
                .await,
            TAG,
            &q,
        ) else {
            continue;
        };
        let Some(parsed) = read_json(resp, TAG).await else {
            continue;
        };
        let Some(hits) = parsed["hits"].as_array() else {
            println!("[{}] sin campo 'hits' para '{}'", TAG, q);
            continue;
        };

        let mut out = Vec::new();
        for hit in hits {
            let t_title = hit["title"].as_str().unwrap_or("").trim().to_string();
            let magnet = hit["magnetUrl"].as_str().unwrap_or("").trim().to_string();
            let mut info_hash = hit["hash"].as_str().unwrap_or("").trim().to_lowercase();
            if info_hash.is_empty() && !magnet.is_empty() {
                info_hash = extract_btih_from_magnet(&magnet);
            }

            if t_title.is_empty() || info_hash.is_empty() || magnet.is_empty() {
                continue;
            }
            if episode > 0 && !title_matches_episode(&t_title, episode) {
                continue;
            }

            out.push(build_candidate(
                t_title,
                magnet,
                info_hash,
                hit["bytes"].as_u64().unwrap_or(0),
                hit["seeders"].as_u64().unwrap_or(1),
            ));
        }

        if !out.is_empty() {
            println!("[{} OK] '{}' devolvió {} candidatos", TAG, q, out.len());
            return out;
        }
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// Orquestador
// ---------------------------------------------------------------------------

/// Consulta Nyaa + nekoBT + Animetosho + Knaben en paralelo y devuelve los candidatos
/// ordenados por su % estadístico (empate: el más liviano primero).
pub async fn search_multi_provider_scored(
    titles: &[String],
    episode: i32,
    year: Option<i32>,
) -> Result<Vec<ScoredCandidate>, String> {
    let expanded_titles = expand_titles(titles);
    let query_titles = build_query_titles(&expanded_titles);
    let is_classic = year.map(|y| y <= 2015).unwrap_or(false);

    let primary_query = query_titles.first().cloned().unwrap_or_default();
    if primary_query.trim().is_empty() {
        return Err("Título vacío para búsqueda multi-proveedor.".to_string());
    }

    let client = crate::build_http_client(HTTP_TIMEOUT_SECS)?;

    // Los 4 proveedores corren en paralelo, sin sumar latencia
    let (nyaa_res, nekobt_list, tosho_list, knaben_list) = tokio::join!(
        search_anime_generic(&expanded_titles, episode, year),
        search_nekobt(&client, &query_titles, episode),
        search_animetosho(&client, &primary_query, episode, is_classic),
        search_knaben(&client, &primary_query, episode)
    );

    let nyaa_list = match nyaa_res {
        Ok(v) => v,
        Err(e) => {
            println!("[Nyaa] error: {}", e);
            Vec::new()
        }
    };

    println!(
        "[Multi-Provider Raw] Nyaa: {} | nekoBT: {} | Animetosho: {} | Knaben: {}",
        nyaa_list.len(),
        nekobt_list.len(),
        tosho_list.len(),
        knaben_list.len()
    );

    // El orden define quién "gana" cuando el mismo hash aparece en varios proveedores
    let pool = nyaa_list
        .into_iter()
        .map(|c| (c, "Nyaa"))
        .chain(nekobt_list.into_iter().map(|c| (c, "nekoBT")))
        .chain(tosho_list.into_iter().map(|c| (c, "Animetosho")))
        .chain(knaben_list.into_iter().map(|c| (c, "Knaben/1337x")));

    let matcher = TitleMatcher::new(&expanded_titles);
    let mut seen_hashes: HashSet<String> = HashSet::new();
    let mut ranked: Vec<ScoredCandidate> = Vec::new();

    // Descartes por proveedor: [duplicados, temporada, similitud]
    let mut drops: HashMap<&'static str, [u32; 3]> = HashMap::new();
    let mut precision_logs = 0u32;

    for (cand, source) in pool {
        if !seen_hashes.insert(cand.info_hash.clone()) {
            drops.entry(source).or_default()[0] += 1;
            continue;
        }
        if !title_matches_season_context(&cand.title, &expanded_titles) {
            drops.entry(source).or_default()[1] += 1;
            continue;
        }
        let sim = matcher.score(&cand.title);
        if sim < MIN_TITLE_SIMILARITY {
            drops.entry(source).or_default()[2] += 1;
            continue;
        }
        // Coincidencia por "contiene mi título": hay que comprobar que no sea otra obra
        // (spin-off / derivada) cuyo nombre solo empieza igual.
        let mut precision_mult = 1.0;
        if sim >= 0.999 {
            let (prec, extras) = title_precision(&cand.title, &expanded_titles);
            if prec < MIN_TITLE_PRECISION {
                drops.entry(source).or_default()[2] += 1;
                if precision_logs < 6 {
                    precision_logs += 1;
                    println!(
                        "[Multi-Provider Precisión] Descartado '{}' (precisión {:.2}, palabras de más: {})",
                        cand.title,
                        prec,
                        extras.join(" ")
                    );
                }
                continue;
            }
            precision_mult = precision_factor(prec);
        }
        let mut pct = calculate_statistical_score(&cand, source);
        if sim >= 0.85 {
            pct = (pct + 4).min(100);
        }
        if precision_mult < 1.0 {
            pct = ((pct as f64) * precision_mult).round() as u8;
        }
        // Release de OTRO episodio (p. ej. "- 05" cuando se pide el 8): no puede ganar por estar en caché
        let ep_factor = episode_mismatch_factor(&cand.title, episode);
        if ep_factor < 1.0 {
            pct = ((pct as f64) * ep_factor).round() as u8;
        }
        ranked.push(ScoredCandidate {
            candidate: cand,
            source: source.to_string(),
            total_pct: pct,
        });
    }

    for src in ["Nyaa", "nekoBT", "Animetosho", "Knaben/1337x"] {
        if let Some([dup, season, sim]) = drops.get(src) {
            println!(
                "[Multi-Provider Descartes] {}: duplicados {} | temporada {} | similitud {}",
                src, dup, season, sim
            );
        }
    }

    let kept = |s: &str| ranked.iter().filter(|r| r.source == s).count();
    println!(
        "[Multi-Provider Final] Nyaa: {} | nekoBT: {} | Animetosho: {} | Knaben: {} (tras filtros)",
        kept("Nyaa"),
        kept("nekoBT"),
        kept("Animetosho"),
        kept("Knaben/1337x")
    );

    ranked.sort_by(|a, b| {
        b.total_pct
            .cmp(&a.total_pct)
            .then_with(|| a.candidate.size_mb.cmp(&b.candidate.size_mb))
    });

    if ranked.is_empty() {
        return Err(format!(
            "No se encontraron fuentes válidas para {} (Ep. {}).",
            primary_query, episode
        ));
    }

    Ok(ranked)
}

/// Mantiene compatibilidad con llamadas anteriores que solo pedían TorrentCandidate.
#[allow(dead_code)]
pub async fn search_multi_provider_ranked(
    titles: &[String],
    episode: i32,
) -> Result<Vec<TorrentCandidate>, String> {
    let scored = search_multi_provider_scored(titles, episode, None).await?;
    Ok(scored.into_iter().map(|r| r.candidate).collect())
}

// ── Episodio declarado en el título del release ─────────────────────────
/// Números de episodio que el título del release declara explícitamente.
/// Vacío si no hay ninguno claro o si parece un rango/pack (conservador: ante la duda, no penaliza).
fn title_episode_numbers(title: &str) -> Vec<i32> {
    let low = title.to_lowercase();
    for w in [
        "batch",
        "complete",
        "completa",
        "completo",
        "season pack",
        "temporada completa",
        "integral",
    ] {
        if low.contains(w) {
            return Vec::new();
        }
    }
    let tokens: Vec<&str> = low
        .split(|c: char| c.is_whitespace() || "[]()_{}".contains(c))
        .filter(|t| !t.is_empty())
        .collect();

    fn num_with_version(t: &str) -> Option<i32> {
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || digits.len() > 4 {
            return None;
        }
        let rest = &t[digits.len()..];
        if !(rest.is_empty()
            || (rest.starts_with('v')
                && rest[1..].chars().all(|c| c.is_ascii_digit())
                && rest.len() > 1))
        {
            return None;
        }
        digits.parse().ok()
    }
    fn is_plain_num(t: &str) -> bool {
        !t.is_empty() && t.len() <= 4 && t.chars().all(|c| c.is_ascii_digit())
    }
    fn is_range_token(t: &str) -> bool {
        for sep in ['-', '~'] {
            if let Some((a, b)) = t.split_once(sep) {
                if is_plain_num(a) && is_plain_num(b) {
                    return true;
                }
            }
        }
        false
    }

    let mut found: Vec<i32> = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if is_range_token(t) {
            return Vec::new();
        }
        let next = tokens.get(i + 1).copied();
        // s02e08
        if t.starts_with('s') {
            if let Some(epos) = t.find('e') {
                let (sp, ep) = (&t[1..epos], &t[epos + 1..]);
                if is_plain_num(sp) {
                    if let Some(n) = num_with_version(ep) {
                        found.push(n);
                        continue;
                    }
                }
            }
        }
        // e08 / ep08
        for pre in ["ep", "e"] {
            if let Some(rest) = t.strip_prefix(pre) {
                if let Some(n) = num_with_version(rest) {
                    found.push(n);
                }
            }
        }
        // "ep 8" / "episode 8"
        if matches!(*t, "ep" | "episode" | "episodio" | "e") {
            if let Some(n) = next.and_then(num_with_version) {
                found.push(n);
            }
        }
        // " - 08"
        if *t == "-" {
            if let Some(nx) = next {
                if let Some(n) = num_with_version(nx) {
                    // un número seguido de otro con guion es un rango, y un año no es episodio
                    let prev_is_num = i > 0 && is_plain_num(tokens[i - 1]);
                    if prev_is_num {
                        return Vec::new();
                    }
                    if !(1900..=2100).contains(&n) {
                        found.push(n);
                    }
                }
            }
        }
    }
    found.sort_unstable();
    found.dedup();
    found
}

/// Multiplicador de puntaje: <1.0 si el release es claramente OTRO episodio.
/// Solo castiga números menores al pedido o apenas mayores (+1/+2): una numeración absoluta
/// (episodio 178 para el 8 de la temporada) queda intacta.
fn episode_mismatch_factor(title: &str, target_ep: i32) -> f64 {
    if target_ep <= 0 {
        return 1.0;
    }
    let nums = title_episode_numbers(title);
    if nums.is_empty() || nums.contains(&target_ep) {
        return 1.0;
    }
    if nums.iter().all(|&n| n < target_ep || n - target_ep <= 2) {
        0.35
    } else {
        1.0
    }
}

#[cfg(test)]
mod precision_tests {
    use super::*;

    fn sao() -> Vec<String> {
        vec!["Sword Art Online".into(), "S.A.O".into(), "SAO".into()]
    }
    fn lain() -> Vec<String> {
        vec!["Serial Experiments Lain".into(), "Lain".into()]
    }
    fn p(t: &str, a: &[String]) -> f64 {
        title_precision(t, a).0
    }
    fn dropped(t: &str, a: &[String]) -> bool {
        p(t, a) < MIN_TITLE_PRECISION
    }

    #[test]
    fn spinoff_is_rejected() {
        for t in [
            "[Erai-raws] Sword Art Online Alternative - Gun Gale Online II - 02 [1080p][Multiple Subtitle]",
            "[Erai-raws] Sword Art Online Alternative: Gun Gale Online II - 03 [1080p]",
            "[amZero] Sword Art Online Alternative - Gun Gale Online II - 03 [1080p AV1 10Bit][AAC][MultiSubs] (Weekly)",
            "[SubsPlease] Sword Art Online Alternative - Gun Gale Online S2 - 05 (1080p) [ABCD1234].mkv",
        ] {
            assert!(
                dropped(t, &sao()),
                "debía descartarse: {} -> {}",
                t,
                p(t, &sao())
            );
        }
        let (_, extras) = title_precision(
            "[G] Sword Art Online Alternative - Gun Gale Online II - 02",
            &sao(),
        );
        assert!(extras.contains(&"gale".to_string()));
    }

    #[test]
    fn real_releases_pass() {
        for t in [
            "[HorribleSubs] Sword Art Online - 02 [1080p].mkv",
            "[WZF]Sword_Art_Online_-_01~25[HD][X264-AAC][1280x720][Sub_Esp][MP4]",
            "Sword.Art.Online.S01E02.1080p.BluRay.x265-GROUP",
            "[Judas] Sword Art Online - S01E05 [1080p][HEVC x265 10bit][Multi-Subs]",
            "Sword Art Online (2012) Complete 1080p BD Dual Audio",
            "Anime Time Sword Art Online 02 1080p",
        ] {
            assert!(p(t, &sao()) >= 0.99, "{} -> {}", t, p(t, &sao()));
        }
    }

    // Casos reales del log de Lain: eran releases legítimos y se descartaban por error
    #[test]
    fn real_lain_packs_from_the_log_pass() {
        for t in [
            "Serial Experiments Lain (Català, Castellà, Japonès - 1080p - v2)",
            "[Team246] Serial Experiments Lain (20e aniversary dybex - intégrale - bdrip 1080p french-jap-vostfr)",
            "(H.A.L)Serial Experiments Lain-DVD e 01-13 MKV pt br",
            "Serial.Experiments.Lain.S01.1998.Hybrid.FR-DVDRip.Multi.Audio.x264-Arya",
            "Serial Experiments Lain (DVD, Selecta Vision, UDF) - Català / Castellà / Japonès",
        ] {
            assert!(
                !dropped(t, &lain()),
                "NO debía descartarse: {} -> {}",
                t,
                p(t, &lain())
            );
        }
    }

    #[test]
    fn episode_title_after_number_is_ignored() {
        let a = vec!["Cowboy Bebop".to_string()];
        assert!(
            p(
                "[G] Cowboy Bebop - 05 - Ballad of Fallen Angels [1080p]",
                &a
            ) >= 0.99
        );
        assert!(p("Cowboy.Bebop.S01E05.Ballad.of.Fallen.Angels.1080p", &a) >= 0.99);
    }

    #[test]
    fn dual_title_releases_pass() {
        let a = vec![
            "Shingeki no Kyojin".to_string(),
            "Attack on Titan".to_string(),
        ];
        assert!(p("[G] Shingeki no Kyojin - Attack on Titan - 05 [1080p]", &a) >= 0.99);
        assert!(p("[G] Attack on Titan - 05 [1080p]", &a) >= 0.99);
    }

    #[test]
    fn numbers_in_title_are_kept() {
        let a = vec!["Mob Psycho 100".to_string()];
        assert!(p("[G] Mob Psycho 100 - 05 [1080p]", &a) >= 0.99);
        let b = vec!["86".to_string(), "Eighty Six".to_string()];
        assert!(p("[G] 86 - Eighty Six - 05 [1080p]", &b) >= 0.99);
    }

    #[test]
    fn accents_and_punctuation() {
        let a = vec!["Pokémon".to_string(), "Re:Zero".to_string()];
        assert!(p("[G] Pokemon - 05", &a) >= 0.99);
        assert!(p("[G] Re Zero - 05", &a) >= 0.99);
    }

    #[test]
    fn other_derivatives_are_penalized() {
        let pr = p(
            "[G] Sword Art Online Progressive - Aria of a Starless Night [1080p]",
            &sao(),
        );
        assert!(pr < MIN_TITLE_PRECISION, "{}", pr);
        let one = p("[G] Sword Art Online Progressive - 05 [1080p]", &sao());
        assert!(one < 0.85 && one >= MIN_TITLE_PRECISION, "{}", one);
    }

    #[test]
    fn factor_is_monotonic() {
        assert_eq!(precision_factor(1.0), 1.0);
        assert!(precision_factor(0.75) < 1.0);
        assert!(precision_factor(0.6) < precision_factor(0.75));
        assert!(precision_factor(0.55) < precision_factor(0.6));
    }
}

#[cfg(test)]
mod episode_tests {
    use super::*;
    const ERAI5: &str =
        "[Erai-raws] Black Clover 2nd Season - 05 [1080p CR WEBRip HEVC AAC][MultiSub][8128624C]";
    const ERAI8: &str =
        "[Erai-raws] Black Clover 2nd Season - 08 [1080p CR WEBRip HEVC AAC][MultiSub][5C7D7530]";
    #[test]
    fn wrong_episode_penalized() {
        assert_eq!(episode_mismatch_factor(ERAI5, 8), 0.35);
    }
    #[test]
    fn right_episode_ok() {
        assert_eq!(episode_mismatch_factor(ERAI8, 8), 1.0);
    }
    #[test]
    fn next_episode_penalized() {
        assert_eq!(
            episode_mismatch_factor("[SubsPlease] Show - 09 (1080p) [AB12CD34].mkv", 8),
            0.35
        );
    }
    #[test]
    fn batch_and_ranges_untouched() {
        assert_eq!(
            episode_mismatch_factor("[Group] Show (01-12) [1080p] Batch", 8),
            1.0
        );
        assert_eq!(
            episode_mismatch_factor("[Group] Show 01~24 [BD 1080p]", 8),
            1.0
        );
        assert_eq!(
            episode_mismatch_factor("[Group] Show Complete Series", 8),
            1.0
        );
    }
    #[test]
    fn absolute_numbering_untouched() {
        assert_eq!(
            episode_mismatch_factor("[SubsPlease] Black Clover - 178 (1080p)", 8),
            1.0
        );
        assert_eq!(
            episode_mismatch_factor("[ASW] One Piece - 1100 [1080p HEVC]", 8),
            1.0
        );
    }
    #[test]
    fn sxxexx_and_versions() {
        assert_eq!(episode_mismatch_factor("Show.S02E08.1080p.WEB", 8), 1.0);
        assert_eq!(episode_mismatch_factor("Show S02E05 1080p", 8), 0.35);
        assert_eq!(episode_mismatch_factor("[G] Show - 08v2 [1080p]", 8), 1.0);
        assert_eq!(episode_mismatch_factor("Show EP05 1080p", 8), 0.35);
    }
    #[test]
    fn titles_with_numbers_untouched() {
        assert_eq!(
            episode_mismatch_factor("[G] Mob Psycho 100 - 05 [1080p]", 8),
            1.0
        );
        assert_eq!(episode_mismatch_factor("[G] 86 - 08 [1080p]", 8), 1.0);
        assert_eq!(episode_mismatch_factor("[G] Show (2024) [1080p]", 8), 1.0);
        assert_eq!(episode_mismatch_factor("[G] Show 1080p WEB", 8), 1.0);
    }
    #[test]
    fn pack_search_ignored() {
        assert_eq!(episode_mismatch_factor(ERAI5, 0), 1.0);
    }
}
