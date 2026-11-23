mod debrid;
mod metadata;
mod player;
mod providers;
mod subtitles;
use providers::multi_aggregator::search_multi_provider_ranked;

use debrid::torbox::{find_first_cached_hash, last_pack_episode_files, resolve_direct_link};
use metadata::anilist::{
    AnimeItem, fetch_airing_anime, fetch_catalog_page, fetch_today_airing_anime,
    search_anime_anilist,
};

use player::mpv::{
    PlayerStatus, get_player_status, launch_mpv_player, send_mpv_ipc, stop_mpv_player,
};

use serde_json::json;
use std::collections::HashSet;
use tauri::{Manager, Window, WindowEvent};

// Recuerda regenerar tu clave en TorBox cuando termines tus pruebas
/// La clave sale de .env en tiempo de compilación (ver build.rs); no va escrita en el código
const TORBOX_API_KEY: &str = match option_env!("TORBOX_API_KEY") {
    Some(k) => k,
    None => "",
};
/// Configura la ventana de Tauri para flotar transparente encima de MPV en escritorio
fn set_osd_window_mode(window: &Window, active: bool) {
    #[cfg(not(target_os = "android"))]
    {
        if active {
            let _ = window.set_decorations(false);
            let _ = window.set_fullscreen(true);
            let _ = window.set_always_on_top(true);
            let _ = window.set_focus();
        } else {
            let _ = window.set_always_on_top(false);
            let _ = window.set_fullscreen(false);
            let _ = window.set_decorations(true);
        }
    }
    #[cfg(target_os = "android")]
    {
        let _ = (window, active);
    }
}

#[tauri::command]
async fn get_airing_anime() -> Result<Vec<AnimeItem>, String> {
    fetch_airing_anime().await
}
#[tauri::command]
async fn search_anime(query: String) -> Result<Vec<AnimeItem>, String> {
    search_anime_anilist(&query).await
}
#[tauri::command]
async fn get_today_anime() -> Result<Vec<AnimeItem>, String> {
    fetch_today_airing_anime().await
}

#[tauri::command]
async fn get_catalog_page(
    page: i32,
    genre: String,
    sort: String,
    year: Option<String>,
    format: Option<String>,
) -> Result<Vec<AnimeItem>, String> {
    fetch_catalog_page(page, 25, genre, sort, year, format).await
}

// --- NUEVO CÓDIGO FASE 2 ---
mod mappings;
use mappings::{
    SavedSourceMapping, SavedSubtitleChoice, clear_subtitle_choice, get_saved_mapping,
    load_last_play, save_mapping_hint, save_subtitle_choice,
};
use providers::multi_aggregator::search_multi_provider_scored;
use serde::Serialize;
use tauri::AppHandle;

