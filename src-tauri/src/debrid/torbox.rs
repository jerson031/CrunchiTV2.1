use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::time::sleep;

/// Cuántos archivos con pinta de episodio tenía el último torrent resuelto.
/// Con 3 o más es un pack: así se recuerda aunque su título no lo diga.
static LAST_PACK_EPISODES: AtomicUsize = AtomicUsize::new(0);

pub fn last_pack_episode_files() -> usize {
    LAST_PACK_EPISODES.load(Ordering::SeqCst)
}

fn api_base() -> String {
    format!("https://{}.torbox.app/v1/api", "api")
}

fn build_client() -> Result<Client, String> {
    crate::build_http_client(20)
}

async fn parse_torbox_response(res: reqwest::Response, context: &str) -> Result<Value, String> {
    let status = res.status();

    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            return Err("Tu API Key de TorBox es inválida, caducó o tu suscripción venció. Revisa tu cuenta.".to_string());
        }
        StatusCode::TOO_MANY_REQUESTS => {
            return Err("TorBox recibió demasiadas peticiones seguidas. Espera unos segundos e intenta de nuevo.".to_string());
        }
        s if s.is_server_error() => {
            return Err(format!(
                "Los servidores de TorBox están caídos o en mantenimiento (Error {}).",
                s.as_u16()
            ));
        }
        _ => {}
    }

    let body: Value = res.json().await.map_err(|e| {
        format!(
            "Respuesta corrupta de TorBox en {} (posible caída temporal): {}",
            context, e
        )
    })?;

    if body["success"].as_bool() == Some(false) {
        let detail = body["detail"]
            .as_str()
            .unwrap_or("Error desconocido en TorBox");
        let error_code = body["error"].as_str().unwrap_or("");

        if error_code.contains("AUTH") || detail.to_lowercase().contains("token") {
            return Err("Tu token de TorBox ha caducado o fue revocado.".to_string());
        }
        return Err(format!(
            "TorBox rechazó la solicitud ({}): {}",
            context, detail
        ));
    }

    Ok(body)
}

pub async fn find_first_cached_hash(
    api_key: &str,
    hashes: &[String],
) -> Result<Option<String>, String> {
    if hashes.is_empty() {
        return Ok(None);
    }
    if api_key.trim().is_empty() || api_key == "TU_API_KEY_DE_TORBOX_AQUI" {
        return Err("Falta configurar tu API Key de TorBox en el código.".to_string());
    }

    let client = build_client()?;
    let hash_param = hashes.join(",");
    let url = format!(
        "{}/torrents/checkcached?hash={}&format=list",
        api_base(),
        hash_param
    );

    let res = client
        .get(&url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() || e.is_connect() {
                "No se pudo conectar con TorBox (servicio caído o sin internet).".to_string()
            } else {
                format!("Fallo de red consultando caché: {}", e)
            }
        })?;

    let body = parse_torbox_response(res, "checkcached").await?;

    if let Some(items) = body["data"].as_array() {
        for candidate_hash in hashes {
            let found = items.iter().any(|item| {
                item["hash"]
                    .as_str()
                    .map(|h| h.eq_ignore_ascii_case(candidate_hash))
                    .unwrap_or(false)
            });
            if found {
                return Ok(Some(candidate_hash.clone()));
            }
        }
    }

    Ok(None)
}

