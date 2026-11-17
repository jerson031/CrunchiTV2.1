use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

const SUBDOMAIN: &str = "graphql";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AnimeItem {
    pub id: i32,
    pub title: String,
    pub title_romaji: String,
    pub synonyms: Vec<String>,
    pub cover: String,
    pub banner: Option<String>,
    pub desc: String,
    pub ep: String,
    pub total_episodes: i32,
    pub genres: Vec<String>,
}

#[derive(Deserialize)]
struct GraphQLResponse {
    data: Option<PageData>,
}

#[derive(Deserialize)]
struct PageData {
    #[serde(rename = "Page")]
    page: MediaList,
}

#[derive(Deserialize)]
struct MediaList {
    media: Vec<MediaNode>,
}
#[derive(Deserialize)]
struct AiringGraphQLResponse {
    data: Option<AiringPageData>,
}

#[derive(Deserialize)]
struct AiringPageData {
    #[serde(rename = "Page")]
    page: AiringScheduleList,
}

#[derive(Deserialize)]
struct AiringScheduleList {
    #[serde(rename = "airingSchedules")]
    airing_schedules: Vec<AiringScheduleNode>,
}

#[derive(Deserialize)]
struct AiringScheduleNode {
    media: MediaNode,
}

#[derive(Deserialize)]
struct MediaNode {
    id: i32,
    title: MediaTitle,
    synonyms: Option<Vec<String>>,
    episodes: Option<i32>,
    genres: Option<Vec<String>>,
    #[serde(rename = "coverImage")]
    cover_image: CoverImage,
    #[serde(rename = "bannerImage")]
    banner_image: Option<String>,
    description: Option<String>,
    #[serde(rename = "nextAiringEpisode")]
    next_airing_episode: Option<NextAiring>,
}

#[derive(Deserialize)]
struct MediaTitle {
    romaji: String,
    english: Option<String>,
}

#[derive(Deserialize)]
struct CoverImage {
    #[serde(rename = "extraLarge")]
    extra_large: String,
}

#[derive(Deserialize)]
struct NextAiring {
    episode: i32,
}

/// Limpia etiquetas HTML residuales de las sinopsis de AniList
fn clean_description(raw: Option<String>) -> String {
    match raw {
        Some(text) if !text.trim().is_empty() => text
            .replace("<br>", " ")
            .replace("<br/>", " ")
            .replace("<i>", "")
            .replace("</i>", "")
            .replace("<b>", "")
            .replace("</b>", "")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        _ => "Sin descripción disponible.".to_string(),
    }
}

pub async fn fetch_airing_anime() -> Result<Vec<AnimeItem>, String> {
    let client = crate::build_http_client(15)?;

    let query = r#"
        query {
            Page(page: 1, perPage: 35) {
                media(status: RELEASING, type: ANIME, sort: POPULARITY_DESC, isAdult: false) {
                    id
                    title {
                        romaji
                        english
                    }
                    synonyms
                    episodes
                    genres
                    coverImage {
                        extraLarge
                    }
                    bannerImage
                    description(asHtml: false)
                    nextAiringEpisode {
                        episode
                    }
                }
            }
        }
    "#;

    let url = format!("https://{}.anilist.co", SUBDOMAIN);
    let response = client
        .post(&url)
        .json(&json!({ "query": query }))
        .send()
        .await
        .map_err(|e| format!("Error de red con AniList: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "AniList respondió con estado HTTP {}",
            response.status().as_u16()
        ));
    }

    let parsed: GraphQLResponse = response
        .json()
        .await
        .map_err(|e| format!("Error leyendo JSON de AniList: {}", e))?;

    let nodes = parsed.data.map(|d| d.page.media).unwrap_or_default();
    Ok(map_nodes_to_items(nodes))
}