#[derive(Debug, Clone, Serialize)]
pub struct SourceOptionDto {
    pub title: String,
    pub magnet: String,
    pub info_hash: String,
    pub source: String,
    pub total_pct: u8,
    pub size_mb: u64,
    pub seeders: u32,
    pub resolution_score: u8,
    pub spanish_score: u8,
    pub is_cached: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlayEpisodeResponse {
    pub status: String, // "playing" | "select_source"
    pub played_title: Option<String>,
    pub reason: Option<String>,
    pub candidates: Vec<SourceOptionDto>,
}

fn build_search_titles(title: &str, title_romaji: &str, synonyms: Vec<String>) -> Vec<String> {
    let mut seen_titles = HashSet::new();
    let mut search_titles = Vec::new();
    for raw in std::iter::once(title.to_string())
        .chain(std::iter::once(title_romaji.to_string()))
        .chain(synonyms.into_iter())
    {
        let clean = raw.trim().to_string();
        if !clean.is_empty() && seen_titles.insert(clean.to_lowercase()) {
            search_titles.push(clean);
        }
    }
    search_titles
}

#[tauri::command]
async fn search_episode_sources(
    title: String,
    title_romaji: String,
    synonyms: Vec<String>,
    episode: i32,
    year: Option<i32>,
) -> Result<Vec<SourceOptionDto>, String> {
    let search_titles = build_search_titles(&title, &title_romaji, synonyms);
    let mut scored = search_multi_provider_scored(&search_titles, episode, year).await?;

    if scored.iter().all(|s| s.total_pct < 60) {
        if let Ok(packs) = search_multi_provider_scored(&search_titles, 0, year).await {
            let mut existing: HashSet<String> = scored
                .iter()
                .map(|s| s.candidate.info_hash.clone())
                .collect();

            for p in packs {
                if existing.insert(p.candidate.info_hash.clone()) {
                    scored.push(p);
                }
            }
            scored.sort_by(|a, b| {
                b.total_pct
                    .cmp(&a.total_pct)
                    .then_with(|| a.candidate.size_mb.cmp(&b.candidate.size_mb))
            });
        }
    }

    let mut cached_hashes = HashSet::new();
    let mut rem: Vec<String> = scored
        .iter()
        .map(|s| s.candidate.info_hash.clone())
        .collect();
    for _ in 0..3 {
        if let Ok(Some(found)) = find_first_cached_hash(TORBOX_API_KEY, &rem).await {
            cached_hashes.insert(found.clone());
            rem.retain(|h| h != &found);
        } else {
            break;
        }
    }

    Ok(scored
        .into_iter()
        .map(|s| {
            let is_cached = cached_hashes.contains(&s.candidate.info_hash);
            SourceOptionDto {
                title: s.candidate.title,
                magnet: s.candidate.magnet,
                info_hash: s.candidate.info_hash,
                source: s.source,
                total_pct: s.total_pct,
                size_mb: s.candidate.size_mb,
                seeders: s.candidate.seeders,
                resolution_score: s.candidate.resolution_score,
                spanish_score: s.candidate.spanish_score,
                is_cached,
            }
        })
        .collect())
}

// --- SELECTOR DE SUBTÍTULOS (otro proveedor / mejor coincidencia) ---
fn subtitle_cache_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_cache_dir()
        .map(|d| d.join("subtitles"))
        .map_err(|e| format!("No se pudo obtener la carpeta de caché: {}", e))
}

/// Busca TODOS los subtítulos en español disponibles para el episodio, ya descargados,
/// analizados y puntuados (mejor primero).
#[tauri::command]
async fn search_subtitle_options(
    app: AppHandle,
    anilist_id: i32,
    title: String,
    title_romaji: String,
    synonyms: Vec<String>,
    episode: i32,
) -> Result<Vec<subtitles::fetcher::SubOption>, String> {
    let titles = build_search_titles(&title, &title_romaji, synonyms);
    let dir = subtitle_cache_dir(&app)?;
    // URL del video que se estaba viendo (para el hash del archivo y su duración)
    let stream_url = load_last_play(&app, anilist_id, episode).map(|p| p.stream_url);
    subtitles::fetcher::search_subtitle_options(
        anilist_id,
        &titles,
        episode,
        &dir,
        stream_url.as_deref(),
        true,
    )
    .await
}

/// Guarda la elección del usuario para ese episodio y reinicia la reproducción con ese subtítulo.
#[tauri::command]
async fn apply_subtitle_choice(
    app: AppHandle,
    window: Window,
    anilist_id: i32,
    episode: i32,
    option: subtitles::fetcher::SubOption,
    start_ms: Option<i64>,
) -> Result<(), String> {
    let dir = subtitle_cache_dir(&app)?;
    let src = std::path::PathBuf::from(&option.path);
    if !src.starts_with(&dir) || !src.exists() {
        return Err("El archivo de subtítulo ya no está disponible. Vuelve a buscar.".to_string());
    }
    let kept = subtitles::fetcher::persist_choice(&src, &dir, anilist_id, episode)?;

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    save_subtitle_choice(
        &app,
        anilist_id,
        episode,
        SavedSubtitleChoice {
            provider: option.provider.clone(),
            label: option.label.clone(),
            url: option.url.clone(),
            path: kept.clone(),
            format: option.format.clone(),
            score: option.score,
            saved_at: now_secs,
        },
    )?;

    #[cfg(target_os = "android")]
    {
        let last = load_last_play(&app, anilist_id, episode).ok_or_else(|| {
            "No encuentro el video que estabas viendo. Abre el episodio de nuevo: el subtítulo ya quedó guardado."
                .to_string()
        })?;
        use tauri::Emitter;
        let _ = window.emit(
            "android-exo-play",
            json!({
                "url": last.stream_url,
                "title": last.title,
                "episode": episode,
                "subtitlePath": kept,
                "preferSpanish": true,
                "startMs": start_ms.unwrap_or(0)
            }),
        );
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (&window, start_ms);
        let _ = send_mpv_ipc(json!({
            "command": ["sub-add", kept.replace('\\', "/"), "select", option.label]
        }))
        .await;
    }
    Ok(())
}

