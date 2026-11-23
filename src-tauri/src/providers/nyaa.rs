use reqwest::Client;
use roxmltree::Document;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentCandidate {
    pub title: String,
    pub magnet: String,
    pub info_hash: String,
    pub size_mb: u64,
    pub seeders: u32,
    pub resolution_score: u8,
    pub spanish_score: u8,
}

/// Convierte cadenas como "1.4 GiB" o "485.2 MiB" a megabytes para desempatar por ligereza en TVs
fn parse_nyaa_size_mb(raw_size: &str) -> u64 {
    let parts: Vec<&str> = raw_size.split_whitespace().collect();
    if parts.len() < 2 {
        return 9999;
    }
    let val: f64 = parts[0].parse().unwrap_or(9999.0);
    match parts[1].to_uppercase().as_str() {
        "GIB" | "GB" => (val * 1024.0) as u64,
        "MIB" | "MB" => val as u64,
        "KIB" | "KB" => (val / 1024.0) as u64,
        _ => 9999,
    }
}

/// Prioriza 1080p comprimido (WEB-DL/WEBRip/BDRip) sobre 720p, y deja BD Remux/ISO gigantes como respaldo
pub fn calculate_resolution_score(title: &str) -> u8 {
    let low = title.to_lowercase();
    if low.contains("remux") || low.contains("bdmv") || low.contains(".iso") {
        return 1;
    }
    if low.contains("1080p") || low.contains("1920x1080") {
        3
    } else if low.contains("720p") || low.contains("1280x720") {
        2
    } else if low.contains("480p") || low.contains("848x480") {
        0
    } else {
        1
    }
}

/// Valida de forma 100% genérica que el torrent corresponda al episodio o sea un pack/película completa
pub fn title_matches_episode(title: &str, episode: i32) -> bool {
    let low = title.to_lowercase();

    // 1. Si es un pack explícito de temporada/serie, lo permitimos (torbox.rs seleccionará el archivo exacto)
    if low.contains("batch")
        || low.contains("complete")
        || low.contains("season")
        || low.contains("001-")
        || low.contains("01-")
        || low.contains("1~")
    {
        return true;
    }

    // 2. Quitamos etiquetas técnicas/años para ver si el título menciona algún número de episodio suelto
    let cleaned = low
        .replace("1080p", " ")
        .replace("720p", " ")
        .replace("480p", " ")
        .replace("2160p", " ")
        .replace("10bit", " ")
        .replace("10-bit", " ")
        .replace("x264", " ")
        .replace("x265", " ")
        .replace("h264", " ")
        .replace("h265", " ")
        .replace("aac2.0", " ")
        .replace("5.1", " ")
        .replace("2.0", " ");

    // Detectamos tokens numéricos de episodio (ej. "06", "12", "1180", "e06", "ep06") ignorando años (1980..2030)
    let has_any_episode_number = cleaned.split(|c: char| !c.is_alphanumeric()).any(|tok| {
        let num_str = tok
            .strip_prefix("ep")
            .or_else(|| tok.strip_prefix('e'))
            .unwrap_or(tok);
        if let Ok(n) = num_str.parse::<i32>() {
            // Es un número de episodio si tiene 2-4 dígitos y no es un año de estreno
            tok.len() >= 2 && !(1970..=2035).contains(&n)
        } else {
            false
        }
    });

    // Si el torrent no tiene número de episodio (es una película o un BDrip de serie completa), lo permitimos
    if !has_any_episode_number {
        return true;
    }

    // 3. Si sí tiene números de episodio, exigimos que coincida exactamente con el episodio pedido
    let patterns = if episode >= 100 {
        vec![
            format!(" - {} ", episode),
            format!(" - {}[", episode),
            format!(" - {}(", episode),
            format!("ep{}", episode),
            format!("ep {}", episode),
            format!("e{}", episode),
            format!(" {} ", episode),
            format!(" {}[", episode),
            format!(" {}(", episode),
        ]
    } else {
        vec![
            format!(" - {:02}", episode),
            format!("e{:02}", episode),
            format!("ep{:02}", episode),
            format!("ep {:02}", episode),
            format!(" {:02} ", episode),
            format!(" {:02}[", episode),
            format!(" {:02}(", episode),
            format!("_{:02}_", episode),
        ]
    };

    patterns.iter().any(|p| low.contains(p))
}