/// Estadística No Paramétrica: calcula Mediana (Q2) y Rango Intercuartílico (IQR = Q3 - Q1) de los tamaños
fn compute_quartile_stats(sizes: &[i64]) -> (f64, f64, f64, f64) {
    if sizes.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let mut sorted = sizes.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();

    let percentile = |p: f64| -> f64 {
        if n == 1 {
            return *sorted.first().unwrap_or(&0) as f64;
        }
        let idx = p * (n - 1) as f64;
        let lo = idx.floor() as usize;
        let hi = idx.ceil() as usize;
        let frac = idx - lo as f64;
        sorted[lo] as f64 * (1.0 - frac) + sorted[hi] as f64 * frac
    };

    let q1 = percentile(0.25);
    let q2 = percentile(0.50);
    let q3 = percentile(0.75);
    let iqr = (q3 - q1).max(1.0);
    (q1, q2, q3, iqr)
}
fn extract_file_season(low_path: &str) -> Option<i32> {
    for token in low_path.split(|c: char| !c.is_ascii_alphanumeric()) {
        if let Some(rest) = token.strip_prefix('s') {
            if let Some((s_part, e_part)) = rest.split_once('e') {
                if !s_part.is_empty()
                    && !e_part.is_empty()
                    && s_part.chars().all(|c| c.is_ascii_digit())
                    && e_part
                        .chars()
                        .next()
                        .map(|c| c.is_ascii_digit())
                        .unwrap_or(false)
                {
                    if let Ok(s_num) = s_part.parse::<i32>() {
                        return Some(s_num);
                    }
                }
            }
        }
    }
    for s in 1..=99 {
        if low_path.contains(&format!("season {}", s))
            || low_path.contains(&format!("season {:02}", s))
            || low_path.contains(&format!("/s{:02}/", s))
            || low_path.contains(&format!("\\s{:02}\\", s))
        {
            return Some(s);
        }
    }
    None
}
// ──────────────────────────────────────────────────────────────────────────
// Elección del archivo dentro de un pack (solo `std`, con pruebas unitarias)
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum EpKind {
    /// S01E05 / E05 / EP05 / "episode 5": inequívoco
    Strong,
    /// Número suelto ("Título - 05", "Título 05 [1080p]"): menos fiable
    Weak,
}

#[derive(Debug, Clone, Copy)]
struct EpGuess {
    num: i32,
    kind: EpKind,
}

/// Separa el nombre en tokens alfanuméricos. Antes quita lo que NO puede ser un episodio:
/// canales de audio ("2.0", "5.1"), que antes se leían como episodio 2 o 5.
/// Devuelve (token, ¿lo precede un guion?).
fn tokenize_name(name: &str) -> Vec<(String, bool)> {
    let chars: Vec<char> = name.chars().collect();
    let n = chars.len();
    let mut cleaned: Vec<char> = chars.clone();

    // 1) Decimales tipo "2.0" / "5.1" / "7.1": dos números de 1 dígito separados por un punto
    let mut i = 0;
    while i < n {
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < n && chars[i].is_ascii_digit() {
                i += 1;
            }
            let len = i - start;
            if len == 1 && i + 1 < n && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                let mut j = i + 1;
                while j < n && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j - (i + 1) == 1 {
                    for k in start..j {
                        cleaned[k] = ' ';
                    }
                    i = j;
                }
            }
        } else {
            i += 1;
        }
    }

    // 2) Tokens alfanuméricos y si les precede un guion
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut cur = String::new();
    let mut dash_before = false;
    let mut sep_has_dash = false;
    for c in cleaned {
        if c.is_ascii_alphanumeric() {
            if cur.is_empty() {
                dash_before = sep_has_dash;
            }
            cur.push(c.to_ascii_lowercase());
        } else {
            if !cur.is_empty() {
                out.push((std::mem::take(&mut cur), dash_before));
                sep_has_dash = false;
            }
            if c == '-' {
                sep_has_dash = true;
            }
        }
    }
    if !cur.is_empty() {
        out.push((cur, dash_before));
    }
    out
}

fn strip_version_suffix(t: &str) -> &str {
    // "05v2" -> "05"
    if let Some(pos) = t.rfind('v') {
        let (head, tail) = t.split_at(pos);
        if !head.is_empty()
            && head
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_alphabetic())
            && tail.len() > 1
            && tail[1..].chars().all(|c| c.is_ascii_digit())
            && head.chars().last().map_or(false, |c| c.is_ascii_digit())
        {
            return head;
        }
    }
    t
}