/// Traduce con IA (inglés -> español) el subtítulo de este episodio y devuelve una opción lista
/// para aplicar con `apply_subtitle_choice`. Emite "ai-sub-progress" {phase, done, total}.
#[tauri::command]
async fn ai_translate_subtitle(
    app: AppHandle,
    window: Window,
    anilist_id: i32,
    title: String,
    episode: i32,
) -> Result<subtitles::fetcher::SubOption, String> {
    use tauri::Emitter;
    let cache = subtitle_cache_dir(&app)?;
    let perm = app
        .path()
        .app_data_dir()
        .map(|d| d.join("ai_subs"))
        .map_err(|e| format!("No se pudo obtener la carpeta de datos: {}", e))?;
    let stream_url = load_last_play(&app, anilist_id, episode).map(|p| p.stream_url);
    let w = window.clone();
    let progress = move |phase: &str, done: usize, total: usize| {
        let _ = w.emit(
            "ai-sub-progress",
            json!({ "phase": phase, "done": done, "total": total }),
        );
    };
    subtitles::fetcher::ai_translate_subtitle(
        anilist_id,
        &title,
        episode,
        &cache,
        &perm,
        stream_url.as_deref(),
        &progress,
    )
    .await
}

/// Olvida la elección manual: el episodio vuelve a usar la selección automática.
#[tauri::command]
async fn clear_subtitle_choice_cmd(
    app: AppHandle,
    anilist_id: i32,
    episode: i32,
) -> Result<(), String> {
    clear_subtitle_choice(&app, anilist_id, episode)
}

#[tauri::command]
async fn play_and_save_source(
    app: AppHandle,
    window: Window,
    anilist_id: i32,
    title: String,
    title_romaji: String,
    synonyms: Vec<String>,
    episode: i32,
    magnet: String,
    info_hash: String,
    release_title: String,
    spanish_score: u8,
) -> Result<String, String> {
    let search_titles = build_search_titles(&title, &title_romaji, synonyms);
    let target_season = detect_ordinal_season(&search_titles);
    let stream_url = resolve_direct_link(TORBOX_API_KEY, &magnet, episode, target_season).await?;
    // TorBox confirma si el torrent trae varios episodios (pack), aunque el título no lo diga
    let is_pack = last_pack_episode_files() >= 3;
    let full_title = format!("{} - Episodio {}", title, episode);

    set_osd_window_mode(&window, true);
    if let Err(e) = launch_mpv_player(
        window.clone(),
        &stream_url,
        &full_title,
        search_titles,
        episode,
        anilist_id,
        spanish_score,
    )
    .await
    {
        set_osd_window_mode(&window, false);
        return Err(e);
    }

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let _ = save_mapping_hint(
        &app,
        anilist_id,
        episode,
        SavedSourceMapping {
            info_hash,
            magnet,
            title: release_title,
            spanish_score,
            saved_at: now_secs,
        },
        is_pack,
    );

    Ok(full_title)
}

