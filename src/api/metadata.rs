use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::db::{self, NewExternalMetadata};

use super::{api_error, db_unavailable, valid_job_id, AppState};

#[derive(Debug, Deserialize)]
pub struct MetadataSearchQuery {
    provider: Option<String>,
    q: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LinkJobBody {
    provider: Option<String>,
    provider_id: Option<String>,
    media_kind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LinkSeriesBody {
    media_type: Option<String>,
    series_name: Option<String>,
    provider: Option<String>,
    provider_id: Option<String>,
    media_kind: Option<String>,
}

pub async fn handle_search(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MetadataSearchQuery>,
) -> Response {
    let provider = match query.provider.as_deref().filter(|s| !s.is_empty()) {
        Some("tmdb") => "tmdb",
        Some("anilist") => "anilist",
        Some(other) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_provider",
                format!("unknown provider '{other}'; use tmdb or anilist"),
            )
        }
        None => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "missing_provider",
                "provider query parameter is required (tmdb or anilist)",
            )
        }
    };

    let search_query = match query.q.as_deref().filter(|s| !s.is_empty()) {
        Some(q) => q.trim().to_string(),
        None => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "missing_query",
                "q query parameter is required",
            )
        }
    };

    let cfg = state.config.read().await.clone();

    let results = match provider {
        "tmdb" => {
            if cfg.tmdb_api_key.is_empty() {
                return api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "tmdb_unavailable",
                    "TMDB_API_KEY is not configured",
                );
            }
            search_tmdb(&state.http, &cfg.tmdb_api_key, &search_query).await
        }
        "anilist" => search_anilist(&state.http, &search_query).await,
        _ => unreachable!(),
    };

    match results {
        Ok(items) => Json(json!({ "results": items, "provider": provider })).into_response(),
        Err(e) => api_error(StatusCode::BAD_GATEWAY, "provider_error", e.to_string()),
    }
}