fn map_nodes_to_items(nodes: Vec<MediaNode>) -> Vec<AnimeItem> {
    nodes
        .into_iter()
        .map(|node| {
            let display_title = node
                .title
                .english
                .clone()
                .unwrap_or_else(|| node.title.romaji.clone());

            let latest_available = match node.next_airing_episode.as_ref() {
                Some(next) if next.episode > 1 => next.episode - 1,
                _ => node.episodes.unwrap_or(1),
            };

            let total_episodes = node
                .episodes
                .unwrap_or(latest_available)
                .max(latest_available);

            let ep_text = match node.next_airing_episode {
                Some(next) if next.episode > 1 => {
                    format!("Episodio {} • Sub Español", next.episode - 1)
                }
                _ => format!("{} Episodios • Sub Español", total_episodes),
            };

            let clean_synonyms: Vec<String> = node
                .synonyms
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| {
                    s.len() >= 3
                        && s.len() <= 65
                        && s.chars()
                            .all(|c| c.is_ascii_graphic() || c.is_ascii_whitespace())
                })
                .collect();

            AnimeItem {
                id: node.id,
                title: display_title,
                title_romaji: node.title.romaji,
                synonyms: clean_synonyms,
                cover: node.cover_image.extra_large,
                banner: node.banner_image,
                desc: clean_description(node.description),
                ep: ep_text,
                total_episodes,
                genres: node.genres.unwrap_or_default(),
            }
        })
        .collect()
}