#[tauri::command]
async fn play_anime_episode(
    app: AppHandle,
    window: Window,
    anilist_id: i32,
    title: String,
    title_romaji: String,
    synonyms: Vec<String>,
    episode: i32,
    year: Option<i32>,
) -> Result<PlayEpisodeResponse, String> {
    let search_titles = build_search_titles(&title, &title_romaji, synonyms.clone());
    let target_season = detect_ordinal_season(&search_titles);

    // 1. Revisar caché local mappings.json (100% garantizado si ya se eligió)
    if let Some(saved) = get_saved_mapping(&app, anilist_id, episode) {
        if let Ok(stream_url) =
            resolve_direct_link(TORBOX_API_KEY, &saved.magnet, episode, target_season).await
        {
            let full_title = format!("{} - Episodio {}", title, episode);
            set_osd_window_mode(&window, true);
            if launch_mpv_player(
                window.clone(),
                &stream_url,
                &full_title,
                search_titles.clone(),
                episode,
                anilist_id,
                saved.spanish_score,
            )
            .await
            .is_ok()
            {
                return Ok(PlayEpisodeResponse {
                    status: "playing".to_string(),
                    played_title: Some(full_title),
                    reason: None,
                    candidates: Vec::new(),
                });
            }
            set_osd_window_mode(&window, false);
        }
    }

    // 2. Buscar fuentes ordenadas por %
    let candidates =
        search_episode_sources(title.clone(), title_romaji, synonyms, episode, year).await?;
    let best_cached = candidates.iter().find(|c| c.is_cached);

    // 3. Disparo automático del selector si no hay caché en TorBox o el puntaje es < 75%
    let chosen = match best_cached {
        Some(c) if c.total_pct >= 75 => c,
        _ => {
            return Ok(PlayEpisodeResponse {
                status: "select_source".to_string(),
                played_title: None,
                reason: Some(
                    "Coincidencia baja (<75%) o sin caché directo. Elige una fuente:".to_string(),
                ),
                candidates,
            });
        }
    };

    // 4. Reproducción directa (>= 75% y en caché)
    let stream_url =
        match resolve_direct_link(TORBOX_API_KEY, &chosen.magnet, episode, target_season).await {
            Ok(u) => u,
            // El torrent elegido no trae este episodio: en vez de quedarse en un error, se deja elegir
            Err(e) if e.starts_with("Ningún archivo del torrent") => {
                println!(
                    "[Play] La fuente elegida no trae el episodio ({}). Abriendo selector.",
                    e
                );
                return Ok(PlayEpisodeResponse {
                    status: "select_source".to_string(),
                    played_title: None,
                    reason: Some(
                        "La fuente automática no contenía este episodio. Elige otra:".to_string(),
                    ),
                    candidates,
                });
            }
            Err(e) => return Err(e),
        };
    let is_pack = last_pack_episode_files() >= 3;
    let full_title = format!("{} - Episodio {}", title, episode);
    set_osd_window_mode(&window, true);
    if let Err(e) = launch_mpv_player(
        window.clone(),
        &stream_url,
        &full_title,
        search_titles,
        episode,
        anilist_id,
        chosen.spanish_score,
    )
    .await
    {
        set_osd_window_mode(&window, false);
        return Err(e);
    }

    // Si la fuente automática era un pack, se recuerda para los capítulos siguientes
    if is_pack {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = save_mapping_hint(
            &app,
            anilist_id,
            episode,
            SavedSourceMapping {
                info_hash: chosen.info_hash.clone(),
                magnet: chosen.magnet.clone(),
                title: chosen.title.clone(),
                spanish_score: chosen.spanish_score,
                saved_at: now_secs,
            },
            true,
        );
    }

    Ok(PlayEpisodeResponse {
        status: "playing".to_string(),
        played_title: Some(full_title),
        reason: None,
        candidates: Vec::new(),
    })
}

#[tauri::command]
async fn play_direct_magnet(
    window: Window,
    magnet: String,
    custom_title: String,
) -> Result<String, String> {
    let stream_url = resolve_direct_link(TORBOX_API_KEY, &magnet, 1, 1).await?;

    launch_mpv_player(
        window.clone(),
        &stream_url,
        &custom_title,
        vec![custom_title.clone()],
        1,
        0,
        0,
    )
    .await?;

    set_osd_window_mode(&window, true);
    Ok(custom_title)
}

#[tauri::command]
async fn mpv_status() -> Result<PlayerStatus, String> {
    get_player_status().await
}

#[tauri::command]
async fn mpv_toggle_pause() -> Result<(), String> {
    send_mpv_ipc(json!({ "command": ["cycle", "pause"] })).await?;
    Ok(())
}

