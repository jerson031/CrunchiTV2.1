#[cfg(not(target_os = "android"))]
mod desktop {

    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};
    use std::io::Read;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use tauri::{Emitter, Window};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::ClientOptions;
    use tokio::time::sleep;

    static MPV_PROCESS: Mutex<Option<Child>> = Mutex::new(None);
    const IPC_PIPE_PATH: &str = r"\\.\pipe\crunchy_mpv_ipc";

    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub struct MediaTrack {
        pub id: i64,
        pub track_type: String, // "audio" | "sub"
        pub title: String,
        pub lang: String,
        pub selected: bool,
    }

    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub struct PlayerStatus {
        pub paused: bool,
        pub time_pos: f64,
        pub duration: f64,
        pub tracks: Vec<MediaTrack>,
    }

    #[cfg(debug_assertions)]
    fn get_mpv_candidates() -> Vec<String> {
        let mut candidates = Vec::new();
        let custom_path = r"C:\Program Files\MPV Player\mpv.exe";
        if Path::new(custom_path).exists() {
            candidates.push(custom_path.to_string());
        }
        candidates.push("mpv".to_string());
        candidates
    }

    #[cfg(not(debug_assertions))]
    fn get_mpv_candidates() -> Vec<String> {
        let mut list = Vec::new();
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(dir) = exe_path.parent() {
                let bundled = dir.join("bin").join("mpv.exe");
                if bundled.exists() {
                    list.push(bundled.to_string_lossy().to_string());
                }
            }
        }
        list.push("mpv".to_string());
        list
    }

    pub fn stop_mpv_player() {
        crate::subtitles::ai_translator::cancel_active_translations();
        crate::player::proxy::stop_active_proxy();

        if let Ok(mut guard) = MPV_PROCESS.lock() {
            if let Some(mut prev_child) = guard.take() {
                let _ = prev_child.kill();
                let _ = prev_child.wait();
            }
        }
    }
    /// Envía un comando JSON-IPC a MPV y lee su respuesta
    pub async fn send_mpv_ipc(command: Value) -> Result<Value, String> {
        let client = ClientOptions::new()
            .open(IPC_PIPE_PATH)
            .map_err(|e| format!("No se pudo conectar al IPC de MPV: {}", e))?;

        let (reader, mut writer) = tokio::io::split(client);
        let mut payload = command.to_string();
        payload.push('\n');

        writer
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| format!("Error enviando comando IPC: {}", e))?;

        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();

        for _ in 0..10 {
            line.clear();
            let bytes =
                tokio::time::timeout(Duration::from_millis(800), buf_reader.read_line(&mut line))
                    .await
                    .map_err(|_| "Tiempo de espera agotado leyendo IPC de MPV".to_string())?
                    .map_err(|e| format!("Error leyendo IPC: {}", e))?;

            if bytes == 0 {
                break;
            }

            if let Ok(val) = serde_json::from_str::<Value>(&line) {
                if val.get("error").is_some() {
                    return Ok(val);
                }
            }
        }

        Err("MPV no devolvió respuesta válida por IPC".to_string())
    }

    /// Selecciona pista interna en Español Latino, descarga .srt externo o traduce con IA emitiendo estado visual
    pub async fn ensure_spanish_subtitles(
        window: &Window,
        stream_url: &str,
        anilist_id: i32,
        titles: &[String],
        episode: i32,
    ) -> Result<(), String> {
        let res = send_mpv_ipc(json!({ "command": ["get_property", "track-list"] })).await?;

        let mut best_sub_id: Option<i64> = None;
        let mut best_score = 0;

        if let Some(tracks) = res["data"].as_array() {
            for t in tracks {
                if t["type"].as_str() == Some("sub") {
                    let id = t["id"].as_i64().unwrap_or(0);
                    let title = t["title"].as_str().unwrap_or("").to_lowercase();
                    let lang = t["lang"].as_str().unwrap_or("").to_lowercase();

                    let selected = t["selected"].as_bool().unwrap_or(false);

                    let mut score = 0;
                    if title.contains("latin")
                        || title.contains("latin_america")
                        || title.contains("latin-america")
                        || title.contains("américa latina")
                        || title.contains("america latina")
                        || title.contains("latino")
                        || lang.contains("419")
                        || lang.contains("mx")
                        || lang.contains("la")
                    {
                        score = 100;
                    } else if lang == "spa"
                        || lang == "es"
                        || title.contains("spanish")
                        || title.contains("español")
                        || title.contains("espanol")
                    {
                        if title.contains("spain")
                            || title.contains("españa")
                            || title.contains("castellano")
                        {
                            score = 40;
                        } else if selected {
                            // Si MPV ya la auto-seleccionó por --slang=es-419,es-MX,es-LA,spa,es
                            score = 95;
                        } else {
                            score = 75;
                        }
                    }

                    if score > best_score {
                        best_score = score;
                        best_sub_id = Some(id);
                    }
                }
            }
        }

        // 1. PRIORIDAD MÁXIMA: Si el video ya trae subtítulos en español internos, los activamos y salimos
        if let Some(sid) = best_sub_id {
            let _ = send_mpv_ipc(json!({ "command": ["set_property", "sid", sid] })).await;
            println!(
                "[MPV Subtítulos] Pista interna en Español activada (sid: {}, score: {}). Sin IA necesaria.",
                sid, best_score
            );
            return Ok(());
        }
        // 1.5. Si ya existe un .srt traducido en caché local, lo cargamos al instante sin mostrar carga de IA
        let cached_srt = std::env::var("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("CrunchyTV")
            .join("subtitles_cache")
            .join(format!(
                "crunchy_ai_latino_{}_ep{}.srt",
                anilist_id, episode
            ));

        if cached_srt.exists() {
            let normalized_path = cached_srt.to_string_lossy().replace('\\', "/");
            let ipc_res = send_mpv_ipc(json!({
                "command": ["sub-add", normalized_path, "select", "Español Latino (Caché)"]
            }))
            .await;
            let _ = send_mpv_ipc(json!({
                "command": ["set_property", "sub-visibility", true]
            }))
            .await;
            println!(
                "[MPV IPC] Subtítulo cargado al instante desde caché: {:?}",
                ipc_res
            );
            return Ok(());
        }

        // 2. Si NO trae español interno, intentamos descargar .srt externo en español
        if let Ok(srt_path) =
            crate::subtitles::fetcher::download_spanish_subtitle(anilist_id, titles, episode).await
        {
            let normalized_path = srt_path.replace('\\', "/");
            let ipc_res = send_mpv_ipc(json!({
                "command": ["sub-add", normalized_path, "select", "Español (Automático)"]
            }))
            .await;
            println!("[MPV IPC] Subtítulo externo cargado: {:?}", ipc_res);
            return Ok(());
        }

        // 3. Último recurso: Extraer pista en inglés y traducirla al Español Latino con IA
        println!(
            "[MPV Subtítulos] No hay español externo. Iniciando traducción IA en segundo plano..."
        );
        let _ = window.emit("ai-sub-status", "loading");
        for _ in 0..20 {
            if let Ok(cache_res) =
                send_mpv_ipc(json!({ "command": ["get_property", "demuxer-cache-duration"] })).await
            {
                if let Some(secs) = cache_res["data"].as_f64() {
                    if secs >= 20.0 {
                        break;
                    }
                }
            }
            sleep(Duration::from_millis(500)).await;
        }
        match crate::subtitles::ai_translator::extract_and_translate_subtitles(
            anilist_id, stream_url, episode,
        )
        .await
        {
            Ok(ai_srt_path) => {
                let normalized_path = ai_srt_path.replace('\\', "/");
                let ipc_res = send_mpv_ipc(json!({
                    "command": ["sub-add", normalized_path, "select", "Español Latino (IA)"]
                }))
                .await;
                println!("[MPV IPC] ¡Subtítulos IA cargados!: {:?}", ipc_res);
                let _ = window.emit("ai-sub-status", "done");
            }
            Err(e) => {
                println!("[IA Subtítulos Error] {}", e);
                let _ = window.emit("ai-sub-status", "error");
            }
        }

        Ok(())
    }

    /// Obtiene el estado en tiempo real (tiempo actual, duración, pausa y pistas) para la barra estilo Crunchyroll
    pub async fn get_player_status() -> Result<PlayerStatus, String> {
        let paused = send_mpv_ipc(json!({ "command": ["get_property", "pause"] }))
            .await
            .ok()
            .and_then(|v| v["data"].as_bool())
            .unwrap_or(false);

        let time_pos = send_mpv_ipc(json!({ "command": ["get_property", "time-pos"] }))
            .await
            .ok()
            .and_then(|v| v["data"].as_f64())
            .unwrap_or(0.0);

        let duration = send_mpv_ipc(json!({ "command": ["get_property", "duration"] }))
            .await
            .ok()
            .and_then(|v| v["data"].as_f64())
            .unwrap_or(0.0);

        let tracks_val = send_mpv_ipc(json!({ "command": ["get_property", "track-list"] }))
            .await
            .unwrap_or(json!({}));

        let mut tracks = Vec::new();
        if let Some(arr) = tracks_val["data"].as_array() {
            for t in arr {
                let t_type = t["type"].as_str().unwrap_or("");
                if t_type == "audio" || t_type == "sub" {
                    let id = t["id"].as_i64().unwrap_or(0);
                    let lang = t["lang"].as_str().unwrap_or("und").to_string();
                    let raw_title = t["title"].as_str().unwrap_or("");
                    let title = if raw_title.is_empty() {
                        format!("Pista {} ({})", id, lang.to_uppercase())
                    } else {
                        raw_title.to_string()
                    };
                    let selected = t["selected"].as_bool().unwrap_or(false);

                    tracks.push(MediaTrack {
                        id,
                        track_type: t_type.to_string(),
                        title,
                        lang,
                        selected,
                    });
                }
            }
        }

        Ok(PlayerStatus {
            paused,
            time_pos,
            duration,
            tracks,
        })
    }

    pub async fn launch_mpv_player(
        window: Window,
        stream_url: &str,
        anime_title: &str,
        search_titles: Vec<String>,
        episode: i32,
        anilist_id: i32,
        spanish_score: u8,
    ) -> Result<(), String> {
        if !stream_url.starts_with("http") {
            return Err("La URL de video recibida de TorBox no es válida.".to_string());
        }

        stop_mpv_player();

        let cached_srt = std::env::var("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("CrunchyTV")
            .join("subtitles_cache")
            .join(format!(
                "crunchy_ai_latino_{}_ep{}.srt",
                anilist_id, episode
            ));

        let has_spanish_source = spanish_score >= 85 || cached_srt.exists();

        let (video_target, pre_translated_srt): (String, Option<String>) = if has_spanish_source {
            println!(
                "[MPV Directo] Fuente con Español/Caché (score: {}). Abriendo flujo directo sin descarga temporal.",
                spanish_score
            );
            (stream_url.to_string(), None)
        } else if let Ok(external_srt) = crate::subtitles::fetcher::download_spanish_subtitle(
            anilist_id,
            &search_titles,
            episode,
        )
        .await
        {
            println!(
                "[MPV Directo] Subtítulo externo encontrado en web: {}. Abriendo flujo directo sin IA.",
                external_srt
            );
            (
                stream_url.to_string(),
                Some(external_srt.replace('\\', "/")),
            )
        } else {
            println!(
                "[MPV Secuencial] Sin Español nativo ni externo (score: {}). Descargando y creando subtítulos IA antes de reproducir...",
                spanish_score
            );
            let _ = window.emit("ai-sub-status", "loading");

            // 1. Descarga única completa a disco (sin MPV abierto para no saturar red/CPU)
            let session =
                crate::player::proxy::start_single_download_proxy(stream_url, anilist_id, episode)
                    .await?;

            while !session.finished.load(Ordering::SeqCst) {
                if session.cancelled.load(Ordering::SeqCst) {
                    let _ = window.emit("ai-sub-status", "done");
                    return Err("Descarga cancelada por el usuario.".to_string());
                }
                if session.failed.load(Ordering::SeqCst) {
                    let _ = window.emit("ai-sub-status", "done");
                    return Err("Error descargando el episodio para crear subtítulos.".to_string());
                }
                tokio::time::sleep(Duration::from_millis(250)).await;

                let dl = session.downloaded_bytes.load(Ordering::SeqCst);
                let tot = session.total_bytes.load(Ordering::SeqCst);
                if tot > 0 {
                    let dl_pct = ((dl as f64 / tot as f64) * 60.0).min(60.0) as u8;
                    let dl_mb = dl / (1024 * 1024);
                    let tot_mb = tot / (1024 * 1024);
                    let _ = window.emit(
                        "load-progress",
                        json!({
                            "percent": dl_pct,
                            "label": format!("Descargando episodio ({} / {} MB)...", dl_mb, tot_mb)
                        }),
                    );
                }

                sleep(Duration::from_millis(300)).await;
            }

            let _ = window.emit(
                "load-progress",
                json!({
                    "percent": 65,
                    "label": "Extrayendo y traduciendo subtítulos al Español Latino con IA..."
                }),
            );

            let _ = window.emit(
                "load-progress",
                json!({
                    "percent": 65,
                    "label": "Extrayendo y traduciendo subtítulos al Español Latino con IA..."
                }),
            );
            // Si el usuario canceló mientras se preparaba o si ya estaba en caché, abortamos antes de tocar la IA
            if crate::player::proxy::is_session_cancelled(anilist_id, episode) {
                let _ = window.emit("ai-sub-status", "done");
                return Err("Reproducción cancelada por el usuario.".to_string());
            }

            // 2. Extracción y traducción IA secuencial
            let srt_opt = match crate::subtitles::ai_translator::extract_and_translate_subtitles(
                anilist_id, stream_url, episode,
            )
            .await
            {
                Ok(path) => {
                    let _ = window.emit("ai-sub-status", "done");
                    Some(path.replace('\\', "/"))
                }
                Err(e) => {
                    println!("[IA Subtítulos Error] {}", e);
                    let _ = window.emit("ai-sub-status", "error");
                    None
                }
            };

            // 3. Abrimos MPV directamente desde el archivo local ya descargado (0ms buffering)
            (session.file_path.to_string_lossy().to_string(), srt_opt)
        };

        let media_title_arg = format!("--force-media-title={}", anime_title);
        let ipc_arg = format!("--input-ipc-server={}", IPC_PIPE_PATH);

        let args = [
            video_target.as_str(),
            "--fs",
            "--no-border",
            "--force-window=immediate",
            "--keep-open=yes",
            "--osc=no",
            "--osd-level=0",
            "--ytdl=no",
            "--tls-verify=no",
            "--user-agent=Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            "--http-header-fields=Referer: https://torbox.app: https://torbox.app",
            "--slang=es-419,es-MX,es-LA,spa,es",
            "--alang=jpn,ja",
            "--sub-auto=fuzzy",
            "--sub-font=Segoe UI Semibold",
            "--sub-font-size=44",
            "--sub-border-size=2.5",
            "--sub-shadow-offset=1.5",
            "--hr-seek=no",
            "--cache=yes",
            "--cache-on-disk=yes",
            "--demuxer-seekable-cache=yes",
            "--demuxer-max-bytes=180M",
            "--demuxer-max-back-bytes=40M",
            "--demuxer-readahead-secs=60",
            "--stream-lavf-o=reconnect=1,reconnect_streamed=1,reconnect_delay_max=3",
            &ipc_arg,
            &media_title_arg,
        ];

        let mut spawned_child: Option<Child> = None;
        let mut last_error = String::new();

        for bin in get_mpv_candidates() {
            match Command::new(&bin)
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(child) => {
                    spawned_child = Some(child);
                    break;
                }
                Err(e) => {
                    last_error = format!("Fallo ejecutando '{}': {}", bin, e);
                }
            }
        }
        let mut child = spawned_child.ok_or_else(|| {
            format!(
                "No se pudo iniciar el reproductor MPV. Verifica que esté instalado. ({})",
                last_error
            )
        })?;
        sleep(Duration::from_millis(1800)).await;
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out_msg = String::new();
                if let Some(mut stdout) = child.stdout.take() {
                    let _ = stdout.read_to_string(&mut out_msg);
                }
                let mut err_msg = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    let _ = stderr.read_to_string(&mut err_msg);
                }
                let combined = format!("{}\n{}", out_msg.trim(), err_msg.trim());
                let detail = if combined.trim().is_empty() {
                    "Enlace caducado o formato no soportado.".to_string()
                } else {
                    combined
                        .trim()
                        .lines()
                        .take(5)
                        .collect::<Vec<_>>()
                        .join(" | ")
                };
                return Err(format!(
                    "MPV salió con código {:?}: {}",
                    status.code(),
                    detail
                ));
            }
            Ok(None) => {
                if let Some(mut stdout) = child.stdout.take() {
                    std::thread::spawn(move || {
                        let mut sink = Vec::new();
                        let _ = stdout.read_to_end(&mut sink);
                    });
                }
                if let Some(mut stderr) = child.stderr.take() {
                    std::thread::spawn(move || {
                        let mut sink = Vec::new();
                        let _ = stderr.read_to_end(&mut sink);
                    });
                }
                if let Ok(mut guard) = MPV_PROCESS.lock() {
                    *guard = Some(child);
                }
                let stream_url_owned = stream_url.to_string();
                let pre_srt_owned = pre_translated_srt.clone();
                tokio::spawn(async move {
                    // Esperar a que MPV termine de cargar el archivo real (duration > 0)
                    let mut file_loaded = false;
                    for _ in 0..80 {
                        sleep(Duration::from_millis(250)).await;
                        if let Ok(res) =
                            send_mpv_ipc(json!({ "command": ["get_property", "duration"] })).await
                        {
                            if let Some(dur) = res["data"].as_f64() {
                                if dur > 0.0 {
                                    file_loaded = true;
                                    break;
                                }
                            }
                        }
                    }

                    if !file_loaded {
                        println!("[MPV IPC] Tiempo de espera agotado esperando carga del video.");
                        return;
                    }

                    sleep(Duration::from_millis(300)).await;

                    if has_spanish_source && pre_srt_owned.is_none() {
                        // Fuente con español nativo o caché previo: activamos pista interna o caché al instante
                        let _ = ensure_spanish_subtitles(
                            &window,
                            &stream_url_owned,
                            anilist_id,
                            &search_titles,
                            episode,
                        )
                        .await;
                    } else if let Some(srt_path) = pre_srt_owned {
                        let normalized_path = srt_path.replace('\\', "/");
                        let _ = send_mpv_ipc(json!({
                            "command": ["sub-add", normalized_path, "select", "Sub Español Latino"]
                        }))
                        .await;
                        let _ = send_mpv_ipc(json!({
                            "command": ["set_property", "sub-visibility", true]
                        }))
                        .await;
                    }
                });
            }
            Err(e) => {
                return Err(format!(
                    "Error verificando el estado del reproductor: {}",
                    e
                ));
            }
        }
        Ok(())
    }
}
#[cfg(not(target_os = "android"))]
pub use desktop::*;

