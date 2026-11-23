use reqwest::Client;
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// La clave sale de .env en tiempo de compilación (ver build.rs); no va escrita en el código
const GEMINI_API_KEY: &str = match option_env!("GEMINI_API_KEY") {
    Some(k) => k,
    None => "",
};

static TRANSLATION_GEN: AtomicU64 = AtomicU64::new(0);

/// Clave integrada (la usan los subtítulos con IA de la TV si no hay gemini_api_key.txt)
pub fn builtin_key() -> &'static str {
    GEMINI_API_KEY
}

// --- AQUÍ VA LA DEFINICIÓN DE LA FUNCIÓN ---
pub fn cancel_active_translations() {
    TRANSLATION_GEN.fetch_add(1, Ordering::SeqCst);
}

#[cfg(debug_assertions)]
fn find_ffmpeg_binary() -> String {
    let candidates = [
        r"C:\Program Files\MPV Player\ffmpeg.exe",
        r"C:\Program Files\FFmpeg\bin\ffmpeg.exe",
        r"C:\ffmpeg\bin\ffmpeg.exe",
    ];
    for c in candidates {
        if Path::new(c).exists() {
            return c.to_string();
        }
    }
    if let Ok(local_app) = env::var("LOCALAPPDATA") {
        let winget_link = PathBuf::from(local_app)
            .join("Microsoft")
            .join("WinGet")
            .join("Links")
            .join("ffmpeg.exe");
        if winget_link.exists() {
            return winget_link.to_string_lossy().to_string();
        }
    }
    "ffmpeg".to_string()
}

#[cfg(not(debug_assertions))]
fn find_ffmpeg_binary() -> String {
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(dir) = exe_path.parent() {
            let bundled = dir.join("bin").join("ffmpeg.exe");
            if bundled.exists() {
                return bundled.to_string_lossy().to_string();
            }
        }
    }
    "ffmpeg".to_string()
}
/// Carpeta permanente en AppData\Local\CrunchyTV\subtitles_cache
fn get_permanent_cache_dir() -> PathBuf {
    let dir = env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::temp_dir())
        .join("CrunchyTV")
        .join("subtitles_cache");
    let _ = fs::create_dir_all(&dir);
    dir
}

#[derive(Clone)]
struct SrtBlock {
    index: String,
    timestamp: String,
    text: String,
}

fn wrap_subtitle_for_tv(text: &str) -> String {
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.len() <= 44 {
        return clean;
    }

    let words: Vec<&str> = clean.split(' ').collect();
    if words.len() <= 1 {
        return clean;
    }

    let mid = clean.len() / 2;
    let mut current_len = 0;
    let mut split_idx = 0;

    for (i, w) in words.iter().enumerate() {
        current_len += w.len() + 1;
        if current_len >= mid {
            split_idx = i + 1;
            break;
        }
    }

    if split_idx > 0 && split_idx < words.len() {
        format!(
            "{}\n{}",
            words[..split_idx].join(" "),
            words[split_idx..].join(" ")
        )
    } else {
        clean
    }
}

fn strip_subtitle_tags(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_brace = false;
    let mut in_angle = false;

    for c in raw.chars() {
        if c == '{' {
            in_brace = true;
        } else if c == '}' {
            in_brace = false;
        } else if c == '<' {
            in_angle = true;
        } else if c == '>' {
            in_angle = false;
        } else if !in_brace && !in_angle {
            out.push(c);
        }
    }
    out.trim().to_string()
}