fn is_technical_token(t: &str) -> bool {
    if t.len() == 8 && t.chars().all(|c| c.is_ascii_hexdigit()) {
        return true; // CRC32
    }
    if t.len() >= 3 && t.ends_with('p') && t[..t.len() - 1].chars().all(|c| c.is_ascii_digit()) {
        return true; // 480p / 1080p / 2160p
    }
    matches!(
        t,
        "10bit"
            | "8bit"
            | "x264"
            | "x265"
            | "h264"
            | "h265"
            | "264"
            | "265"
            | "hevc"
            | "av1"
            | "aac"
            | "aac2"
            | "flac"
            | "ac3"
            | "dts"
            | "opus"
            | "ddp"
            | "4k"
            | "mkv"
            | "mp4"
            | "webm"
            | "bd"
            | "bdrip"
            | "web"
            | "webrip"
            | "dvd"
            | "dual"
    )
}

/// Adivina el número de episodio de un NOMBRE de archivo (sin carpetas).
fn guess_episode(file_name: &str) -> Option<EpGuess> {
    let stem = {
        let low = file_name.to_lowercase();
        let cut = low
            .strip_suffix(".mkv")
            .or_else(|| low.strip_suffix(".mp4"))
            .or_else(|| low.strip_suffix(".webm"))
            .unwrap_or(&low)
            .to_string();
        cut
    };
    let toks = tokenize_name(&stem);

    // Inequívoco: S01E05, E05, EP05, "episode 5"
    for (i, (t, _)) in toks.iter().enumerate() {
        let tv = strip_version_suffix(t);
        if let Some(rest) = tv.strip_prefix('s') {
            if let Some((sp, ep)) = rest.split_once('e') {
                if !sp.is_empty()
                    && sp.chars().all(|c| c.is_ascii_digit())
                    && !ep.is_empty()
                    && ep.chars().all(|c| c.is_ascii_digit())
                {
                    if let Ok(n) = ep.parse::<i32>() {
                        return Some(EpGuess {
                            num: n,
                            kind: EpKind::Strong,
                        });
                    }
                }
            }
        }
        let after = tv
            .strip_prefix("episode")
            .or_else(|| tv.strip_prefix("ep"))
            .or_else(|| {
                // "e05" pero no palabras como "end" o "extra"
                tv.strip_prefix('e')
                    .filter(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()))
            });
        if let Some(rest) = after {
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(n) = rest.parse::<i32>() {
                    return Some(EpGuess {
                        num: n,
                        kind: EpKind::Strong,
                    });
                }
            } else if rest.is_empty() {
                if let Some((nt, _)) = toks.get(i + 1) {
                    let nt = strip_version_suffix(nt);
                    if nt.chars().all(|c| c.is_ascii_digit()) && !nt.is_empty() {
                        if let Ok(n) = nt.parse::<i32>() {
                            return Some(EpGuess {
                                num: n,
                                kind: EpKind::Strong,
                            });
                        }
                    }
                }
            }
        }
    }

    // Débil: números sueltos. Se prefiere el primero que va tras un guion ("Título - 05").
    let mut weak: Vec<(i32, bool)> = Vec::new();
    for (t, dash) in &toks {
        if is_technical_token(t) {
            continue;
        }
        let tv = strip_version_suffix(t);
        if tv.is_empty() || tv.len() > 4 || !tv.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let n: i32 = match tv.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        if tv.len() == 4 && (1900..=2030).contains(&n) {
            continue; // año
        }
        weak.push((n, *dash));
    }
    if let Some((n, _)) = weak.iter().find(|(_, d)| *d) {
        return Some(EpGuess {
            num: *n,
            kind: EpKind::Weak,
        });
    }
    weak.last().map(|(n, _)| EpGuess {
        num: *n,
        kind: EpKind::Weak,
    })
}