// ============================================================================
// MODO ANDROID / GOOGLE TV (Media3 ExoPlayer Nativo vía Eventos + Bridge)
// ============================================================================
#[cfg(target_os = "android")]
mod android {
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};
    use std::sync::Mutex;
    use tauri::{Emitter, Window};

    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub struct MediaTrack {
        pub id: i64,
        pub track_type: String,
        pub title: String,
        pub lang: String,
        pub selected: bool,
    }

    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub struct PlayerStatus {
        pub paused: bool,
        pub time_pos: f64,
        pub duration: f64,
        pub tracks: Vec<MediaTrack>,
    }

    static ANDROID_PLAYER_STATE: Mutex<Option<PlayerStatus>> = Mutex::new(None);
    static ACTIVE_WINDOW: Mutex<Option<Window>> = Mutex::new(None);

    pub fn update_android_player_state(state: PlayerStatus) {
        if let Ok(mut guard) = ANDROID_PLAYER_STATE.lock() {
            *guard = Some(state);
        }
    }

    pub fn stop_mpv_player() {
        crate::subtitles::ai_translator::cancel_active_translations();
        crate::player::proxy::stop_active_proxy();
        if let Ok(guard) = ACTIVE_WINDOW.lock() {
            if let Some(win) = guard.as_ref() {
                let _ = win.emit("android-exo-cmd", json!({ "action": "stop" }));
            }
        }
        if let Ok(mut st) = ANDROID_PLAYER_STATE.lock() {
            *st = None;
        }
    }

    pub async fn send_mpv_ipc(command: Value) -> Result<Value, String> {
        if let Ok(guard) = ACTIVE_WINDOW.lock() {
            if let Some(win) = guard.as_ref() {
                let _ = win.emit("android-exo-cmd", command);
            }
        }
        Ok(json!({ "error": "success" }))
    }

    pub async fn get_player_status() -> Result<PlayerStatus, String> {
        if let Ok(guard) = ANDROID_PLAYER_STATE.lock() {
            if let Some(st) = guard.as_ref() {
                return Ok(st.clone());
            }
        }
        Ok(PlayerStatus {
            paused: false,
            time_pos: 0.0,
            duration: 0.0,
            tracks: Vec::new(),
        })
    }

    pub async fn launch_mpv_player(
        window: Window,
        stream_url: &str,
        anime_title: &str,
        search_titles: Vec<String>,
        episode: i32,
        anilist_id: i32,
        spanish_score: u8,
    ) -> Result<(), String> {
        if !stream_url.starts_with("http") {
            return Err("La URL de video recibida de TorBox no es válida.".to_string());
        }

        if let Ok(mut guard) = ACTIVE_WINDOW.lock() {
            *guard = Some(window.clone());
        }

        let external_srt = if spanish_score < 85 {
            crate::subtitles::fetcher::download_spanish_subtitle(
                anilist_id,
                &search_titles,
                episode,
            )
            .await
            .ok()
        } else {
            None
        };

        let _ = window.emit(
            "android-exo-play",
            json!({
                "url": stream_url,
                "title": anime_title,
                "episode": episode,
                "subtitlePath": external_srt,
                "preferSpanish": true
            }),
        );

        Ok(())
    }
}

#[cfg(target_os = "android")]
pub use android::*;
