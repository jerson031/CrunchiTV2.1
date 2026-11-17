use reqwest::Client;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::time::sleep;

pub struct ProxySession {
    pub file_path: PathBuf,
    pub downloaded_bytes: Arc<AtomicU64>,
    pub total_bytes: Arc<AtomicU64>,
    pub finished: Arc<AtomicBool>,
    pub failed: Arc<AtomicBool>,
    pub cancelled: Arc<AtomicBool>,
}

static ACTIVE_SESSION: Mutex<Option<Arc<ProxySession>>> = Mutex::new(None);

/// Cancela la sesión anterior y borra inmediatamente el archivo .mkv temporal de la TV
pub fn stop_active_proxy() {
    if let Ok(mut guard) = ACTIVE_SESSION.lock() {
        if let Some(prev) = guard.take() {
            prev.cancelled.store(true, Ordering::SeqCst);
            let path = prev.file_path.clone();
            std::thread::spawn(move || {
                for _ in 0..15 {
                    if fs::remove_file(&path).is_ok() {
                        println!("[Proxy Local] Archivo temporal eliminado: {:?}", path);
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
        }
    }
}

pub fn get_active_session() -> Option<Arc<ProxySession>> {
    ACTIVE_SESSION.lock().ok().and_then(|g| g.clone())
}

/// Inicia la descarga única desde TorBox hacia el disco temporal para extraer/traducir subtítulos
pub async fn start_single_download_proxy(
    torbox_url: &str,
    anilist_id: i32,
    episode: i32,
) -> Result<Arc<ProxySession>, String> {
    stop_active_proxy();

    let file_path = env::temp_dir().join(format!(
        "crunchy_bridge_{}_ep{}_{}.mkv",
        anilist_id,
        episode,
        std::process::id()
    ));
    let _ = fs::remove_file(&file_path);

    let session = Arc::new(ProxySession {
        file_path: file_path.clone(),
        downloaded_bytes: Arc::new(AtomicU64::new(0)),
        total_bytes: Arc::new(AtomicU64::new(0)),
        finished: Arc::new(AtomicBool::new(false)),
        failed: Arc::new(AtomicBool::new(false)),
        cancelled: Arc::new(AtomicBool::new(false)),
    });

    if let Ok(mut guard) = ACTIVE_SESSION.lock() {
        *guard = Some(session.clone());
    }

    let dl_session = session.clone();
    let url_owned = torbox_url.to_string();
    tokio::spawn(async move {
        let client = match crate::build_http_client(600) {
            Ok(c) => c,
            Err(_) => {
                dl_session.failed.store(true, Ordering::SeqCst);
                return;
            }
        };

        let mut resp = match client
            .get(&url_owned)
            .header("Referer", "https://torbox.app")
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            _ => {
                dl_session.failed.store(true, Ordering::SeqCst);
                return;
            }
        };

        if let Some(len) = resp.content_length() {
            dl_session.total_bytes.store(len, Ordering::SeqCst);
        }

        let mut file = match tokio::fs::File::create(&dl_session.file_path).await {
            Ok(f) => f,
            Err(_) => {
                dl_session.failed.store(true, Ordering::SeqCst);
                return;
            }
        };

        let mut written: u64 = 0;
        let mut last_log_mb: u64 = 0;

        loop {
            if dl_session.cancelled.load(Ordering::SeqCst) {
                drop(file);
                let _ = fs::remove_file(&dl_session.file_path);
                return;
            }

            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    if file.write_all(&chunk).await.is_err() {
                        dl_session.failed.store(true, Ordering::SeqCst);
                        return;
                    }
                    written += chunk.len() as u64;
                    dl_session.downloaded_bytes.store(written, Ordering::SeqCst);

                    let mb = written / (1024 * 1024);
                    if mb >= last_log_mb + 250 {
                        last_log_mb = mb;
                        println!("[Proxy Único] Descargado en caché local: {} MB...", mb);
                    }
                }
                Ok(None) => {
                    let _ = file.flush().await;
                    if dl_session.total_bytes.load(Ordering::SeqCst) == 0 {
                        dl_session.total_bytes.store(written, Ordering::SeqCst);
                    }
                    dl_session.finished.store(true, Ordering::SeqCst);
                    println!(
                        "[Proxy Único] Descarga completa ({} MB).",
                        written / (1024 * 1024)
                    );
                    break;
                }
                Err(e) => {
                    println!("[Proxy Único] Error de red con TorBox: {}", e);
                    dl_session.failed.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
    });

    for _ in 0..80 {
        if session.downloaded_bytes.load(Ordering::SeqCst) >= 512 * 1024
            || session.finished.load(Ordering::SeqCst)
        {
            return Ok(session);
        }
        if session.failed.load(Ordering::SeqCst) {
            return Err("No se pudo iniciar la descarga desde TorBox.".to_string());
        }
        sleep(Duration::from_millis(100)).await;
    }

    Err("Tiempo de espera agotado iniciando el flujo desde TorBox.".to_string())
}

pub fn cancel_active_proxy() {
    if let Ok(mut guard) = ACTIVE_SESSION.lock() {
        if let Some(session) = guard.take() {
            session.cancelled.store(true, Ordering::SeqCst);
            let _ = std::fs::remove_file(&session.file_path);
            println!("[Proxy Único] Descarga cancelada por el usuario.");
        }
    }
}
pub fn is_session_cancelled(_anilist_id: i32, _episode: i32) -> bool {
    if let Ok(guard) = ACTIVE_SESSION.lock() {
        match guard.as_ref() {
            Some(session) => return session.cancelled.load(Ordering::SeqCst),
            None => return true, // Si cancel_active_proxy() hizo guard.take(), ya fue cancelada
        }
    }
    false
}