/// ¿Es un extra (opening, ending, OVA, special, PV…) y no un episodio?
fn is_extra_file(full_path: &str, has_strong_ep: bool) -> bool {
    let low = full_path.to_lowercase().replace('\\', "/");
    let mut parts: Vec<&str> = low.split('/').collect();
    let file = parts.pop().unwrap_or("");

    // Carpetas de extras
    const FOLDER_WORDS: [&str; 14] = [
        "extras",
        "extra",
        "special",
        "specials",
        "ova",
        "ovas",
        "oad",
        "sp",
        "ncop",
        "nced",
        "oped",
        "creditless",
        "bonus",
        "featurettes",
    ];
    for seg in &parts {
        for t in seg.split(|c: char| !c.is_ascii_alphanumeric()) {
            if FOLDER_WORDS.contains(&t) || matches!(t, "pv" | "cm" | "trailers" | "omake") {
                return true;
            }
        }
    }

    // Temporada 0 = especiales
    if let Some(0) = extract_file_season(&low) {
        return true;
    }

    // Marcas inequívocas en el nombre
    if low.contains("clean opening")
        || low.contains("clean ending")
        || low.contains("non-credit")
        || low.contains("noncredit")
    {
        return true;
    }
    let toks: Vec<String> = file
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect();
    for t in &toks {
        let base = t.trim_end_matches(|c: char| c.is_ascii_digit());
        if matches!(base, "ncop" | "nced" | "oped" | "creditless") {
            return true;
        }
    }

    // Marcas que solo valen si el nombre NO trae S01E05 / E05 (un título de episodio podría
    // contener "ending" o llamarse "Ed")
    if has_strong_ep {
        return false;
    }
    for (i, t) in toks.iter().enumerate() {
        let base = t.trim_end_matches(|c: char| c.is_ascii_digit());
        let has_digits = base.len() != t.len();
        let next_is_num = toks.get(i + 1).map_or(false, |n| {
            !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())
        });
        match base {
            "ova" | "oad" | "special" | "specials" | "preview" | "trailer" | "menu" | "bonus"
            | "omake" | "opening" | "ending" => return true,
            "op" | "ed" | "sp" | "pv" | "cm" | "nc" if has_digits || next_is_num => return true,
            _ => {}
        }
    }
    false
}

#[derive(Debug)]
struct FileInfo {
    season: Option<i32>,
    ep: Option<EpGuess>,
    extra: bool,
}

fn analyze_file(full_path: &str) -> FileInfo {
    let norm = full_path.replace('\\', "/");
    let file_only = norm.rsplit('/').next().unwrap_or(&norm);
    let ep = guess_episode(file_only);
    let strong = matches!(
        ep,
        Some(EpGuess {
            kind: EpKind::Strong,
            ..
        })
    );
    FileInfo {
        season: extract_file_season(&norm.to_lowercase()),
        ep,
        extra: is_extra_file(&norm, strong),
    }
}

#[derive(Debug)]
pub struct Pick {
    pub index: Option<usize>,
    pub score: i32,
    pub note: String,
    /// Los mejores candidatos, para el log
    pub top: Vec<String>,
}