pub async fn handle_link_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
    Json(body): Json<LinkJobBody>,
) -> Response {
    if !valid_job_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }

    let provider = body.provider.as_deref().unwrap_or("tmdb").to_string();
    let provider_id = match body.provider_id.filter(|s| !s.is_empty()) {
        Some(id) => id,
        None => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "missing_provider_id",
                "provider_id required",
            )
        }
    };
    let media_kind = body.media_kind.as_deref().unwrap_or("movie").to_string();

    if !matches!(provider.as_str(), "tmdb" | "anilist") {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_provider",
            "provider must be tmdb or anilist",
        );
    }

    let cfg = state.config.read().await.clone();

    let new_meta =
        match fetch_metadata(&state.http, &cfg, &provider, &provider_id, &media_kind).await {
            Ok(m) => m,
            Err(e) => return api_error(StatusCode::BAD_GATEWAY, "provider_error", e.to_string()),
        };

    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let jid = job_id.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<i64> {
        let meta_id = db::save_external_metadata(&conn, &new_meta)?;
        db::link_job_metadata(&conn, &jid, meta_id, "primary")?;

        // If this job belongs to a series, also create the series-level link
        // and rename the series to the metadata title.
        if let Ok(Some(job)) = db::get_job(&conn, &jid) {
            if !job.series_name.is_empty() {
                let new_title = new_meta.title.clone();
                if !new_title.is_empty() && new_title != job.series_name {
                    db::rename_series(&conn, &job.series_name, &new_title, &job.media_type)?;
                    db::link_series_metadata(&conn, &job.media_type, &new_title, meta_id)?;
                } else {
                    db::link_series_metadata(&conn, &job.media_type, &job.series_name, meta_id)?;
                }
            }
        }
        Ok(meta_id)
    })
    .await;

    match result {
        Ok(Ok(meta_id)) => {
            Json(json!({ "linked": true, "job_id": job_id, "metadata_id": meta_id }))
                .into_response()
        }
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "link_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "link_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_link_series(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LinkSeriesBody>,
) -> Response {
    let media_type = match body.media_type.filter(|s| !s.is_empty()) {
        Some(mt) => mt,
        None => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "missing_media_type",
                "media_type required",
            )
        }
    };
    let series_name = match body.series_name.filter(|s| !s.is_empty()) {
        Some(sn) => sn,
        None => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "missing_series_name",
                "series_name required",
            )
        }
    };
    let provider = body.provider.as_deref().unwrap_or("tmdb").to_string();
    let provider_id = match body.provider_id.filter(|s| !s.is_empty()) {
        Some(id) => id,
        None => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "missing_provider_id",
                "provider_id required",
            )
        }
    };
    let media_kind = body.media_kind.as_deref().unwrap_or("tv").to_string();

    if !matches!(provider.as_str(), "tmdb" | "anilist") {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_provider",
            "provider must be tmdb or anilist",
        );
    }

    let cfg = state.config.read().await.clone();

    let new_meta =
        match fetch_metadata(&state.http, &cfg, &provider, &provider_id, &media_kind).await {
            Ok(m) => m,
            Err(e) => return api_error(StatusCode::BAD_GATEWAY, "provider_error", e.to_string()),
        };

    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let mt = media_type.clone();
    let sn = series_name.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<i64> {
        let meta_id = db::save_external_metadata(&conn, &new_meta)?;
        db::link_series_metadata(&conn, &mt, &sn, meta_id)?;
        Ok(meta_id)
    })
    .await;

    match result {
        Ok(Ok(meta_id)) => {
            if provider == "tmdb" && matches!(media_kind.as_str(), "tv" | "anime") {
                let state2 = state.clone();
                let pid = provider_id.clone();
                let sn = series_name.clone();
                tokio::spawn(async move {
                    backfill_tmdb_episode_titles(&state2, &pid, &sn).await;
                });
            }
            Json(json!({ "linked": true, "media_type": media_type, "series_name": series_name, "metadata_id": meta_id })).into_response()
        }
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "link_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "link_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_refresh(
    State(state): State<Arc<AppState>>,
    Path(metadata_id): Path<i64>,
) -> Response {
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let existing = match tokio::task::spawn_blocking(move || {
        db::get_external_metadata_by_id(&conn, metadata_id)
    })
    .await
    {
        Ok(Ok(Some(r))) => r,
        Ok(Ok(None)) => return api_error(StatusCode::NOT_FOUND, "not_found", "metadata not found"),
        Ok(Err(e)) => {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string())
        }
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string()),
    };

    let cfg = state.config.read().await.clone();

    let new_meta = match fetch_metadata(
        &state.http,
        &cfg,
        &existing.provider,
        &existing.provider_id,
        &existing.media_kind,
    )
    .await
    {
        Ok(m) => m,
        Err(e) => return api_error(StatusCode::BAD_GATEWAY, "provider_error", e.to_string()),
    };

    let conn2 = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let result =
        tokio::task::spawn_blocking(move || db::save_external_metadata(&conn2, &new_meta)).await;
    match result {
        Ok(Ok(id)) => Json(json!({ "refreshed": true, "metadata_id": id })).into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "refresh_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "refresh_failed",
            e.to_string(),
        ),
    }
}

pub(crate) async fn fetch_metadata(
    client: &reqwest::Client,
    cfg: &crate::config::Config,
    provider: &str,
    provider_id: &str,
    media_kind: &str,
) -> Result<NewExternalMetadata, String> {
    match provider {
        "tmdb" => {
            if cfg.tmdb_api_key.is_empty() {
                return Err("TMDB_API_KEY is not configured".into());
            }
            fetch_tmdb(client, &cfg.tmdb_api_key, provider_id, media_kind).await
        }
        "anilist" => fetch_anilist(client, provider_id, media_kind).await,
        _ => Err(format!("unknown provider: {provider}")),
    }
}