/// Evita que al pedir Temporada 1 de una franquicia (ej. "Tokyo Ghoul") se cuelen secuelas (ej. ":Re", "Season 2", "Part 2")
pub fn title_matches_season_context(torrent_title: &str, requested_titles: &[String]) -> bool {
    let t_low = torrent_title.to_lowercase();
    let req_joined = requested_titles.join(" ").to_lowercase();

    // Si es un pack completo multi-temporada explícito, lo permitimos
    if t_low.contains("complete") || t_low.contains("s01-s") || t_low.contains("season 1-") {
        return true;
    }

    let sequel_markers = [
        "season 2",
        "season 3",
        "season 4",
        "season 5",
        "2nd season",
        "3rd season",
        "4th season",
        "s02",
        "s03",
        "s04",
        "s05",
        "part 2",
        "part 3",
        "cour 2",
        "ghoul:re",
        "ghoul-re",
        "ghoul - re",
        "ghoul re",
        "root a",
        "√a",
        "shippuden",
        "brotherhood",
        "alicization",
    ];

    for marker in sequel_markers {
        let torrent_has = t_low.contains(marker);
        let request_has = req_joined.contains(marker);
        // Si el torrent pertenece a una secuela que el usuario NO pidió, lo descartamos
        if torrent_has && !request_has {
            return false;
        }
    }

    true
}
/// Calcula la probabilidad real de contener Español Latino/España (CR > MultiSub occidental > BILI > Inglés)
pub fn calculate_spanish_score(title: &str, category: &str) -> u8 {
    let low = title.to_lowercase();

    let is_bili = low.contains("bili") || low.contains("bilibili");
    let is_cr_or_western_sub = low.contains("erai-raws")
        || low.contains("puyasubs")
        || low.contains("came-esp")
        || low.contains("darkmahou")
        || low.contains(" cr ")
        || low.contains("[cr]")
        || low.contains("crunchyroll");

    let has_explicit_spanish = [
        "latino",
        "es-la",
        "es-419",
        "español",
        "espanol",
        "sub esp",
        "sub-esp",
        "subesp",
        "castellano",
        "[esp]",
        "(esp)",
        "[spa]",
        "(spa)",
    ]
    .iter()
    .any(|t| low.contains(t));

    let has_multisub = [
        "multisub",
        "multi-sub",
        "multi subs",
        "multi-subs",
        "[multi]",
        "(multi)",
        "multiple subtitle",
    ]
    .iter()
    .any(|t| low.contains(t));

    // Si el título enumera idiomas explícitos como [ENG][POR-BR][ARA] pero NO incluye [SPA] ni [ESP], no tiene español
    let lists_specific_langs_without_spanish =
        low.contains("[eng]") && low.contains("[ara]") && !has_explicit_spanish;

    // 1. Máxima prioridad (100): Español explícito o rip de Crunchyroll/Fansub Latino con MultiSub
    if has_explicit_spanish
        || (is_cr_or_western_sub && has_multisub && !lists_specific_langs_without_spanish)
    {
        return 100;
    }

    // 2. Alta prioridad (90): Multi-Subs de fuentes occidentales (Judas, DKB, VARYG, dragsterPS, AMZN, NF)
    if has_multisub && !is_bili && !lists_specific_langs_without_spanish {
        return 90;
    }
    // 3. Prioridad media-alta (75): Multi-Subs de BILI (Bilibili) o categoría 1_3 (Non-English)
    if (has_multisub && is_bili) || category == "1_3" {
        return 75;
    }

    // 4. Prioridad media (60): Grupos que suelen incluir softsubs múltiples sin etiquetar
    let multi_track_groups = [
        "erai-raws",
        "varyg",
        "judas",
        "dkb",
        "dragsterps",
        "ducksterps",
        "toonshub",
    ];
    if multi_track_groups.iter().any(|g| low.contains(g)) && !is_bili {
        return 60;
    }

    // 5. Prioridad baja (40): Dual-Audio
    if low.contains("dual") {
        return 40;
    }

    // 6. Último recurso (20): Releases solo inglés (SubsPlease, Ember) -> activarán traducción IA
    if low.contains("subsplease") || low.contains("ember") {
        return 20;
    }

    10
}