/// Elige qué archivo de un pack es el episodio `target_ep` de la temporada `target_season`.
/// `files` = (ruta completa, tamaño en bytes) de los videos.
pub fn pick_episode_file(files: &[(String, i64)], target_ep: i32, target_season: i32) -> Pick {
    if files.is_empty() {
        return Pick {
            index: None,
            score: i32::MIN,
            note: "sin archivos".into(),
            top: vec![],
        };
    }
    let infos: Vec<FileInfo> = files.iter().map(|(p, _)| analyze_file(p)).collect();

    // Si TODO parece "extra" (p. ej. un pack de OVAs), esa marca no sirve para distinguir
    let all_extra = infos.iter().all(|i| i.extra);
    let use_extra = !all_extra;

    // Estadística de tamaños solo con los archivos que sí parecen episodios
    let sizes: Vec<i64> = files
        .iter()
        .zip(&infos)
        .filter(|(_, i)| !use_extra || !i.extra)
        .map(|((_, s), _)| *s)
        .filter(|&s| s > 0)
        .collect();
    let (q1, q2, q3, iqr) = compute_quartile_stats(&sizes);

    let mut scored: Vec<(usize, i32, i64)> = Vec::new();
    for (idx, ((_, size), info)) in files.iter().zip(&infos).enumerate() {
        let mut score = 0i32;
        if let Some(s) = info.season {
            if s == target_season {
                score += 50;
            } else {
                score -= 80;
            }
        }
        if use_extra && info.extra {
            score -= 120;
        }
        match info.ep {
            Some(EpGuess { num, kind }) if num == target_ep => {
                score += if kind == EpKind::Strong { 100 } else { 85 };
            }
            Some(EpGuess { num, .. }) if num > 0 => score -= 95,
            _ => {}
        }
        if q2 > 0.0 && *size > 0 {
            let sz = *size as f64;
            let lower = (q1 - 1.5 * iqr).max(q2 * 0.45);
            let upper = (q3 + 1.5 * iqr).max(q2 * 1.85);
            if sz >= lower && sz <= upper {
                score += 20;
            } else if sz < q2 * 0.35 {
                score -= 40;
            }
        }
        scored.push((idx, score, *size));
    }
    scored.sort_by(|a, b| (b.1, b.2).cmp(&(a.1, a.2)));
    let top: Vec<String> = scored
        .iter()
        .take(3)
        .map(|(i, sc, _)| format!("{} pts: {}", sc, files[*i].0))
        .collect();

    let (best_idx, best_score, _) = scored[0];
    let best_matches = matches!(infos[best_idx].ep, Some(EpGuess { num, .. }) if num == target_ep);

    if best_score >= 0 && (best_matches || files.len() == 1 || infos[best_idx].ep.is_none()) {
        return Pick {
            index: Some(best_idx),
            score: best_score,
            note: "coincidencia directa".into(),
            top,
        };
    }

    // Respaldo: numeración absoluta (la temporada 2 de AniList empieza en 1, pero el pack
    // numera 25, 26, 27…). Se aplica solo si los números son correlativos.
    let mut nums: Vec<(i32, usize)> = infos
        .iter()
        .enumerate()
        .filter(|(_, i)| !(use_extra && i.extra))
        .filter(|(_, i)| i.season.map_or(true, |s| s == target_season || s == 1))
        .filter_map(|(idx, i)| i.ep.map(|e| (e.num, idx)))
        .collect();
    nums.sort();
    let mut uniq: Vec<i32> = nums.iter().map(|n| n.0).collect();
    uniq.dedup();
    if uniq.len() >= 3 {
        let min = uniq[0];
        let max = *uniq.last().unwrap();
        let contiguous = (max - min + 1) as usize <= uniq.len() + 2;
        if min > 1 && contiguous && (target_ep as usize) <= uniq.len() {
            let wanted = min + target_ep - 1;
            if let Some((_, idx)) = nums
                .iter()
                .filter(|(n, _)| *n == wanted)
                .max_by_key(|(_, idx)| files[*idx].1)
            {
                return Pick {
                    index: Some(*idx),
                    score: 0,
                    note: format!("numeración absoluta (el pack empieza en {})", min),
                    top,
                };
            }
        }
    }

    Pick {
        index: None,
        score: best_score,
        note: "ningún archivo coincide".into(),
        top,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(names: &[&str]) -> Vec<(String, i64)> {
        names.iter().map(|n| (n.to_string(), 300_000_000)).collect()
    }
    fn pick(names: &[&str], ep: i32, season: i32) -> Option<String> {
        let f = v(names);
        pick_episode_file(&f, ep, season)
            .index
            .map(|i| f[i].0.clone())
    }

    #[test]
    fn opus_audio_is_not_an_episode() {
        // Caso real: "Opus 2.0" hacía que S01E35 empatara con S01E02
        let mut f = v(&[
            "Naruto (2002) - S01E02 - Konoha [DVD-480p][Opus 2.0][AV1]-BlackRabbit.mkv",
            "Naruto (2002) - S01E35 - The Scrolls Secret [DVD-480p][Opus 2.0][AV1]-BlackRabbit.mkv",
        ]);
        f[1].1 = 400_000_000; // el 35 es más grande: antes ganaba el desempate
        let p = pick_episode_file(&f, 2, 1);
        assert_eq!(p.index, Some(0), "{:?}", p);
    }

    #[test]
    fn audio_channels_do_not_match_episode_one() {
        assert_eq!(
            pick(
                &[
                    "[G] Show - 08 [DDP5.1][x265].mkv",
                    "[G] Show - 01 [DDP5.1][x265].mkv"
                ],
                1,
                1
            ),
            Some("[G] Show - 01 [DDP5.1][x265].mkv".to_string())
        );
        assert_eq!(
            pick(
                &[
                    "[G] Show - 08 [DDP5.1][x265].mkv",
                    "[G] Show - 02 [DDP5.1][x265].mkv"
                ],
                8,
                1
            ),
            Some("[G] Show - 08 [DDP5.1][x265].mkv".to_string())
        );
    }

    #[test]
    fn openings_and_ovas_are_ignored() {
        let names = [
            "Show/Show - OP 1.mkv",
            "Show/Show - ED 1.mkv",
            "Show/Show - OVA 1.mkv",
            "Show/Show - NCOP1.mkv",
            "Show/Show - 01.mkv",
            "Show/Show - 02.mkv",
        ];
        assert_eq!(pick(&names, 1, 1), Some("Show/Show - 01.mkv".to_string()));
        assert_eq!(pick(&names, 2, 1), Some("Show/Show - 02.mkv".to_string()));
    }

    #[test]
    fn extras_folder_is_ignored() {
        let names = [
            "Show/Extras/01.mkv",
            "Show/Episodes/01.mkv",
            "Show/Episodes/02.mkv",
        ];
        assert_eq!(pick(&names, 1, 1), Some("Show/Episodes/01.mkv".to_string()));
    }

    #[test]
    fn title_numbers_and_years_are_not_episodes() {
        let names = [
            "Mob Psycho 100 - 05 [1080p].mkv",
            "Mob Psycho 100 - 06 [1080p].mkv",
        ];
        assert_eq!(pick(&names, 5, 1), Some(names[0].to_string()));
        let y = [
            "Naruto (2002) - 02 [480p].mkv",
            "Naruto (2002) - 03 [480p].mkv",
        ];
        assert_eq!(pick(&y, 3, 1), Some(y[1].to_string()));
    }

    #[test]
    fn dotted_release_names() {
        let names = ["Show.S01E05.1080p.WEB.mkv", "Show.S01E06.1080p.WEB.mkv"];
        assert_eq!(pick(&names, 5, 1), Some(names[0].to_string()));
    }

    #[test]
    fn season_is_respected() {
        let names = ["Show S01E08.mkv", "Show S02E08.mkv"];
        assert_eq!(pick(&names, 8, 2), Some("Show S02E08.mkv".to_string()));
        assert_eq!(pick(&names, 8, 1), Some("Show S01E08.mkv".to_string()));
    }

    #[test]
    fn absolute_numbering_fallback() {
        let names: Vec<String> = (25..=36)
            .map(|n| format!("[G] Show - {} [1080p].mkv", n))
            .collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            pick(&refs, 8, 1),
            Some("[G] Show - 32 [1080p].mkv".to_string())
        );
    }

    #[test]
    fn pack_made_only_of_ovas_still_works() {
        let names = ["Anime OVA 01.mkv", "Anime OVA 02.mkv", "Anime OVA 03.mkv"];
        assert_eq!(pick(&names, 2, 1), Some("Anime OVA 02.mkv".to_string()));
    }

    #[test]
    fn episode_title_with_ed_is_not_an_extra() {
        let names = [
            "Cowboy Bebop - S01E05 - Ed Meets Radical.mkv",
            "Cowboy Bebop - S01E06 - X.mkv",
        ];
        assert_eq!(pick(&names, 5, 1), Some(names[0].to_string()));
    }

    #[test]
    fn nothing_matches_is_rejected() {
        let names = ["Show - 01.mkv", "Show - 02.mkv"];
        assert_eq!(pick(&names, 9, 1), None);
    }

    #[test]
    fn single_file_without_number_is_accepted() {
        assert!(pick(&["Some Movie.mkv"], 1, 1).is_some());
    }
}

