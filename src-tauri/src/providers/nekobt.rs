use crate::providers::nyaa::{
    TorrentCandidate, calculate_resolution_score, calculate_spanish_score, title_matches_episode,
    title_matches_season_context,
};
use reqwest::Client;
use serde_json::Value;
use std::collections::HashSet;

fn extract_btih(raw: &str) -> String {
    let low = raw.trim().to_lowercase();
    if let Some((_, rest)) = low.split_once("xt=urn:btih:") {
        return rest.split('&').next().unwrap_or("").trim().to_string();
    }
    if (low.len() == 40 || low.len() == 64) && low.chars().all(|c| c.is_ascii_hexdigit()) {
        return low;
    }
    String::new()
}

fn parse_json_u64(val: &Value) -> Option<u64> {
    val.as_u64()
        .or_else(|| val.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
}

/// Consulta la API JSON oficial de nekoBT (https://nekobt.to)
pub async fn search_nekobt(
    client: &Client,
    titles: &[String],
    episode: i32,
) -> Vec<TorrentCandidate> {
    let primary = match titles.first() {
        Some(t) if !t.trim().is_empty() => t.trim(),
        _ => return Vec::new(),
    };

    let api_key = std::env::var("NEKOBT_API_KEY").unwrap_or_else(|_| {
        option_env!("NEKOBT_API_KEY")
            .unwrap_or_default()
            .to_string()
    });

    let queries = if episode > 0 {
        vec![
            format!("{} {:02}", primary, episode),
            format!("{} E{:02}", primary, episode),
            primary.to_string(),
        ]
    } else {
        vec![primary.to_string()]
    };

    let mut out = Vec::new();
    let mut seen_hashes = HashSet::new();

    for q in queries {
        // Ruta completa corregida: /api/v1/torrents/search
        let url = format!(
            "https://nekobt.to/api/v1/torrents/search?query={}&limit=50&sort_by=seeders",
            urlencoding::encode(&q)
        );

        let mut req = client.get(&url).header("Accept", "application/json");
        if !api_key.trim().is_empty() {
            req = req.header("Cookie", format!("ssid={}", api_key.trim()));
        }

        let resp = match req.send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) if r.status().as_u16() == 429 => {
                println!("[nekoBT Rate Limit] Demasiadas peticiones (429).");
                break;
            }
            Ok(r) => {
                println!("[nekoBT Error] HTTP Status: {}", r.status());
                continue;
            }
            Err(e) => {
                println!("[nekoBT Request Error] {}", e);
                continue;
            }
        };

        let parsed: Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };

        if parsed["error"].as_bool() == Some(true) {
            let msg = parsed["message"].as_str().unwrap_or("Error desconocido");
            println!("[nekoBT API Error] {}", msg);
            continue;
        }

        let items = match parsed["data"]["results"].as_array() {
            Some(arr) => arr,
            None => continue,
        };

        for item in items {
            let t_title = item["title"].as_str().unwrap_or("").trim().to_string();
            let raw_magnet = item["magnet"].as_str().unwrap_or("").trim().to_string();
            let raw_hash = item["infohash"].as_str().unwrap_or("");

            let mut info_hash = extract_btih(raw_hash);
            if info_hash.is_empty() && !raw_magnet.is_empty() {
                info_hash = extract_btih(&raw_magnet);
            }

            if t_title.is_empty() || info_hash.is_empty() {
                continue;
            }
            if episode > 0 && !title_matches_episode(&t_title, episode) {
                continue;
            }
            if !title_matches_season_context(&t_title, titles) {
                continue;
            }
            if !seen_hashes.insert(info_hash.clone()) {
                continue;
            }

            let magnet = if raw_magnet.is_empty() {
                format!(
                    "magnet:?xt=urn:btih:{}&dn={}",
                    info_hash,
                    urlencoding::encode(&t_title)
                )
            } else {
                raw_magnet
            };

            let size_bytes = parse_json_u64(&item["filesize"]).unwrap_or(0);
            let size_mb = if size_bytes > 0 {
                size_bytes / (1024 * 1024)
            } else {
                0
            };

            let seeders = parse_json_u64(&item["seeders"]).unwrap_or(1) as u32;
            let resolution_score = calculate_resolution_score(&t_title);

            let langs = format!(
                "{},{},{}",
                item["audio_lang"].as_str().unwrap_or(""),
                item["sub_lang"].as_str().unwrap_or(""),
                item["fsub_lang"].as_str().unwrap_or("")
            )
            .to_lowercase();

            let has_native_es = langs
                .split(',')
                .any(|l| matches!(l.trim(), "es-419" | "es-es" | "es"));

            let spanish_score = if has_native_es {
                100
            } else {
                calculate_spanish_score(&t_title, "1_0")
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
            println!("[nekoBT OK] '{}' devolvió {} candidatos", q, out.len());
            break;
        }
    }

    out
}