/// Normalización 100% genérica para cualquier título de anime
fn normalize_generic_title(raw: &str) -> Vec<String> {
    let mut results = Vec::new();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return results;
    }

    let sanitized = trimmed
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if !sanitized.is_empty() {
        results.push(sanitized.clone());
    }

    let tokens: Vec<&str> = sanitized.split_whitespace().collect();
    let structural_markers = [
        "season",
        "stage",
        "part",
        "cour",
        "arc",
        "chapter",
        "tv",
        "animation",
    ];
    let ordinals = ["1st", "2nd", "3rd", "4th", "5th", "6th", "7th", "final"];

    let mut cut_index = tokens.len();
    for (i, token) in tokens.iter().enumerate() {
        let low = token.to_lowercase();
        if i >= 2
            && (ordinals.contains(&low.as_str()) || structural_markers.contains(&low.as_str()))
        {
            cut_index = i;
            break;
        }
    }

    if cut_index < tokens.len() && cut_index >= 1 {
        let base_title = tokens[..cut_index].join(" ");
        if base_title.len() >= 3 {
            results.push(base_title);
        }
    }

    if let Some((before_colon, after_colon)) = trimmed.split_once(':') {
        // 1. Siempre agregamos la parte ANTES de ':' (ej. "Ninja Scroll: The Series" -> "Ninja Scroll")
        let pre = before_colon
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c.is_whitespace() {
                    c
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if pre.len() >= 4 {
            results.push(pre);
        }

        // 2. Solo agregamos la parte DESPUÉS de ':' si es un subtítulo real (nunca "The Series", "The Animation", etc.)
        let sub = after_colon
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c.is_whitespace() {
                    c
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        let sub_low = sub.to_lowercase();
        let generic_subtitles = [
            "the series",
            "the animation",
            "the movie",
            "tv",
            "ova",
            "ona",
            "special",
            "part",
            "season",
        ];
        if sub.len() >= 6
            && !generic_subtitles
                .iter()
                .any(|g| sub_low == *g || sub_low.starts_with(g))
            && !ordinals.iter().any(|o| sub_low.starts_with(o))
            && !structural_markers.iter().any(|m| sub_low.starts_with(m))
        {
            results.push(sub);
        }
    }

    results.dedup();
    results
}

async fn query_nyaa_rss(
    client: &Client,
    query: &str,
    category: &str,
) -> Result<Vec<TorrentCandidate>, String> {
    let encoded_query = urlencoding::encode(query);
    let hosts = ["nyaa.si", "nyaa.land"];
    let mut last_err = String::new();

    for host in hosts {
        let url = format!(
            "https://{}/?page=rss&q={}&c={}&f=0",
            host, encoded_query, category
        );

        let resp = match client
            .get(&url)
            .header(
                "Accept",
                "application/rss+xml, application/xml, text/xml, */*",
            )
            .header("Accept-Language", "es-ES,es;q=0.9,en;q=0.8")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("Error de red con {}: {}", host, e);
                println!("[Nyaa Error] {}", last_err);
                continue;
            }
        };

        let status = resp.status();
        if !status.is_success() {
            last_err = format!("{} devolvió HTTP {}", host, status);
            println!("[Nyaa Error] {}", last_err);
            continue;
        }

        let xml_text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                last_err = format!("Error leyendo XML de {}: {}", host, e);
                continue;
            }
        };

        let doc = match Document::parse(&xml_text) {
            Ok(d) => d,
            Err(e) => {
                last_err = format!(
                    "{} no devolvió un RSS válido (posible bloqueo/captcha): {}",
                    host, e
                );
                println!("[Nyaa Error] {}", last_err);
                continue;
            }
        };

        let mut candidates = Vec::new();
        for item in doc.descendants().filter(|n| n.has_tag_name("item")) {
            let title = item
                .children()
                .find(|n| n.has_tag_name("title"))
                .and_then(|n| n.text())
                .unwrap_or_default()
                .to_string();

            let info_hash = item
                .children()
                .find(|n| n.tag_name().name() == "infoHash")
                .and_then(|n| n.text())
                .unwrap_or_default()
                .to_lowercase();

            let size_str = item
                .children()
                .find(|n| n.tag_name().name() == "size")
                .and_then(|n| n.text())
                .unwrap_or_default();

            let seeders = item
                .children()
                .find(|n| n.tag_name().name() == "seeders")
                .and_then(|n| n.text())
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(1);

            if !title.is_empty() && !info_hash.is_empty() {
                let spanish_score = calculate_spanish_score(&title, category);
                let resolution_score = calculate_resolution_score(&title);
                let size_mb = parse_nyaa_size_mb(size_str);
                let magnet = format!(
                    "magnet:?xt=urn:btih:{}&dn={}",
                    info_hash,
                    urlencoding::encode(&title)
                );

                candidates.push(TorrentCandidate {
                    title,
                    info_hash,
                    magnet,
                    spanish_score,
                    resolution_score,
                    size_mb,
                    seeders,
                });
            }
        }

        println!(
            "[Nyaa OK] {} -> '{}' devolvió {} resultados",
            host,
            query,
            candidates.len()
        );
        return Ok(candidates);
    }

    Err(last_err)
}