pub async fn resolve_direct_link(
    api_key: &str,
    magnet: &str,
    episode: i32,
    target_season: i32,
) -> Result<String, String> {
    LAST_PACK_EPISODES.store(0, Ordering::SeqCst);
    let client = build_client()?;
    let base = api_base();

    let create_url = format!("{}/torrents/createtorrent", base);
    let create_res = client
        .post(&create_url)
        .bearer_auth(api_key)
        .form(&[("magnet", magnet), ("seed", "1"), ("allow_zip", "false")])
        .send()
        .await
        .map_err(|_| "Tiempo de espera agotado al enviar el episodio a TorBox.".to_string())?;

    let create_body = parse_torbox_response(create_res, "createtorrent").await?;

    let torrent_id = create_body["data"]["torrent_id"].as_i64().ok_or_else(|| {
        "TorBox no devolvió un identificador válido para el episodio.".to_string()
    })?;

    let list_url = format!(
        "{}/torrents/mylist?id={}&bypass_cache=true",
        base, torrent_id
    );
    let mut main_file_id: Option<i64> = None;

    for intento in 1..=10 {
        let list_res = client
            .get(&list_url)
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(|_| {
                "Se perdió la conexión con TorBox mientras se preparaba el video.".to_string()
            })?;

        let list_body = parse_torbox_response(list_res, "mylist").await?;
        let files_opt = list_body["data"]["files"]
            .as_array()
            .or_else(|| list_body["data"][0]["files"].as_array());

        if let Some(files) = files_opt {
            let video_files: Vec<&Value> = files
                .iter()
                .filter(|f| {
                    let name = f["short_name"]
                        .as_str()
                        .or_else(|| f["name"].as_str())
                        .unwrap_or("")
                        .to_lowercase();
                    name.ends_with(".mkv") || name.ends_with(".mp4") || name.ends_with(".webm")
                })
                .collect();

            if !video_files.is_empty() {
                // Elegimos el archivo con reglas explícitas (ver pick_episode_file)
                let files_info: Vec<(String, i64)> = video_files
                    .iter()
                    .map(|f| {
                        let name = f["name"]
                            .as_str()
                            .or_else(|| f["short_name"].as_str())
                            .unwrap_or("")
                            .to_string();
                        (name, f["size"].as_i64().unwrap_or(0))
                    })
                    .collect();
                let episode_like = files_info
                    .iter()
                    .filter(|(p, _)| {
                        let i = analyze_file(p);
                        !i.extra && i.ep.is_some()
                    })
                    .count();
                LAST_PACK_EPISODES.store(episode_like, Ordering::SeqCst);
                let pick = pick_episode_file(&files_info, episode, target_season);
                for line in &pick.top {
                    println!("[TorBox] Candidato Ep. {}: {}", episode, line);
                }

                match pick.index {
                    None => {
                        return Err(format!(
                            "Ningún archivo del torrent corresponde al episodio {} de la temporada {} ({}).",
                            episode, target_season, pick.note
                        ));
                    }
                    Some(i) => {
                        let file = video_files[i];
                        if let Some(id) = file["id"].as_i64() {
                            let chosen_name = file["short_name"]
                                .as_str()
                                .or_else(|| file["name"].as_str())
                                .unwrap_or("video");
                            println!(
                                "[TorBox] Archivo seleccionado (Ep. {}): {} [{}]",
                                episode, chosen_name, pick.note
                            );
                            main_file_id = Some(id);
                            break;
                        }
                    }
                }
            }
        }

        if intento < 10 {
            let wait_ms = if intento <= 3 { 400 } else { 1000 };
            sleep(Duration::from_millis(wait_ms)).await;
        }
    }

    let file_id = main_file_id.ok_or_else(|| {
        "El torrent se añadió a TorBox pero no contiene archivos de video reproducibles o aún se está descargando.".to_string()
    })?;

    let dl_url = format!(
        "{}/torrents/requestdl?token={}&torrent_id={}&file_id={}&zip_link=false",
        base, api_key, torrent_id, file_id
    );

    let dl_res =
        client.get(&dl_url).send().await.map_err(|_| {
            "Error de red al generar el enlace directo de reproducción.".to_string()
        })?;

    let dl_body = parse_torbox_response(dl_res, "requestdl").await?;

    dl_body["data"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "TorBox no entregó una URL de streaming válida.".to_string())
}