pub async fn extract_and_translate_subtitles(
    anilist_id: i32,
    _stream_url: &str,
    episode: i32,
) -> Result<String, String> {
    if GEMINI_API_KEY.trim().is_empty() {
        return Err("Falta configurar GEMINI_API_KEY en ai_translator.rs".to_string());
    }

    let my_gen = TRANSLATION_GEN.fetch_add(1, Ordering::SeqCst) + 1;

    let cache_dir = get_permanent_cache_dir();
    let translated_es_path = cache_dir.join(format!(
        "crunchy_ai_latino_{}_ep{}.srt",
        anilist_id, episode
    ));

    if translated_es_path.exists() {
        println!("[IA Subtítulos] Cargando traducción permanente desde caché...");
        return Ok(translated_es_path.to_string_lossy().to_string());
    }

    let temp_dir = env::temp_dir();
    let raw_en_path = temp_dir.join(format!("crunchy_raw_en_{}_ep{}.srt", anilist_id, episode));

    if !raw_en_path.exists() {
        let session = crate::player::proxy::get_active_session()
            .ok_or_else(|| "No hay descarga activa en el proxy local.".to_string())?;

        println!("[IA Subtítulos] Esperando descarga única del proxy local...");

        loop {
            if TRANSLATION_GEN.load(Ordering::SeqCst) != my_gen
                || session.cancelled.load(Ordering::SeqCst)
            {
                return Err("Traducción cancelada por cambio de episodio.".to_string());
            }
            if session.failed.load(Ordering::SeqCst) {
                return Err("La descarga del video falló en el proxy local.".to_string());
            }
            if session.finished.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        let ffmpeg_bin = find_ffmpeg_binary();
        let local_mkv_str = session.file_path.to_string_lossy().to_string();
        let raw_en_str = raw_en_path.to_string_lossy().to_string();

        let extracted_ok = tokio::task::spawn_blocking(move || -> bool {
            let mut best_srt = String::new();
            let mut best_block_count = 0usize;

            // Probamos hasta 6 pistas internas (0:s:0 .. 0:s:5) y elegimos la de diálogos completos (ignorando Signs/Karaoke)
            for track_idx in 0..6 {
                let map_arg = format!("0:s:{}", track_idx);
                let probe_path = format!("{}.probe{}.srt", raw_en_str, track_idx);

                let out = Command::new(&ffmpeg_bin)
                    .args([
                        "-y",
                        "-i",
                        &local_mkv_str,
                        "-map",
                        &map_arg,
                        "-c:s",
                        "srt",
                        &probe_path,
                    ])
                    .output();

                let mut found_track = false;
                if let Ok(res) = out {
                    if res.status.success() {
                        found_track = true;
                        if let Ok(content) = fs::read_to_string(&probe_path) {
                            let parsed_blocks = parse_srt(&content);
                            // Contamos bloques de diálogo real (> 8 caracteres, evitando sílabas de karaoke)
                            let dialogue_count = parsed_blocks
                                .iter()
                                .filter(|b| b.text.chars().count() > 8)
                                .count();
                            println!(
                                "[IA Subtítulos] Pista 0:s:{} -> {} bloques ({} diálogos reales)",
                                track_idx,
                                parsed_blocks.len(),
                                dialogue_count
                            );
                            if dialogue_count > best_block_count {
                                best_block_count = dialogue_count;
                                best_srt = content;
                            }
                        }
                    }
                }
                let _ = fs::remove_file(&probe_path);
                if !found_track {
                    break;
                }
            }

            if best_block_count > 0 {
                fs::write(&raw_en_str, best_srt).is_ok()
            } else {
                false
            }
        })
        .await
        .map_err(|e| format!("Error hilo FFmpeg: {}", e))?;

        if !extracted_ok || !raw_en_path.exists() {
            let _ = fs::remove_file(&raw_en_path);
            return Err(
                "El archivo MKV no contiene ninguna pista de subtítulos interna.".to_string(),
            );
        }
    } else {
        println!("[IA Subtítulos] Reutilizando pista en inglés ya extraída en caché...");
    }
    let srt_content = fs::read_to_string(&raw_en_path)
        .map_err(|e| format!("Error leyendo .srt extraído: {}", e))?;

    let blocks = parse_srt(&srt_content);
    if blocks.is_empty() {
        return Err("La pista de subtítulos extraída está vacía.".to_string());
    }

    let chunk_size = 95;
    let total_chunks = blocks.len().div_ceil(chunk_size);
    println!(
        "[IA Subtítulos] Traduciendo {} líneas limpias en {} lotes al Español Latino...",
        blocks.len(),
        total_chunks
    );

    let ai_client = crate::build_http_client(90)?;

    let mut translated_blocks = Vec::with_capacity(blocks.len());

    for (chunk_idx, chunk) in blocks.chunks(chunk_size).enumerate() {
        if TRANSLATION_GEN.load(Ordering::SeqCst) != my_gen {
            println!("[IA Subtítulos] Traducción detenida al instante por cancelación.");
            return Err("Traducción cancelada por el usuario.".to_string());
        }

        println!(
            "[IA Subtítulos] Traduciendo lote {}/{}...",
            chunk_idx + 1,
            total_chunks
        );
        let res = translate_chunk(&ai_client, my_gen, chunk_idx, chunk.to_vec()).await?;
        translated_blocks.extend(res);

        if chunk_idx + 1 < total_chunks {
            for _ in 0..7 {
                if TRANSLATION_GEN.load(Ordering::SeqCst) != my_gen {
                    return Err("Traducción cancelada por el usuario.".to_string());
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }

    if TRANSLATION_GEN.load(Ordering::SeqCst) != my_gen {
        return Err("Traducción cancelada por el usuario.".to_string());
    }

    let mut final_srt = String::new();
    for (i, b) in translated_blocks.into_iter().enumerate() {
        let formatted_text = wrap_subtitle_for_tv(&b.text);
        final_srt.push_str(&format!(
            "{}\n{}\n{}\n\n",
            i + 1,
            b.timestamp,
            formatted_text
        ));
    }

    fs::write(&translated_es_path, final_srt)
        .map_err(|e| format!("Error guardando .srt traducido: {}", e))?;

    let _ = fs::remove_file(&raw_en_path);

    println!("[IA Subtítulos] ¡Traducción permanente completada con éxito!");
    Ok(translated_es_path.to_string_lossy().to_string())
}

fn is_ass_vector_drawing(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.starts_with("m ") && (trimmed.contains(" l ") || trimmed.contains(" b ")) {
        let non_coord_chars = trimmed
            .chars()
            .filter(|c| !c.is_ascii_digit() && !" mlbspc.-".contains(*c))
            .count();
        return non_coord_chars == 0;
    }
    false
}

fn parse_srt(content: &str) -> Vec<SrtBlock> {
    let normalized = content.replace("\r\n", "\n");
    let mut blocks: Vec<SrtBlock> = Vec::new();

    for raw_block in normalized.split("\n\n") {
        let lines: Vec<&str> = raw_block.lines().collect();
        if lines.len() >= 3 {
            let index = lines[0].trim().to_string();
            let timestamp = lines[1].trim().to_string();
            let raw_text = lines[2..].join(" ");
            let clean_text = strip_subtitle_tags(&raw_text);

            if !timestamp.contains("-->")
                || clean_text.is_empty()
                || is_ass_vector_drawing(&clean_text)
            {
                continue;
            }

            // Fusiona fotogramas consecutivos de efectos ASS/Karaoke con el mismo texto exacto
            if let Some(prev) = blocks.last_mut() {
                if prev.text == clean_text {
                    if let (Some((start, _)), Some((_, end))) = (
                        prev.timestamp.split_once("-->"),
                        timestamp.split_once("-->"),
                    ) {
                        prev.timestamp = format!("{} --> {}", start.trim(), end.trim());
                        continue;
                    }
                }
            }

            blocks.push(SrtBlock {
                index,
                timestamp,
                text: clean_text,
            });
        }
    }
    blocks
}

async fn translate_chunk(
    client: &Client,
    my_gen: u64,
    chunk_idx: usize,
    mut chunk: Vec<SrtBlock>,
) -> Result<Vec<SrtBlock>, String> {
    let mut numbered_lines = String::new();
    for (i, b) in chunk.iter().enumerate() {
        numbered_lines.push_str(&format!("[{}] {}\n", i, b.text));
    }

    let prompt = format!(
        "Eres un traductor profesional de anime para Crunchyroll Latinoamérica.\n\
        Traduce los siguientes diálogos del inglés al Español Latino neutro, natural y conciso.\n\
        REGLAS ESTRICTAS:\n\
        1. Mantén exactamente el prefijo numérico, [1], [2], etc. al inicio de cada línea.\n\
        2. Devuelve ÚNICAMENTE las líneas traducidas, sin explicaciones ni comillas.\n\n{}",
        numbered_lines
    );

    let url = format!(
        "https://{}.{}/v1beta/interactions",
        "generativelanguage", "googleapis.com"
    );

    let attempts_config = [
        ("gemini-3.5-flash-lite", 5),
        ("gemini-3.1-flash-lite", 8),
        ("gemini-3.6-flash", 12),
        ("gemini-3.8-flash", 15),
        ("gemma-4-31b-it", 20),
    ];

    let mut last_err = String::new();
    for (attempt_idx, (model_name, backoff_secs)) in attempts_config.iter().enumerate() {
        if TRANSLATION_GEN.load(Ordering::SeqCst) != my_gen {
            return Err("Traducción cancelada por el usuario.".to_string());
        }

        let body = json!({
            "model": model_name,
            "input": prompt,
            "store": false,
            "generation_config": {
                "thinking_level": "low",
                "temperature": 0.2
            }
        });

        let res = match client
            .post(&url)
            .header("x-goog-api-key", GEMINI_API_KEY)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("Error de red (intento {}): {:?}", attempt_idx + 1, e);
                tokio::time::sleep(Duration::from_secs(*backoff_secs)).await;
                continue;
            }
        };

        let status_code = res.status();
        let json_val: Value = match res.json().await {
            Ok(v) => v,
            Err(e) => {
                last_err = format!("Error JSON (intento {}): {}", attempt_idx + 1, e);
                tokio::time::sleep(Duration::from_secs(*backoff_secs)).await;
                continue;
            }
        };

        if status_code.as_u16() == 429 || status_code.as_u16() == 503 || status_code.as_u16() >= 500
        {
            last_err = format!("Estado temporal HTTP {}", status_code);
            println!(
                "[IA Subtítulos] Lote {} en espera por cuota/servidor ({}), reintentando en {}s...",
                chunk_idx + 1,
                status_code,
                backoff_secs
            );
            tokio::time::sleep(Duration::from_secs(*backoff_secs)).await;
            continue;
        }

        if !status_code.is_success() {
            last_err = format!("Gemini respondió HTTP {}: {:?}", status_code, json_val);
            continue;
        }

        let mut output_text = String::new();
        if let Some(steps) = json_val["steps"].as_array() {
            for step in steps {
                if step["type"].as_str() == Some("model_output") {
                    if let Some(contents) = step["content"].as_array() {
                        for c in contents {
                            if let Some(txt) = c["text"].as_str() {
                                output_text.push_str(txt);
                            }
                        }
                    }
                }
            }
        }

        if !output_text.is_empty() {
            for line in output_text.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix('[') {
                    if let Some((idx_str, trans_text)) = rest.split_once(']') {
                        if let Ok(idx) = idx_str.trim().parse::<usize>() {
                            if idx < chunk.len() {
                                chunk[idx].text = trans_text.trim().to_string();
                            }
                        }
                    }
                }
            }
            return Ok(chunk);
        }
    }

    Err(format!(
        "Fallo definitivo en lote {}: {}",
        chunk_idx + 1,
        last_err
    ))
}
