use reqwest::Client;
use serde_json::Value;
use std::env;
use std::fs;
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::time::Duration;

/// Convierte un AniList ID al IMDb ID oficial mediante la API abierta ARM
async fn resolve_imdb_id(client: &Client, anilist_id: i32) -> Option<String> {
    if anilist_id <= 0 {
        return None;
    }
    let url = format!("https://haglund.dev{}", anilist_id);
    let res = client.get(&url).send().await.ok()?;
    let body = res.json::<Value>().await.ok()?;
    body["imdb"].as_str().map(|s| s.to_string())
}

/// Busca subtítulos en español usando el mapeo exacto AniList -> IMDb ID y fuentes abiertas de anime
pub async fn download_spanish_subtitle(
    anilist_id: i32,
    raw_titles: &[String],
    episode: i32,
) -> Result<String, String> {
    let client = crate::build_http_client(15)?;


    // 1. Mapeo exacto por IMDb ID (Resuelve franquicias con múltiples temporadas/partes)
    if let Some(imdb_id) = resolve_imdb_id(&client, anilist_id).await {
        println!("[Subtítulos] AniList ID {} mapeado a IMDb: {}", anilist_id, imdb_id);

        // Probamos temporadas 1 a 8 (porque en IMDb las sagas largas como JoJo se agrupan por temporada global)
        for season in 1..=8 {
            let os_url = format!(
                "https://strem.io{}:{}:{}.json",
                imdb_id, season, episode
            );

            if let Ok(res) = client.get(&os_url).send().await {
                if let Ok(body) = res.json::<Value>().await {
                    if let Some(subs) = body["subtitles"].as_array() {
                        let spanish_sub = subs.iter().find(|s| {
                            let lang = s["lang"].as_str().unwrap_or("").to_lowercase();
                            lang == "spa" || lang == "es" || lang == "es-419" || lang == "es-mx"
                        });

                        if let Some(sub) = spanish_sub {
                            if let Some(sub_url) = sub["url"].as_str() {
                                println!("[Subtítulos] Encontrado en OpenSubtitles (IMDb {} S{:02}E{:02})", imdb_id, season, episode);
                                if let Ok(saved) = fetch_and_save_srt(&client, sub_url, episode).await {
                                    return Ok(saved);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Respaldo por texto en Animetosho (pistas extraídas de lanzamientos Multi-Sub)
    let ep_str = if episode >= 100 {
        format!("{}", episode)
    } else {
        format!("{:02}", episode)
    };

    for raw in raw_titles {
        let clean = raw
            .chars()
            .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        if clean.len() < 3 {
            continue;
        }

        let query = format!("{} {}", clean, ep_str);
        let tosho_url = format!("https://animetosho.org{}", urlencoding::encode(&query));

        if let Ok(res) = client.get(&tosho_url).send().await {
            if let Ok(entries) = res.json::<Vec<Value>>().await {
                for entry in entries.iter().take(6) {
                    if let Some(sub_url) = entry["attachments"].as_array().and_then(|arr| {
                        arr.iter().find_map(|att| {
                            let fname = att["filename"].as_str().unwrap_or("").to_lowercase();
                            if (fname.contains("spa") || fname.contains("es-419") || fname.contains("latin"))
                                && (fname.ends_with(".ass") || fname.ends_with(".srt"))
                            {
                                att["url"].as_str().map(|s| s.to_string())
                            } else {
                                None
                            }
                        })
                    }) {
                        if let Ok(saved) = fetch_and_save_srt(&client, &sub_url, episode).await {
                            return Ok(saved);
                        }
                    }
                }
            }
        }
    }

    Err("No se encontraron subtítulos externos en español para este capítulo.".to_string())
}

async fn fetch_and_save_srt(client: &Client, url: &str, episode: i32) -> Result<String, String> {
    let bytes = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Error descargando subtítulo: {}", e))?
        .bytes()
        .await
        .map_err(|e| format!("Error leyendo bytes: {}", e))?;

    let mut temp_path: PathBuf = env::temp_dir();
    let ext = if url.to_lowercase().contains(".ass") { "ass" } else { "srt" };
    temp_path.push(format!("crunchy_sub_es_ep{}.{}", episode, ext));

    if bytes.len() > 4 && &bytes[0..2] == b"PK" {
        let cursor = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|e| format!("Error abriendo ZIP: {}", e))?;

        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|e| format!("Error leyendo entrada ZIP: {}", e))?;

            let name = file.name().unwrap_or_default().to_lowercase();
            if name.ends_with(".srt") || name.ends_with(".ass") || name.ends_with(".vtt") {
                let mut content = Vec::new();
                file.read_to_end(&mut content)
                    .map_err(|e| format!("Error extrayendo archivo: {}", e))?;

                if name.ends_with(".ass") {
                    temp_path.set_extension("ass");
                }
                fs::write(&temp_path, content)
                    .map_err(|e| format!("Error guardando archivo: {}", e))?;
                return Ok(temp_path.to_string_lossy().to_string());
            }
        }
        return Err("ZIP sin archivos de subtítulos válidos.".to_string());
    }

    fs::write(&temp_path, &bytes)
        .map_err(|e| format!("Error guardando subtítulo: {}", e))?;

    Ok(temp_path.to_string_lossy().to_string())
}