async fn fetch_tmdb(
    client: &reqwest::Client,
    api_key: &str,
    provider_id: &str,
    media_kind: &str,
) -> Result<NewExternalMetadata, String> {
    let url = format!(
        "https://api.themoviedb.org/3/{kind}/{id}?api_key={key}",
        kind = media_kind,
        id = provider_id,
        key = api_key
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("TMDB fetch: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("TMDB status {}", resp.status()));
    }
    let body: Value = resp.json().await.map_err(|e| format!("TMDB parse: {e}"))?;

    let title = body["name"]
        .as_str()
        .or(body["title"].as_str())
        .unwrap_or("")
        .to_string();
    let original_title = body["original_name"]
        .as_str()
        .or(body["original_title"].as_str())
        .unwrap_or("")
        .to_string();
    let overview = body["overview"].as_str().unwrap_or("").to_string();
    let poster = tmdb_image(body["poster_path"].as_str(), "w500");
    let backdrop = tmdb_image(body["backdrop_path"].as_str(), "w1280");
    let release_date = body["first_air_date"]
        .as_str()
        .or(body["release_date"].as_str())
        .unwrap_or("")
        .to_string();
    let year = release_date
        .split('-')
        .next()
        .and_then(|s| s.parse::<i64>().ok());
    let rating = body["vote_average"].as_f64();
    let raw_json = serde_json::to_string(&body).unwrap_or_default();

    Ok(NewExternalMetadata {
        provider: "tmdb".into(),
        provider_id: provider_id.to_string(),
        media_kind: media_kind.to_string(),
        title,
        original_title,
        overview,
        poster_url: poster,
        backdrop_url: backdrop,
        release_date,
        year,
        rating,
        raw_json,
    })
}

async fn fetch_anilist(
    client: &reqwest::Client,
    provider_id: &str,
    media_kind: &str,
) -> Result<NewExternalMetadata, String> {
    let gql = json!({
        "query": "query ($id: Int) { Media(id: $id) { id title { romaji english } description coverImage { large extraLarge } bannerImage startDate { year month day } format } }",
        "variables": { "id": provider_id.parse::<i64>().unwrap_or(0) }
    });
    let resp = client
        .post("https://graphql.anilist.co")
        .json(&gql)
        .send()
        .await
        .map_err(|e| format!("AniList fetch: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("AniList status {}", resp.status()));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("AniList parse: {e}"))?;
    let media = &body["data"]["Media"];

    let title = media["title"]["english"]
        .as_str()
        .or(media["title"]["romaji"].as_str())
        .unwrap_or("")
        .to_string();
    let original_title = media["title"]["romaji"].as_str().unwrap_or("").to_string();
    let overview = media["description"].as_str().unwrap_or("").to_string();
    let poster = media["coverImage"]["large"]
        .as_str()
        .or(media["coverImage"]["extraLarge"].as_str())
        .unwrap_or("")
        .to_string();
    let backdrop = media["bannerImage"].as_str().unwrap_or("").to_string();
    let start = &media["startDate"];
    let release_date = start["year"]
        .as_i64()
        .map(|y| {
            format!(
                "{}-{:02}-{:02}",
                y,
                start["month"].as_i64().unwrap_or(1),
                start["day"].as_i64().unwrap_or(1)
            )
        })
        .unwrap_or_default();
    let year = release_date
        .split('-')
        .next()
        .and_then(|s| s.parse::<i64>().ok());
    let raw_json = serde_json::to_string(&body).unwrap_or_default();

    Ok(NewExternalMetadata {
        provider: "anilist".into(),
        provider_id: provider_id.to_string(),
        media_kind: media_kind.to_string(),
        title,
        original_title,
        overview,
        poster_url: poster,
        backdrop_url: backdrop,
        release_date,
        year,
        rating: None,
        raw_json,
    })
}

pub(crate) async fn search_tmdb(
    client: &reqwest::Client,
    api_key: &str,
    query: &str,
) -> Result<Vec<Value>, String> {
    let url = format!(
        "https://api.themoviedb.org/3/search/multi?api_key={}&query={}&include_adult=false",
        api_key,
        url_encode(query)
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("TMDB search: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("TMDB status {}", resp.status()));
    }
    let body: Value = resp.json().await.map_err(|e| format!("TMDB parse: {e}"))?;
    let results: Vec<Value> = body["results"].as_array().unwrap_or(&vec![])
        .iter()
        .filter_map(|item| {
            let media_type = item["media_type"].as_str()?;
            if !matches!(media_type, "movie" | "tv") { return None; }
            Some(json!({
                "provider": "tmdb",
                "provider_id": item["id"].to_string(),
                "media_kind": media_type,
                "title": item["name"].as_str().or(item["title"].as_str()).unwrap_or(""),
                "original_title": item["original_name"].as_str().or(item["original_title"].as_str()).unwrap_or(""),
                "overview": item["overview"].as_str().unwrap_or(""),
                "release_date": item["first_air_date"].as_str().or(item["release_date"].as_str()).unwrap_or(""),
                "poster_url": tmdb_image(item["poster_path"].as_str(), "w500"),
                "backdrop_url": tmdb_image(item["backdrop_path"].as_str(), "w1280"),
                "year": item["first_air_date"].as_str().or(item["release_date"].as_str()).and_then(|d| d.split('-').next().and_then(|y| y.parse::<i64>().ok()))
            }))
        })
        .collect();
    Ok(results)
}

pub(crate) async fn search_anilist(
    client: &reqwest::Client,
    query: &str,
) -> Result<Vec<Value>, String> {
    let gql = json!({
        "query": "query ($q: String) { Page(page: 1, perPage: 20) { media(search: $q, type: ANIME, sort: SEARCH_MATCH) { id title { romaji english } description coverImage { large extraLarge } bannerImage startDate { year month day } format } } }",
        "variables": { "q": query }
    });
    let resp = client
        .post("https://graphql.anilist.co")
        .json(&gql)
        .send()
        .await
        .map_err(|e| format!("AniList search: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("AniList status {}", resp.status()));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("AniList parse: {e}"))?;
    let results: Vec<Value> = body["data"]["Page"]["media"].as_array().unwrap_or(&vec![])
        .iter()
        .map(|item| {
            let start = &item["startDate"];
            let release_date = start["year"].as_i64()
                .map(|y| format!("{}-{:02}-{:02}", y, start["month"].as_i64().unwrap_or(1), start["day"].as_i64().unwrap_or(1)))
                .unwrap_or_default();
            json!({
                "provider": "anilist",
                "provider_id": item["id"].to_string(),
                "media_kind": "anime",
                "title": item["title"]["english"].as_str().or(item["title"]["romaji"].as_str()).unwrap_or(""),
                "original_title": item["title"]["romaji"].as_str().unwrap_or(""),
                "overview": item["description"].as_str().unwrap_or(""),
                "poster_url": item["coverImage"]["large"].as_str().or(item["coverImage"]["extraLarge"].as_str()).unwrap_or(""),
                "backdrop_url": item["bannerImage"].as_str().unwrap_or(""),
                "release_date": release_date,
                "year": start["year"].as_i64()
            })
        })
        .collect();
    Ok(results)
}

pub(crate) async fn auto_fetch_and_link(
    state: &std::sync::Arc<AppState>,
    job_id: &str,
    search_term: &str,
    media_type: &str,
) {
    let cfg = state.config.read().await.clone();
    let provider = if media_type.starts_with("Anime") {
        "anilist"
    } else {
        "tmdb"
    };
    if provider == "tmdb" && cfg.tmdb_api_key.is_empty() {
        return;
    }

    let results = match provider {
        "anilist" => search_anilist(&state.http, search_term).await,
        _ => search_tmdb(&state.http, &cfg.tmdb_api_key, search_term).await,
    };
    let results = match results {
        Ok(r) if !r.is_empty() => r,
        _ => return,
    };

    let first = &results[0];
    let prov = first["provider"].as_str().unwrap_or(provider).to_string();
    let prov_id = match first["provider_id"].as_str().filter(|s| !s.is_empty()) {
        Some(id) => id.to_string(),
        None => return,
    };
    let media_kind = first["media_kind"].as_str().unwrap_or("movie").to_string();

    let new_meta = match fetch_metadata(&state.http, &cfg, &prov, &prov_id, &media_kind).await {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!(job_id, error = %e, "auto-fetch metadata skipped");
            return;
        }
    };

    let conn = match state.db_conn().await {
        Ok(c) => c,
        Err(_) => return,
    };
    let jid = job_id.to_string();
    let _ = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let meta_id = db::save_external_metadata(&conn, &new_meta)?;
        db::link_job_metadata(&conn, &jid, meta_id, "primary")?;
        Ok(())
    })
    .await;
    tracing::info!(job_id, provider = %prov, "auto-fetched and linked metadata");
}

pub(crate) async fn backfill_tmdb_episode_titles(
    state: &Arc<AppState>,
    tmdb_series_id: &str,
    series_name: &str,
) {
    let cfg = state.config.read().await.clone();
    if cfg.tmdb_api_key.is_empty() {
        return;
    }
    let client = &state.http;
    let api_key = cfg.tmdb_api_key.clone();
    let series_id = tmdb_series_id.to_string();

    // Fetch the TV show to get its seasons list
    let show_url = format!("https://api.themoviedb.org/3/tv/{series_id}?api_key={api_key}");
    let show: Value = match client.get(&show_url).send().await {
        Ok(r) if r.status().is_success() => match r.json().await {
            Ok(v) => v,
            Err(_) => return,
        },
        _ => return,
    };

    let seasons = match show["seasons"].as_array() {
        Some(s) => s.clone(),
        None => return,
    };

    // Build season_num -> { ep_num -> title } map
    let mut ep_titles: std::collections::HashMap<(i64, i64), String> =
        std::collections::HashMap::new();
    for season in &seasons {
        let sn = match season["season_number"].as_i64() {
            Some(n) if n > 0 => n,
            _ => continue,
        };
        let season_url =
            format!("https://api.themoviedb.org/3/tv/{series_id}/season/{sn}?api_key={api_key}");
        let season_data: Value = match client.get(&season_url).send().await {
            Ok(r) if r.status().is_success() => match r.json().await {
                Ok(v) => v,
                Err(_) => continue,
            },
            _ => continue,
        };
        if let Some(episodes) = season_data["episodes"].as_array() {
            for ep in episodes {
                if let (Some(en), Some(name)) = (ep["episode_number"].as_i64(), ep["name"].as_str())
                {
                    if !name.is_empty() {
                        ep_titles.insert((sn, en), name.to_string());
                    }
                }
            }
        }
    }

    if ep_titles.is_empty() {
        return;
    }

    let conn = match state.db_conn().await {
        Ok(c) => c,
        Err(_) => return,
    };
    let sn = series_name.to_string();
    let _ = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let jobs = db::get_season_episode_job_ids(&conn, &sn)?;
        let updates: Vec<(String, String)> = jobs
            .into_iter()
            .filter_map(|(job_id, season, ep)| {
                ep_titles.get(&(season, ep)).map(|t| (job_id, t.clone()))
            })
            .collect();
        if !updates.is_empty() {
            db::set_episode_titles(&conn, &updates)?;
        }
        Ok(())
    })
    .await;
    tracing::info!(series_name, "backfilled TMDB episode titles");
}

fn tmdb_image(path: Option<&str>, size: &str) -> String {
    match path.filter(|p| !p.is_empty()) {
        Some(p) => format!("https://image.tmdb.org/t/p/{size}{p}"),
        None => String::new(),
    }
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            _ => format!("%{:02X}", b),
        })
        .collect()
}
