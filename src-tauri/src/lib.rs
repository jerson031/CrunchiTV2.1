mod debrid;
mod metadata;
mod player;
mod providers;
mod subtitles;
use providers::multi_aggregator::search_multi_provider_ranked;

use debrid::torbox::{find_first_cached_hash, resolve_direct_link};
use metadata::anilist::{
    AnimeItem, fetch_airing_anime, fetch_catalog_page, fetch_today_airing_anime,
    search_anime_anilist,
};

use player::mpv::{
    PlayerStatus, get_player_status, launch_mpv_player, send_mpv_ipc, stop_mpv_player,
};

use serde_json::json;
use std::collections::HashSet;
use tauri::{Window, WindowEvent};

// Recuerda regenerar tu clave en TorBox cuando termines tus pruebas
const TORBOX_API_KEY: &str = "";
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

#[tauri::command]
async fn play_anime_episode(
    window: Window,
    anilist_id: i32,
    title: String,
    title_romaji: String,
    synonyms: Vec<String>,
    episode: i32,
) -> Result<String, String> {
    // 1. Armamos la lista maestra de títulos y sinónimos sin duplicados ni vacíos
    let mut seen_titles = HashSet::new();
    let mut search_titles = Vec::new();

    for raw in std::iter::once(title.clone())
        .chain(std::iter::once(title_romaji))
        .chain(synonyms.into_iter())
    {
        let clean = raw.trim().to_string();
        if !clean.is_empty() && seen_titles.insert(clean.to_lowercase()) {
            search_titles.push(clean);
        }
    }

    // 2. Ejecutamos la búsqueda paralela multi-proveedor (Nyaa + Animetosho + Knaben/1337x) con ranking estadístico %
    let mut candidates = search_multi_provider_ranked(&search_titles, episode).await?;

    if candidates.is_empty() {
        return Err(format!(
            "No se encontraron fuentes disponibles para {} (Episodio {}).",
            title, episode
        ));
    }

    // 3. Consultamos cuáles de esos torrents están en caché instantáneo de TorBox
    let is_requested_movie = search_titles
        .iter()
        .any(|t| t.to_lowercase().contains("movie"));

    if !is_requested_movie {
        candidates.retain(|c| {
            let low = c.title.to_lowercase();
            let looks_like_movie = low.contains("the movie")
                || low.contains("(movie)")
                || low.contains("movie 01")
                || low.contains("movie 1");
            !looks_like_movie
        });
    }

    let hashes: Vec<String> = candidates.iter().map(|c| c.info_hash.clone()).collect();
    // Filtramos los candidatos que estén en caché respetando el ranking matemático
    let mut cached_candidates = Vec::new();
    let mut remaining_hashes = hashes.clone();

    for _ in 0..3 {
        if let Some(found_hash) = find_first_cached_hash(TORBOX_API_KEY, &remaining_hashes).await? {
            if let Some(pos) = candidates.iter().position(|c| c.info_hash == found_hash) {
                cached_candidates.push(candidates.remove(pos));
            }
            remaining_hashes.retain(|h| h != &found_hash);
        } else {
            break;
        }
    }
    let target_season = detect_ordinal_season(&search_titles);

    if cached_candidates.is_empty() {
        println!(
            "[TorBox Pack Rescue] Ningún suelto en caché para Ep. {} (Temporada {}). Buscando packs...",
            episode, target_season
        );
        if let Ok(mut pack_cands) = search_multi_provider_ranked(&search_titles, 0).await {
            pack_cands.retain(|c| {
                let low = c.title.to_lowercase();
                if !is_requested_movie && (low.contains("the movie") || low.contains("(movie)")) {
                    return false;
                }
                for s in 1..=99 {
                    if s != target_season {
                        let wrong_season = low.contains(&format!("season {}", s))
                            || low.contains(&format!("season {:02}", s))
                            || low.contains(&format!("(s{:02})", s))
                            || low.contains(&format!("[s{:02}]", s))
                            || low.contains(&format!(" s{:02} ", s));
                        if wrong_season {
                            return false;
                        }
                    }
                }
                true
            });

            let mut rem_pack_hashes: Vec<String> =
                pack_cands.iter().map(|c| c.info_hash.clone()).collect();
            for _ in 0..3 {
                if let Some(found_hash) =
                    find_first_cached_hash(TORBOX_API_KEY, &rem_pack_hashes).await?
                {
                    if let Some(pos) = pack_cands.iter().position(|c| c.info_hash == found_hash) {
                        cached_candidates.push(pack_cands.remove(pos));
                    }
                    rem_pack_hashes.retain(|h| h != &found_hash);
                } else {
                    break;
                }
            }
        }
    }

    if cached_candidates.is_empty() {
        return Err(format!(
            "El episodio {} de '{}' aún no está disponible en caché instantáneo.",
            episode, title
        ));
    }

    let mut chosen_pair: Option<(crate::providers::nyaa::TorrentCandidate, String)> = None;
    let mut last_resolve_err = String::new();

    for cand in cached_candidates {
        println!("[TorBox] Intentando torrent: {}", cand.title);
        match resolve_direct_link(TORBOX_API_KEY, &cand.magnet, episode, target_season).await {
            Ok(url) => {
                chosen_pair = Some((cand, url));
                break;
            }
            Err(e) => {
                println!("[TorBox Fallback] Falló '{}': {}", cand.title, e);
                last_resolve_err = e;
            }
        }
    }

    let (selected, stream_url) = chosen_pair.ok_or(last_resolve_err)?;

    // 5. Iniciamos MPV y superponemos la ventana transparente de Tauri con el OSD estilo Crunchyroll
    let full_title = format!("{} - Episodio {}", title, episode);
    set_osd_window_mode(&window, true);
    if let Err(e) = launch_mpv_player(
        window.clone(),
        &stream_url,
        &full_title,
        search_titles,
        episode,
        anilist_id,
        selected.spanish_score,
    )
    .await
    {
        set_osd_window_mode(&window, false);
        return Err(e);
    }

    Ok(full_title)
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
        .setup(|_app| {
            let _ = rustls::crypto::ring::default_provider().install_default();
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
            get_catalog_page
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