pub async fn search_anime_generic(
    titles: &[String],
    episode: i32,
    year: Option<i32>,
) -> Result<Vec<TorrentCandidate>, String> {
    let client = crate::build_http_client(15)?;
    let is_classic = year.map(|y| y <= 2015).unwrap_or(false);

    let mut all_variants = Vec::new();
    for raw_title in titles {
        for v in normalize_generic_title(raw_title) {
            if v.len() >= 5 && !all_variants.contains(&v) {
                all_variants.push(v);
            }
        }
    }
    let ep_classic = if episode >= 100 {
        format!("{}", episode)
    } else {
        format!("{:02}", episode)
    };

    let mut combined = Vec::new();
    let mut seen_hashes = HashSet::new();
    let mut last_network_error: Option<String> = None;

    // 1. Consultas por episodio (en clásicos usamos 2 variantes principales para dejar espacio a packs)
    let variant_limit = if is_classic { 2 } else { 4 };
    let mut queries_to_try: Vec<String> = Vec::new();
    for variant in all_variants.iter().take(variant_limit) {
        queries_to_try.push(format!("{} {}", variant, ep_classic));
    }

    for raw_t in titles.iter().take(2) {
        let low_t = raw_t.to_lowercase();
        let season_num = if low_t.contains("season 2")
            || low_t.contains("2nd season")
            || low_t.ends_with(" 2")
        {
            Some(2)
        } else if low_t.contains("season 3")
            || low_t.contains("3rd season")
            || low_t.ends_with(" 3")
        {
            Some(3)
        } else if low_t.contains("season 4")
            || low_t.contains("4th season")
            || low_t.ends_with(" 4")
        {
            Some(4)
        } else {
            None
        };

        if let Some(s_num) = season_num {
            if let Some(base_clean) = normalize_generic_title(raw_t).into_iter().last() {
                queries_to_try.push(format!("{} S{:02}E{:02}", base_clean, s_num, episode));
            }
        }
    }
    queries_to_try.dedup();

    for q in queries_to_try {
        match query_nyaa_rss(&client, &q, "1_0").await {
            Ok(items) => {
                for item in items {
                    if title_matches_episode(&item.title, episode)
                        && title_matches_season_context(&item.title, titles)
                        && seen_hashes.insert(item.info_hash.clone())
                    {
                        combined.push(item);
                    }
                }
            }
            Err(e) => {
                last_network_error = Some(e);
            }
        }

        // En modernos cortamos si ya tenemos español; en clásicos siempre buscamos packs también
        if !is_classic && combined.iter().any(|c| c.spanish_score >= 90) {
            break;
        }
    }

    // 2. Modo Clásico (<= 2015): búsqueda dirigida a packs completos y grupos de re-encode
    if is_classic {
        if let Some(primary_title) = all_variants.first() {
            println!(
                "[Nyaa Modo Clásico ({})] Buscando batches para: {}",
                year.unwrap_or(0),
                primary_title
            );
            let classic_queries = [
                format!("{} Batch", primary_title),
                format!("{} Complete", primary_title),
                format!("{} Judas", primary_title),
                format!("{} \"Anime Time\"", primary_title),
                format!("{} AnimeRG", primary_title),
            ];

            for cq in classic_queries {
                match query_nyaa_rss(&client, &cq, "1_0").await {
                    Ok(items) => {
                        for item in items.into_iter().take(25) {
                            if title_matches_episode(&item.title, episode)
                                && title_matches_season_context(&item.title, titles)
                                && seen_hashes.insert(item.info_hash.clone())
                            {
                                combined.push(item);
                            }
                        }
                    }
                    Err(e) => {
                        last_network_error = Some(e);
                    }
                }
            }
        }
    }

    // 3. Respaldo genérico para OVAs/Specials/Packs antiguos (ampliado a take(65))
    if !combined.iter().any(|c| c.spanish_score >= 85) {
        if let Some(primary_title) = all_variants.first() {
            println!(
                "[Nyaa Fallback] Buscando packs/OVAs por título completo: {}",
                primary_title
            );
            match query_nyaa_rss(&client, primary_title, "1_0").await {
                Ok(items) => {
                    for item in items.into_iter().take(65) {
                        if title_matches_episode(&item.title, episode)
                            && title_matches_season_context(&item.title, titles)
                            && seen_hashes.insert(item.info_hash.clone())
                        {
                            combined.push(item);
                        }
                    }
                }
                Err(e) => {
                    last_network_error = Some(e);
                }
            }
        }
    }

    if combined.is_empty() {
        if let Some(err) = last_network_error {
            return Err(format!("No se pudo consultar Nyaa: {}", err));
        }
    }

    // Ordenamos: 1° mayor puntuación en español, 2° mejor resolución (1080p > 720p > 480p), 3° archivo más ligero
    combined.sort_by(|a, b| {
        b.spanish_score
            .cmp(&a.spanish_score)
            .then_with(|| b.resolution_score.cmp(&a.resolution_score))
            .then_with(|| a.size_mb.cmp(&b.size_mb))
    });

    for c in combined.iter().take(5) {
        println!(
            "[Nyaa Ranking] Score: {} | Res: {} | {} MB | {}",
            c.spanish_score, c.resolution_score, c.size_mb, c.title
        );
    }

    Ok(combined)
}