#[tauri::command]
async fn mpv_seek(seconds: f64) -> Result<(), String> {
    send_mpv_ipc(json!({ "command": ["seek", seconds, "absolute"] })).await?;
    Ok(())
}

#[tauri::command]
async fn mpv_seek_relative(offset: f64) -> Result<(), String> {
    send_mpv_ipc(json!({ "command": ["seek", offset, "relative"] })).await?;
    Ok(())
}

#[tauri::command]
async fn mpv_set_track(track_type: String, id: i64) -> Result<(), String> {
    let prop = if track_type == "audio" { "aid" } else { "sid" };
    send_mpv_ipc(json!({ "command": ["set_property", prop, id] })).await?;
    Ok(())
}

#[tauri::command]
async fn mpv_close_player(window: Window) -> Result<(), String> {
    stop_mpv_player();
    set_osd_window_mode(&window, false);
    Ok(())
}

#[tauri::command]
async fn cancel_episode_load(window: Window) -> Result<(), String> {
    crate::player::proxy::cancel_active_proxy();
    crate::subtitles::ai_translator::cancel_active_translations();
    stop_mpv_player();
    set_osd_window_mode(&window, false);
    Ok(())
}

pub fn build_http_client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    reqwest::Client::builder()
        .use_preconfigured_tls(tls_config)
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()
        .map_err(|e| format!("Error creando cliente HTTP: {}", e))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let _ = rustls::crypto::ring::default_provider().install_default();
            // SubDL (opcional): si existe subdl_api_key.txt en la carpeta de datos, se activa
            if let Ok(dir) = app.path().app_data_dir() {
                if let Ok(key) = std::fs::read_to_string(dir.join("subdl_api_key.txt")) {
                    subtitles::fetcher::set_subdl_key(&key);
                }
                // Gemini para los subtítulos con IA: gemini_api_key.txt tiene prioridad sobre la clave integrada
                match std::fs::read_to_string(dir.join("gemini_api_key.txt")) {
                    Ok(key) if !key.trim().is_empty() => subtitles::fetcher::set_gemini_key(&key),
                    _ => {
                        subtitles::fetcher::set_gemini_key(subtitles::ai_translator::builtin_key())
                    }
                }
            } else {
                subtitles::fetcher::set_gemini_key(subtitles::ai_translator::builtin_key());
            }
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .on_window_event(|_window, event| {
            if let WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed = event {
                stop_mpv_player();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_airing_anime,
            get_today_anime,
            play_anime_episode,
            play_direct_magnet,
            mpv_status,
            mpv_toggle_pause,
            mpv_seek,
            mpv_seek_relative,
            mpv_set_track,
            mpv_close_player,
            search_anime,
            cancel_episode_load,
            get_catalog_page,
            search_episode_sources,
            play_and_save_source,
            search_subtitle_options,
            apply_subtitle_choice,
            clear_subtitle_choice_cmd,
            ai_translate_subtitle,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
fn detect_ordinal_season(titles: &[String]) -> i32 {
    for t in titles.iter().take(2) {
        let low = t.to_lowercase();
        let tokens: Vec<&str> = low
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|s| !s.is_empty())
            .collect();

        for (i, &tok) in tokens.iter().enumerate() {
            match tok {
                "ii" => return 2,
                "iii" => return 3,
                "iv" => return 4,
                _ => {}
            }
            if let Some(num_str) = tok
                .strip_suffix("st")
                .or_else(|| tok.strip_suffix("nd"))
                .or_else(|| tok.strip_suffix("rd"))
                .or_else(|| tok.strip_suffix("th"))
            {
                if let Ok(n) = num_str.parse::<i32>() {
                    if (2..=99).contains(&n) {
                        return n;
                    }
                }
            }
            if (tok == "season" || tok == "s") && i + 1 < tokens.len() {
                if let Ok(n) = tokens[i + 1].parse::<i32>() {
                    if (2..=99).contains(&n) {
                        return n;
                    }
                }
            }
            if let Some(rest) = tok.strip_prefix('s') {
                if !rest.is_empty() && rest.len() <= 3 && rest.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(n) = rest.parse::<i32>() {
                        if (2..=99).contains(&n) {
                            return n;
                        }
                    }
                }
            }
        }
    }
    1
}
