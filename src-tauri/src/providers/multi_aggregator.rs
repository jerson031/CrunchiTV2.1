use crate::providers::nyaa::{
    TorrentCandidate, search_anime_generic, title_matches_season_context,
};
use reqwest::Client;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ScoredCandidate {
    pub candidate: TorrentCandidate,
    pub source: &'static str,
    pub total_pct: u8,
}

fn compact_alnum(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn char_bigrams(s: &str) -> HashSet<(char, char)> {
    let chars: Vec<char> = s.chars().collect();
    let mut set = HashSet::new();
    for w in chars.windows(2) {
        set.insert((w[0], w[1]));
    }
    set
}

/// Coeficiente de Jaccard sobre bigramas conservando títulos entre paréntesis (...)
fn title_similarity_score(torrent_title: &str, requested_titles: &[String]) -> f64 {
    // Solo quitamos corchetes [...] (tags de grupo/códec), pero CONSERVAMOS paréntesis (...)
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

    let mut best_sim: f64 = 0.0;
    for req in requested_titles {
        let r_compact = compact_alnum(req);
        if r_compact.len() < 3 {
            continue;
        }
        if t_compact.contains(&r_compact) {
            return 1.0;
        }
        let r_bigrams = char_bigrams(&r_compact);
        let inter = t_bigrams.intersection(&r_bigrams).count() as f64;
        let union = t_bigrams.union(&r_bigrams).count() as f64;
        if union > 0.0 {
            let j = inter / union;
            if j > best_sim {
                best_sim = j;
            }
        }
    }
    best_sim
}

/// Calcula el índice estadístico (0% - 100%) combinando Idioma (45%), Grupo limpio (20%), Calidad (20%) y Salud Logarítmica (15%)
pub fn calculate_statistical_score(cand: &TorrentCandidate, source: &'static str) -> u8 {
    let low = cand.title.to_lowercase();

    // 1. Dimensión Idioma / Subtítulos (0 a 45 pts)
    let mut lang_pts: f64 = 18.0;
    if cand.spanish_score >= 100
        || low.contains("spa-la")
        || low.contains("es-la")
        || low.contains("es-419")
        || low.contains("es-mx")
        || low.contains("latino")
        || low.contains("latin")
        || low.contains("español")
        || low.contains("espanol")
        || low.contains("sub esp")
        || low.contains("sub. esp")
        || low.contains("sub.esp")
        || low.contains("subesp")
        || low.contains("castellano")
        || low.contains("[spa]")
        || low.contains("(spa)")
        || low.contains("puyasubs")
    {
        lang_pts = 45.0;
    } else if cand.spanish_score >= 90
        || low.contains("multisub")
        || low.contains("multi-sub")
        || low.contains("multi sub")
        || low.contains("multiple subtitle")
        || low.contains("[msub]")
        || low.contains("(msub)")
        || low.contains("[m-sub]")
        || low.contains("(m-sub)")
    {
        lang_pts = 43.0;
    } else if low.contains("subsplease")
        || low.contains("horriblesubs")
        || low.contains("eng sub")
        || low.contains("english sub")
        || low.contains("varyg")
        || low.contains("judas")
        || low.contains("ember")
        || low.contains("dual audio")
        || low.contains("dual-audio")
        || low.contains("dual sub")
        || low.contains("dual-sub")
    {
        lang_pts = 26.0;
    } else if low.contains("[raw]")
        || low.contains("(raw)")
        || low.contains(" raws ")
        || low.ends_with("raw")
    {
        lang_pts = 0.0;
    }

    // Penalización si el torrent es una película suelta y no el episodio de la serie
    if low.contains("(movie)") || low.contains(" movie ") || low.contains("movie 01") {
        lang_pts -= 30.0;
    }
    let is_batch = low.contains("batch")
        || low.contains("complete")
        || low.contains("season")
        || low.contains("01 ~")
        || low.contains("01-")
        || low.contains("1-");
    if !is_batch && cand.size_mb > 2300 {
        lang_pts -= 45.0;
    }
    // 2. Dimensión Grupo Limpio / Sin Karaoke destructivo (0 a 20 pts)
    let clean_groups = [
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
    let group_pts: f64 = if clean_groups.iter().any(|g| low.contains(g)) {
        20.0
    } else if low.contains("judas") || low.contains("ember") || low.contains("anime time") {
        13.0
    } else {
        7.0
    };

    // 3. Dimensión Calidad Visual y Eficiencia de Peso (0 a 20 pts)
    let quality_pts: f64 = match cand.resolution_score {
        3 => {
            if cand.size_mb > 0 && cand.size_mb <= 1900 {
                20.0
            } else {
                16.0
            }
        }
        2 => 14.0,
        _ => 8.0,
    };

    // 4. Dimensión Salud de Seeders para Debrid/TorBox (0 a 6 pts: con >= 5 seeders ya está sano)
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
/// Consulta Animetosho JSON API (espejo de Nyaa con metadatos de archivos reales)
fn extract_btih_from_magnet(magnet: &str) -> String {
    let low = magnet.to_lowercase();
    if let Some((_, rest)) = low.split_once("xt=urn:btih:") {
        return rest.split('&').next().unwrap_or("").trim().to_string();
    }
    String::new()
}

/// Consulta Animetosho JSON API (con fallback a pack completo si episode query da 0)
async fn search_animetosho(client: &Client, title: &str, episode: i32) -> Vec<TorrentCandidate> {
    let queries = [format!("{} {:02}", title, episode), title.to_string()];

    for q in queries {
        let url = format!(
            "https://{}.animetosho.org/json?q={}",
            "feed",
            urlencoding::encode(&q)
        );

        let resp = match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };

        let arr: Vec<Value> = match resp.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };

        let mut out = Vec::new();
        for item in arr.into_iter().take(30) {
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

            let size_bytes = item["total_size"].as_u64().unwrap_or(0);
            let size_mb = size_bytes / (1024 * 1024);
            let seeders = item["seeders"].as_u64().unwrap_or(1) as u32;

            let low = t_title.to_lowercase();
            let resolution_score = if low.contains("1080p") {
                3
            } else if low.contains("720p") {
                2
            } else {
                1
            };

            let spanish_score = if low.contains("spa-la")
                || low.contains("es-la")
                || low.contains("es-419")
                || low.contains("es-mx")
                || low.contains("latino")
                || low.contains("latin")
                || low.contains("español")
                || low.contains("espanol")
                || low.contains("sub esp")
                || low.contains("sub. esp")
                || low.contains("sub.esp")
                || low.contains("subesp")
                || low.contains("castellano")
                || low.contains("[spa]")
                || low.contains("(spa)")
                || low.contains("puyasubs")
            {
                100
            } else if low.contains("multisub")
                || low.contains("multi-sub")
                || low.contains("multi sub")
                || low.contains("multiple subtitle")
                || low.contains("[msub]")
                || low.contains("(msub)")
                || low.contains("[m-sub]")
                || low.contains("(m-sub)")
            {
                95
            } else if low.contains("dual audio")
                || low.contains("dual-audio")
                || low.contains("dual sub")
                || low.contains("dual-sub")
            {
                75
            } else {
                40
            };

            out.push(TorrentCandidate {
                title: t_title,
                magnet,
                info_hash,
                size_mb,
                seeders,
                resolution_score,
                spanish_score,
            });
        }

        if !out.is_empty() {
            return out;
        }
    }
    Vec::new()
}

/// Consulta Knaben API JSON (agrega 1337x, RuTracker y trackers globales sin Cloudflare)
async fn search_knaben(client: &Client, title: &str, episode: i32) -> Vec<TorrentCandidate> {
    let queries = [format!("{} {:02}", title, episode), title.to_string()];
    let url = format!("https://{}.knaben.org/v1", "api");

    for q in queries {
        let body = json!({
            "query": q,
            "size": 30,
            "order_by": "seeders",
            "order_direction": "desc"
        });

        let resp = match client.post(&url).json(&body).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };

        let parsed: Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };

        let hits = match parsed["hits"].as_array() {
            Some(h) => h,
            None => continue,
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

            let size_bytes = hit["bytes"].as_u64().unwrap_or(0);
            let size_mb = size_bytes / (1024 * 1024);
            let seeders = hit["seeders"].as_u64().unwrap_or(1) as u32;

            let low = t_title.to_lowercase();
            let resolution_score = if low.contains("1080p") {
                3
            } else if low.contains("720p") {
                2
            } else {
                1
            };

            let spanish_score = if low.contains("spa-la")
                || low.contains("es-la")
                || low.contains("es-419")
                || low.contains("es-mx")
                || low.contains("latino")
                || low.contains("latin")
                || low.contains("español")
                || low.contains("espanol")
                || low.contains("sub esp")
                || low.contains("sub. esp")
                || low.contains("sub.esp")
                || low.contains("subesp")
                || low.contains("castellano")
                || low.contains("[spa]")
                || low.contains("(spa)")
                || low.contains("puyasubs")
            {
                100
            } else if low.contains("multisub")
                || low.contains("multi-sub")
                || low.contains("multi sub")
                || low.contains("multiple subtitle")
                || low.contains("[msub]")
                || low.contains("(msub)")
                || low.contains("[m-sub]")
                || low.contains("(m-sub)")
            {
                95
            } else if low.contains("dual audio")
                || low.contains("dual-audio")
                || low.contains("dual sub")
                || low.contains("dual-sub")
            {
                75
            } else {
                40
            };

            out.push(TorrentCandidate {
                title: t_title,
                magnet,
                info_hash,
                size_mb,
                seeders,
                resolution_score,
                spanish_score,
            });
        }

        if !out.is_empty() {
            return out;
        }
    }
    Vec::new()
}
/// Orquestador paralelo estilo Torrentio: consulta Nyaa + Animetosho + Knaben (1337x) y ordena por % matemático
pub async fn search_multi_provider_ranked(
    titles: &[String],
    episode: i32,
) -> Result<Vec<TorrentCandidate>, String> {
    let mut expanded_titles = titles.to_vec();
    if let Some(first) = titles.first().cloned() {
        // 1. Variante compacta solo para títulos cortos (ej. "Dan Da Dan" -> "Dandadan")
        let word_count = first.split_whitespace().count();
        let compact = first.replace(' ', "");
        if word_count <= 3
            && (4..=16).contains(&compact.len())
            && !expanded_titles
                .iter()
                .any(|t| t.eq_ignore_ascii_case(&compact))
        {
            expanded_titles.insert(1.min(expanded_titles.len()), compact);
        }

        // 2. Variante limpia corta sin subtítulos largos ("2nd 3rd STAGE", ": Part X", etc.)
        let short_clean: String = first
            .split(':')
            .next()
            .unwrap_or(&first)
            .split_whitespace()
            .take(4)
            .collect::<Vec<_>>()
            .join(" ");
        if short_clean.len() >= 5
            && !expanded_titles
                .iter()
                .any(|t| t.eq_ignore_ascii_case(&short_clean))
        {
            expanded_titles.push(short_clean);
        }
    }

    let primary_title = expanded_titles.first().cloned().unwrap_or_default();
    if primary_title.is_empty() {
        return Err("Título vacío para búsqueda multi-proveedor.".to_string());
    }

    let client = crate::build_http_client(15)?;


    // Ejecutamos los 3 proveedores en paralelo sin sumar latencia
    let (nyaa_res, tosho_list, knaben_list) = tokio::join!(
        search_anime_generic(&expanded_titles, episode),
        search_animetosho(&client, &primary_title, episode),
        search_knaben(&client, &primary_title, episode)
    );

    println!(
        "[Multi-Provider Raw] Nyaa: {} | Animetosho: {} | Knaben/1337x: {}",
        nyaa_res.as_ref().map(|v| v.len()).unwrap_or(0),
        tosho_list.len(),
        knaben_list.len()
    );

    let mut seen_hashes = HashSet::new();
    let mut ranked: Vec<ScoredCandidate> = Vec::new();

    let mut evaluate_and_push = |cand: TorrentCandidate, source: &'static str| {
        if !seen_hashes.insert(cand.info_hash.clone()) {
            return;
        }
        if !title_matches_season_context(&cand.title, &expanded_titles) {
            return;
        }
        let sim = title_similarity_score(&cand.title, &expanded_titles);
        if sim < 0.42 {
            return;
        }
        let mut pct = calculate_statistical_score(&cand, source);
        if sim >= 0.85 {
            pct = (pct + 4).min(100);
        }
        ranked.push(ScoredCandidate {
            candidate: cand,
            source,
            total_pct: pct,
        });
    };

    if let Ok(nyaa_items) = nyaa_res {
        for cand in nyaa_items {
            evaluate_and_push(cand, "Nyaa");
        }
    }
    for cand in tosho_list {
        evaluate_and_push(cand, "Animetosho");
    }
    for cand in knaben_list {
        evaluate_and_push(cand, "Knaben/1337x");
    }

    // Ordenamos de mayor a menor porcentaje matemático; en empate, preferimos el más ligero
    ranked.sort_by(|a, b| {
        b.total_pct
            .cmp(&a.total_pct)
            .then_with(|| a.candidate.size_mb.cmp(&b.candidate.size_mb))
    });
    println!(
        "[Multi-Provider Ranking] Total candidatos únicos válidos: {}",
        ranked.len()
    );
    for item in ranked.iter().take(6) {
        println!(
            "  -> {}% [{}] ({} seeders | {} MB) {}",
            item.total_pct,
            item.source,
            item.candidate.seeders,
            item.candidate.size_mb,
            item.candidate.title
        );
    }

    if ranked.is_empty() {
        return Err(format!(
            "No se encontraron fuentes válidas en Nyaa, Animetosho ni Knaben/1337x para {} (Ep. {}).",
            primary_title, episode
        ));
    }

    Ok(ranked.into_iter().map(|r| r.candidate).collect())
}