pub async fn search_anime_anilist(search_query: &str) -> Result<Vec<AnimeItem>, String> {
    let trimmed = search_query.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let client = crate::build_http_client(15)?;

    let query = r#"
        query ($search: String) {
            Page(page: 1, perPage: 24) {
        media(search: $search, type: ANIME, status_not: NOT_YET_RELEASED, sort: POPULARITY_DESC, isAdult: false) 
 {
                    id
                    title {
                        romaji
                        english
                    }
                    synonyms
                    episodes
                    genres
                    coverImage {
                        extraLarge
                    }
                    bannerImage
                    description(asHtml: false)
                    nextAiringEpisode {
                        episode
                    }
                }
            }
        }
    "#;

    let url = format!("https://{}.anilist.co", SUBDOMAIN);
    let response = client
        .post(&url)
        .json(&json!({
            "query": query,
            "variables": { "search": trimmed }
        }))
        .send()
        .await
        .map_err(|e| format!("Error buscando en AniList: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "AniList respondió HTTP {}",
            response.status().as_u16()
        ));
    }

    let parsed: GraphQLResponse = response
        .json()
        .await
        .map_err(|e| format!("Error leyendo búsqueda de AniList: {}", e))?;

    let nodes = parsed.data.map(|d| d.page.media).unwrap_or_default();
    Ok(map_nodes_to_items(nodes))
}
pub async fn fetch_today_airing_anime() -> Result<Vec<AnimeItem>, String> {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Ventana de últimas 24 horas hasta las próximas 6 horas
    let start_window = now_secs - (24 * 3600);
    let end_window = now_secs + (6 * 3600);

    let client = crate::build_http_client(15)?;

    let query = r#"
        query ($start: Int, $end: Int) {
            Page(page: 1, perPage: 30) {
                airingSchedules(airingAt_greater: $start, airingAt_lesser: $end, sort: TIME_DESC) {
                    media {
                        id
                        title {
                            romaji
                            english
                        }
                        synonyms
                        episodes
                        genres
                        coverImage {
                            extraLarge
                        }
                        bannerImage
                        description(asHtml: false)
                        nextAiringEpisode {
                            episode
                        }
                    }
                }
            }
        }
    "#;

    let url = format!("https://{}.anilist.co", SUBDOMAIN);
    let response = client
        .post(&url)
        .json(&json!({
            "query": query,
            "variables": { "start": start_window, "end": end_window }
        }))
        .send()
        .await
        .map_err(|e| format!("Error de red con AniList (Hoy): {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "AniList respondió HTTP {}",
            response.status().as_u16()
        ));
    }

    let parsed: AiringGraphQLResponse = response
        .json()
        .await
        .map_err(|e| format!("Error leyendo JSON de hoy: {}", e))?;

    let schedules = parsed
        .data
        .map(|d| d.page.airing_schedules)
        .unwrap_or_default();

    let mut seen_ids = std::collections::HashSet::new();
    let mut today_nodes = Vec::new();
    for s in schedules {
        if seen_ids.insert(s.media.id) {
            today_nodes.push(s.media);
        }
    }

    Ok(map_nodes_to_items(today_nodes))
}
pub async fn fetch_catalog_page(
    page: i32,
    per_page: i32,
    genre: String,
    sort: String,
    year: Option<String>,
    format: Option<String>,
) -> Result<Vec<AnimeItem>, String> {
    let client = crate::build_http_client(15)?;

    let sort_enum = match sort.as_str() {
        "TITLE" => "TITLE_ROMAJI",
        "SCORE" => "SCORE_DESC",
        "TRENDING" => "TRENDING_DESC",
        "NEWEST" => "START_DATE_DESC",
        _ => "POPULARITY_DESC",
    };

    let is_gore = genre.eq_ignore_ascii_case("Gore");
    let has_genre = !genre.is_empty() && genre != "ALL" && !is_gore;

    let mut var_defs = vec!["$page: Int", "$perPage: Int"];
    let mut media_args = vec![
        "type: ANIME".to_string(),
        "status_not: NOT_YET_RELEASED".to_string(),
        "isAdult: false".to_string(),
        format!("sort: [{}]", sort_enum),
    ];
    let mut vars_obj = json!({
        "page": page.max(1),
        "perPage": per_page.clamp(10, 50),
    });

    if has_genre {
        var_defs.push("$genre: String");
        media_args.push("genre: $genre".to_string());
        vars_obj["genre"] = json!(genre);
    } else if is_gore {
        var_defs.push("$tag: String");
        media_args.push("tag: $tag".to_string());
        vars_obj["tag"] = json!("Gore");
    }

    if let Some(ref y) = year {
        let y_clean = y.trim();
        if y_clean == "RELEASING" {
            media_args.push("status: RELEASING".to_string());
        } else if y_clean == "CLASSICS" {
            media_args.push("startDate_lesser: 20200000".to_string());
        } else if y_clean == "2020_2022" {
            media_args.push("startDate_greater: 20200000".to_string());
            media_args.push("startDate_lesser: 20230000".to_string());
        } else if let Ok(year_num) = y_clean.parse::<i32>() {
            var_defs.push("$seasonYear: Int");
            media_args.push("seasonYear: $seasonYear".to_string());
            vars_obj["seasonYear"] = json!(year_num);
        }
    }

    if let Some(ref fmt) = format {
        match fmt.trim() {
            "TV" => media_args.push("format: TV".to_string()),
            "MOVIE" => media_args.push("format: MOVIE".to_string()),
            "OVA" => media_args.push("format_in: [OVA, SPECIAL, ONA]".to_string()),
            _ => {}
        }
    }

    let query = format!(
        r#"
        query ({vars}) {{
            Page(page: $page, perPage: $perPage) {{
                media({args}) {{
                    id
                    title {{
                        romaji
                        english
                    }}
                    synonyms
                    episodes
                    genres
                    coverImage {{
                        extraLarge
                    }}
                    bannerImage
                    description(asHtml: false)
                    nextAiringEpisode {{
                        episode
                    }}
                }}
            }}
        }}
        "#,
        vars = var_defs.join(", "),
        args = media_args.join(", "),
    );

    let url = format!("https://{}.anilist.co", SUBDOMAIN);
    let response = client
        .post(&url)
        .json(&json!({
            "query": query,
            "variables": vars_obj
        }))
        .send()
        .await
        .map_err(|e| format!("Error de red con AniList (Catálogo): {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "AniList respondió HTTP {}",
            response.status().as_u16()
        ));
    }

    let parsed: GraphQLResponse = response
        .json()
        .await
        .map_err(|e| format!("Error leyendo JSON del catálogo: {}", e))?;

    let nodes = parsed.data.map(|d| d.page.media).unwrap_or_default();
    Ok(map_nodes_to_items(nodes))
}
