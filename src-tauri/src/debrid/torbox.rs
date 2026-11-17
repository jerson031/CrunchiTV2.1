use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::time::Duration;
use tokio::time::sleep;

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
/// Limpia tokens técnicos (1080p, 10bit, x265, CRC32) y evalúa coincidencia exacta con el episodio pedido
fn score_video_file(
    full_path: &str,
    size: i64,
    target_ep: i32,
    target_season: i32,
    q1: f64,
    q2: f64,
    q3: f64,
    iqr: f64,
) -> i32 {
    let low = full_path.to_lowercase();

    // 1. Penalización severa a carpetas o archivos de extras / openings / especiales
    let junk_markers = [
        "/extras",
        "\\extras",
        "/special",
        "\\special",
        "/ova",
        "\\ova",
        "/sp/",
        "\\sp\\",
        "ncop",
        "nced",
        "creditless",
        "clean opening",
        "clean ending",
        "preview",
        "trailer",
        "menu",
        "bonus",
        "pv0",
        "pv1",
        "cm0",
    ];
    let mut score: i32 = 0;
    if let Some(f_season) = extract_file_season(&low) {
        if f_season == target_season {
            score += 50;
        } else {
            score -= 80;
        }
    }
    if junk_markers.iter().any(|m| low.contains(m)) {
        score -= 120;
    }

    // Extraemos solo el nombre de archivo sin ruta ni extensión
    let file_only = low.rsplit(['/', '\\']).next().unwrap_or(&low);
    let stem = file_only
        .strip_suffix(".mkv")
        .or_else(|| file_only.strip_suffix(".mp4"))
        .or_else(|| file_only.strip_suffix(".webm"))
        .unwrap_or(file_only);

    // Tokenizamos por cualquier delimitador (_ - espacio . [ ] ( ))
    let tokens: Vec<&str> = stem
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();

    let mut found_exact_ep = false;
    let mut found_other_ep = false;

    for tok in &tokens {
        // Ignoramos metadatos técnicos y años
        if matches!(
            *tok,
            "1080p"
                | "720p"
                | "480p"
                | "2160p"
                | "4k"
                | "10bit"
                | "8bit"
                | "x264"
                | "x265"
                | "h264"
                | "h265"
                | "hevc"
                | "av1"
                | "aac"
                | "aac2"
                | "flac"
                | "ac3"
                | "dts"
                | "mp4"
                | "mkv"
                | "v2"
                | "v3"
        ) {
            continue;
        }
        // Ignoramos hashes CRC32 de 8 caracteres hexadecimales (ej. e74280ad)
        if tok.len() == 8 && tok.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }

        // Caso A: Formato S01E17 / E17 / EP17
        let ep_num_opt = if let Some(rest) = tok.strip_prefix("ep") {
            rest.trim_end_matches("v2")
                .trim_end_matches("v3")
                .parse::<i32>()
                .ok()
        } else if let Some(rest) = tok.strip_prefix('e') {
            rest.trim_end_matches("v2")
                .trim_end_matches("v3")
                .parse::<i32>()
                .ok()
        } else if let Some((_, e_part)) = tok.split_once('e') {
            if tok.starts_with('s') {
                e_part
                    .trim_end_matches("v2")
                    .trim_end_matches("v3")
                    .parse::<i32>()
                    .ok()
            } else {
                None
            }
        } else {
            // Caso B: Número puro delimitado (ej. "17" en "[Cleo]Texhnolyze_-_17_(Dual...)" o "17v2")
            let num_part = tok.trim_end_matches("v2").trim_end_matches("v3");
            if !num_part.is_empty()
                && num_part.len() <= 4
                && num_part.chars().all(|c| c.is_ascii_digit())
            {
                let n = num_part.parse::<i32>().unwrap_or(-1);
                // Descartamos años (1980..2030) para no confundirlos con episodios
                if (1980..=2030).contains(&n) {
                    None
                } else {
                    Some(n)
                }
            } else {
                None
            }
        };

        if let Some(num) = ep_num_opt {
            if num == target_ep {
                found_exact_ep = true;
            } else if num > 0 {
                found_other_ep = true;
            }
        }
    }

    if found_exact_ep {
        score += 85;
    } else if found_other_ep {
        score -= 95;
    }

    // 3. Evaluación Estadística por Cuartiles (IQR) y Mediana (Q2)
    if q2 > 0.0 && size > 0 {
        let s = size as f64;
        let lower_fence = (q1 - 1.5 * iqr).max(q2 * 0.45);
        let upper_fence = (q3 + 1.5 * iqr).max(q2 * 1.85);

        if s >= lower_fence && s <= upper_fence {
            score += 20; // Tamaño consistente con la distribución normal de episodios
        } else if s < q2 * 0.35 {
            score -= 40; // Outlier diminuto (Opening/Ending o preview suelto)
        }
    }

    score
}

pub async fn resolve_direct_link(
    api_key: &str,
    magnet: &str,
    episode: i32,
    target_season: i32,
) -> Result<String, String> {
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
                let sizes = video_files
                    .iter()
                    .map(|f| f["size"].as_i64().unwrap_or(0))
                    .filter(|&s| s > 0)
                    .collect::<Vec<_>>();
                let (q1, q2, q3, iqr) = compute_quartile_stats(&sizes);

                let mut best_match: Option<(&Value, i32)> = None;
                for f in &video_files {
                    let full_path = f["name"]
                        .as_str()
                        .or_else(|| f["short_name"].as_str())
                        .unwrap_or("");
                    let size = f["size"].as_i64().unwrap_or(0);
                    let f_score =
                        score_video_file(full_path, size, episode, target_season, q1, q2, q3, iqr);

                    match best_match {
                        None => best_match = Some((f, f_score)),
                        Some((prev_f, prev_score)) => {
                            let prev_size = prev_f["size"].as_i64().unwrap_or(0);
                            if (f_score, size) > (prev_score, prev_size) {
                                best_match = Some((f, f_score));
                            }
                        }
                    }
                }

                // Seguro genérico: si es un pack multi-archivo y ningún video coincide con el episodio/temporada (score < 0), rechazamos este torrent
                if video_files.len() > 1 {
                    if let Some((_, best_score)) = best_match {
                        if best_score < 0 {
                            return Err(format!(
                                "El pack no contiene el episodio {} de la temporada {}.",
                                episode, target_season
                            ));
                        }
                    }
                }

                if let Some((file, _)) = best_match {
                    if let Some(id) = file["id"].as_i64() {
                        let chosen_name = file["short_name"]
                            .as_str()
                            .or_else(|| file["name"].as_str())
                            .unwrap_or("video");
                        println!(
                            "[TorBox] Archivo seleccionado (Ep. {}): {}",
                            episode, chosen_name
                        );
                        main_file_id = Some(id);
                        break;
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
