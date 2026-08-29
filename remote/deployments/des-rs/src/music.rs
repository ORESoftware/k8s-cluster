use std::{
    env, fs,
    net::IpAddr,
    path::{Path as StdPath, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use axum::{
    extract::{Multipart, State},
    http::{header, HeaderName, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use des_engine::des::general::music_production::{
    analyze_music_sample_prompt, derive_music_sample_seed_from_mp4, generate_microtonal_song,
    song_spec_from_music_sample_seed_with_prompt, ArrangementSummary,
};

use crate::docs::apply_discovery_headers;
use crate::pages::MUSIC_PRODUCTION_HTML;
use crate::state::{json_error, now_ms, AppState};

pub(crate) const MAX_MUSIC_UPLOAD_BYTES: usize = 96 * 1024 * 1024;
const MAX_MUSIC_SOURCE_URL_CHARS: usize = 4096;
const MAX_MUSIC_TITLE_CHARS: usize = 160;
const MAX_MUSIC_PROMPT_CHARS: usize = 12_000;
const MAX_MUSIC_AUTH_CHARS: usize = 32_000;
const MAX_MUSIC_AUTH_HEADER_NAME_CHARS: usize = 64;
const MAX_MUSIC_COOKIE_BYTES: usize = 512 * 1024;
const MUSIC_DOWNLOAD_TIMEOUT_SECS: u64 = 180;

pub(crate) async fn music_production_page(State(state): State<AppState>) -> Response {
    let mut res = Html(MUSIC_PRODUCTION_HTML).into_response();
    apply_discovery_headers(res.headers_mut(), &state);
    res
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MusicSourceAuthMode {
    Public,
    Authenticated,
}

impl MusicSourceAuthMode {
    fn as_str(self) -> &'static str {
        match self {
            MusicSourceAuthMode::Public => "public",
            MusicSourceAuthMode::Authenticated => "authenticated",
        }
    }
}

#[derive(Clone, Debug)]
struct MusicSourceAuth {
    mode: MusicSourceAuthMode,
    auth_header_name: Option<HeaderName>,
    auth_header: Option<String>,
    cookie_header: Option<String>,
    cookies_file: Option<PathBuf>,
}

impl MusicSourceAuth {
    fn has_credentials(&self) -> bool {
        self.auth_header.is_some() || self.cookie_header.is_some() || self.cookies_file.is_some()
    }

    fn effective_mode(&self) -> MusicSourceAuthMode {
        if self.mode == MusicSourceAuthMode::Authenticated || self.has_credentials() {
            MusicSourceAuthMode::Authenticated
        } else {
            MusicSourceAuthMode::Public
        }
    }

    fn summary_json(&self) -> Value {
        json!({
            "mode": self.effective_mode().as_str(),
            "auth_header": self.auth_header.as_ref().map(|_| {
                self.auth_header_name
                    .as_ref()
                    .map(|name| name.as_str())
                    .unwrap_or(header::AUTHORIZATION.as_str())
            }),
            "cookie_header": self.cookie_header.is_some(),
            "cookies_file": self.cookies_file.is_some(),
        })
    }
}

fn parse_music_source_auth_mode(raw: &str) -> Result<MusicSourceAuthMode, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "public" => Ok(MusicSourceAuthMode::Public),
        "authenticated" | "auth" | "private" => Ok(MusicSourceAuthMode::Authenticated),
        other => Err(format!(
            "source_auth_mode must be public or authenticated, got {other:?}"
        )),
    }
}

fn clean_music_auth_field(raw: String, label: &str) -> Result<Option<String>, String> {
    let value = raw.trim().to_string();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > MAX_MUSIC_AUTH_CHARS {
        return Err(format!(
            "{label} must be at most {MAX_MUSIC_AUTH_CHARS} characters"
        ));
    }
    if value.chars().any(|ch| ch.is_control()) {
        return Err(format!("{label} must be a single HTTP header value"));
    }
    Ok(Some(value))
}

fn clean_music_auth_header_name_field(raw: String) -> Result<Option<HeaderName>, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > MAX_MUSIC_AUTH_HEADER_NAME_CHARS {
        return Err(format!(
            "source_auth_header_name must be at most {MAX_MUSIC_AUTH_HEADER_NAME_CHARS} characters"
        ));
    }
    HeaderName::from_bytes(value.as_bytes())
        .map(Some)
        .map_err(|e| format!("invalid source_auth_header_name: {e}"))
}

fn clean_music_source_url_field(raw: String) -> Result<Option<String>, String> {
    let value = raw.trim().to_string();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > MAX_MUSIC_SOURCE_URL_CHARS {
        return Err(format!(
            "source_url must be at most {MAX_MUSIC_SOURCE_URL_CHARS} characters"
        ));
    }
    Ok(Some(value))
}

fn redacted_source_url(raw: Option<&String>) -> Option<String> {
    raw.map(|value| redacted_source_url_value(value))
}

fn redacted_source_url_value(value: &str) -> String {
    match reqwest::Url::parse(value) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            if url.query().is_some() {
                url.set_query(Some("redacted=1"));
            }
            url.to_string()
        }
        Err(_) => "<invalid-url>".to_string(),
    }
}

fn sanitize_url_in_error(value: &str, raw_url: &str, redacted_url: &str) -> String {
    if raw_url == redacted_url {
        value.to_string()
    } else {
        value.replace(raw_url, redacted_url)
    }
}

pub(crate) async fn music_sample_seed_render(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Response {
    let mut sample_bytes: Option<Vec<u8>> = None;
    let mut source_url: Option<String> = None;
    let mut source_auth_mode = MusicSourceAuthMode::Public;
    let mut source_auth_header_name: Option<HeaderName> = None;
    let mut source_auth_header: Option<String> = None;
    let mut source_cookie_header: Option<String> = None;
    let mut source_cookies: Option<Vec<u8>> = None;
    let mut prompt = String::new();
    let mut title = "music-sample-seed variation".to_string();
    let mut duration_seconds = 180.0;

    while let Some(field) = match multipart.next_field().await {
        Ok(field) => field,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!("invalid multipart body: {e}"),
            )
        }
    } {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "sample" => match field.bytes().await {
                Ok(bytes) => sample_bytes = Some(bytes.to_vec()),
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read sample upload: {e}"),
                    )
                }
            },
            "source_url" => match field.text().await {
                Ok(text) => match clean_music_source_url_field(text) {
                    Ok(value) => source_url = value,
                    Err(e) => return json_error(StatusCode::BAD_REQUEST, e),
                },
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read source_url: {e}"),
                    )
                }
            },
            "source_auth_mode" | "auth_mode" | "source_access" => match field.text().await {
                Ok(text) => match parse_music_source_auth_mode(&text) {
                    Ok(mode) => source_auth_mode = mode,
                    Err(e) => return json_error(StatusCode::BAD_REQUEST, e),
                },
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read source_auth_mode: {e}"),
                    )
                }
            },
            "source_auth_header" | "auth_header" | "authorization" => match field.text().await {
                Ok(text) => match clean_music_auth_field(text, "source_auth_header") {
                    Ok(value) => source_auth_header = value,
                    Err(e) => return json_error(StatusCode::BAD_REQUEST, e),
                },
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read source_auth_header: {e}"),
                    )
                }
            },
            "source_auth_header_name" | "auth_header_name" | "authorization_header_name" => {
                match field.text().await {
                    Ok(text) => match clean_music_auth_header_name_field(text) {
                        Ok(value) => source_auth_header_name = value,
                        Err(e) => return json_error(StatusCode::BAD_REQUEST, e),
                    },
                    Err(e) => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            format!("failed to read source_auth_header_name: {e}"),
                        )
                    }
                }
            }
            "source_cookie_header" | "cookie_header" => match field.text().await {
                Ok(text) => match clean_music_auth_field(text, "source_cookie_header") {
                    Ok(value) => source_cookie_header = value,
                    Err(e) => return json_error(StatusCode::BAD_REQUEST, e),
                },
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read source_cookie_header: {e}"),
                    )
                }
            },
            "source_cookies" | "auth_cookies" | "cookies" => match field.bytes().await {
                Ok(bytes) if bytes.is_empty() => {}
                Ok(bytes) if bytes.len() <= MAX_MUSIC_COOKIE_BYTES => {
                    source_cookies = Some(bytes.to_vec())
                }
                Ok(bytes) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!(
                            "source_cookies is too large ({} bytes; max {MAX_MUSIC_COOKIE_BYTES})",
                            bytes.len()
                        ),
                    )
                }
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read source_cookies: {e}"),
                    )
                }
            },
            "prompt" => match field.text().await {
                Ok(text) => prompt = text,
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read prompt: {e}"),
                    )
                }
            },
            "title" => match field.text().await {
                Ok(text) if !text.trim().is_empty() => title = text.trim().to_string(),
                Ok(_) => {}
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read title: {e}"),
                    )
                }
            },
            "duration_seconds" => match field.text().await {
                Ok(text) => match text.trim().parse::<f64>() {
                    Ok(value) if (15.0..=240.0).contains(&value) => duration_seconds = value,
                    Ok(_) => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            "duration_seconds must be between 15 and 240".to_string(),
                        )
                    }
                    Err(e) => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            format!("invalid duration_seconds: {e}"),
                        )
                    }
                },
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read duration_seconds: {e}"),
                    )
                }
            },
            _ => {}
        }
    }

    if prompt.chars().count() > MAX_MUSIC_PROMPT_CHARS {
        return json_error(
            StatusCode::BAD_REQUEST,
            format!("prompt must be at most {MAX_MUSIC_PROMPT_CHARS} characters"),
        );
    }
    if title.chars().count() > MAX_MUSIC_TITLE_CHARS {
        return json_error(
            StatusCode::BAD_REQUEST,
            format!("title must be at most {MAX_MUSIC_TITLE_CHARS} characters"),
        );
    }

    let now = now_ms();
    let upload_dir = env::temp_dir().join("dd-des-rs-music-uploads");
    if let Err(e) = fs::create_dir_all(&upload_dir) {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to create upload dir: {e}"),
        );
    }
    let upload_path = upload_dir.join(format!("music-sample-seed-{now}.mp4"));
    let auth_cookie_path = if let Some(bytes) = source_cookies {
        let path = upload_dir.join(format!("music-sample-seed-{now}-cookies.txt"));
        if let Err(e) = fs::write(&path, &bytes) {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to persist source_cookies: {e}"),
            );
        }
        Some(path)
    } else {
        None
    };
    let source_auth = MusicSourceAuth {
        mode: source_auth_mode,
        auth_header_name: source_auth_header_name,
        auth_header: source_auth_header,
        cookie_header: source_cookie_header,
        cookies_file: auth_cookie_path,
    };
    if source_url.is_some()
        && source_auth.mode == MusicSourceAuthMode::Authenticated
        && !source_auth.has_credentials()
    {
        return json_error(
            StatusCode::BAD_REQUEST,
            "authenticated source_url requires an Authorization header, Cookie header, or source_cookies file".to_string(),
        );
    }
    let source_kind = if let Some(sample_bytes) = sample_bytes {
        if sample_bytes.is_empty() {
            if let Some(path) = &source_auth.cookies_file {
                let _ = fs::remove_file(path);
            }
            return json_error(
                StatusCode::BAD_REQUEST,
                "sample upload is empty".to_string(),
            );
        }
        if let Err(e) = fs::write(&upload_path, &sample_bytes) {
            if let Some(path) = &source_auth.cookies_file {
                let _ = fs::remove_file(path);
            }
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to persist upload: {e}"),
            );
        }
        "upload".to_string()
    } else if let Some(url) = &source_url {
        match download_music_source_url(url, &upload_path, &source_auth).await {
            Ok(kind) => kind,
            Err(e) => {
                if let Some(path) = &source_auth.cookies_file {
                    let _ = fs::remove_file(path);
                }
                let _ = fs::remove_file(&upload_path);
                return json_error(StatusCode::BAD_REQUEST, e);
            }
        }
    } else {
        if let Some(path) = &source_auth.cookies_file {
            let _ = fs::remove_file(path);
        }
        return json_error(
            StatusCode::BAD_REQUEST,
            "provide multipart field `sample` or `source_url`".to_string(),
        );
    };
    if let Some(path) = &source_auth.cookies_file {
        let _ = fs::remove_file(path);
    }

    let render_dir = state.out_dir.join("music-production").join("sample-seed");
    if let Err(e) = fs::create_dir_all(&render_dir) {
        let _ = fs::remove_file(&upload_path);
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to create music output dir: {e}"),
        );
    }
    let wav_path = render_dir.join(format!("sample-seed-{now}.wav"));
    let manifest_path = render_dir.join(format!("sample-seed-{now}.json"));
    let wav_url = out_url(&state, &wav_path).unwrap_or_else(|| "out/".to_string());
    let manifest_url = out_url(&state, &manifest_path).unwrap_or_else(|| "out/".to_string());
    let prompt_for_render = prompt.trim().to_string();
    let source_auth_summary = source_auth.summary_json();
    let source_url_for_manifest = redacted_source_url(source_url.as_ref());

    let _guard = state.sim_lock.lock().await;
    let render_result: Result<Value, String> = tokio::task::spawn_blocking(move || {
        let result = (|| {
            let sample = derive_music_sample_seed_from_mp4(&upload_path)
                .map_err(|e| format!("failed to derive music-sample-seed: {e}"))?;
            let prompt_influence = if prompt_for_render.is_empty() {
                None
            } else {
                analyze_music_sample_prompt(&prompt_for_render)
            };
            let spec = song_spec_from_music_sample_seed_with_prompt(
                &sample,
                title,
                duration_seconds,
                if prompt_for_render.is_empty() {
                    None
                } else {
                    Some(prompt_for_render.as_str())
                },
            );
            let render = generate_microtonal_song(spec);
            render
                .audio
                .write_wav16(&wav_path)
                .map_err(|e| format!("failed to write wav: {e}"))?;
            let response = json!({
                "ok": true,
                "wav_url": wav_url,
                "manifest_url": manifest_url,
                "wav_path": wav_path.display().to_string(),
                "sample": {
                    "source_kind": source_kind,
                    "source_url": source_url_for_manifest,
                    "source_auth": source_auth_summary,
                    "source_duration_seconds": sample.duration_seconds,
                    "seed": sample.seed,
                    "byte_entropy": sample.byte_entropy,
                    "suggested_genre": sample.suggested_genre.as_str(),
                    "suggested_bpm": sample.suggested_bpm,
                    "descriptors": sample.descriptors,
                    "source_audio_copied": false
                },
                "prompt": prompt_influence.map(|influence| json!({
                    "chars": influence.prompt_chars,
                    "hash": influence.prompt_hash,
                    "genre": influence.genre.map(|genre| genre.as_str()),
                    "bpm_delta": influence.bpm_delta,
                    "key_bias_delta": influence.key_bias_delta,
                    "meter_bias": influence.meter_bias.map(|(n, d)| format!("{n}/{d}")),
                    "tags": influence.feature_tags
                })),
                "summary": music_summary_json(&render.summary)
            });
            fs::write(
                &manifest_path,
                serde_json::to_string_pretty(&response)
                    .map_err(|e| format!("failed to serialize manifest: {e}"))?,
            )
            .map_err(|e| format!("failed to write manifest: {e}"))?;
            Ok(response)
        })();
        let _ = fs::remove_file(&upload_path);
        result
    })
    .await
    .unwrap_or_else(|e| Err(format!("music render task failed: {e}")));

    match render_result {
        Ok(value) => Json(value).into_response(),
        Err(error) => json_error(StatusCode::BAD_REQUEST, error),
    }
}

fn music_summary_json(summary: &ArrangementSummary) -> Value {
    json!({
        "title": &summary.title,
        "genre": summary.genre.as_str(),
        "duration_seconds": summary.duration_seconds,
        "bpm": summary.bpm,
        "scale": &summary.scale_name,
        "key_changes": summary.key_changes.len(),
        "time_signature_changes": summary.time_signature_changes.len(),
        "pauses": summary.pauses.len(),
        "drum_patterns": summary.drum_variation.pattern_names.len(),
        "drum_fills": summary.drum_variation.fills,
        "drum_micro_variations": summary.drum_variation.micro_variations,
        "drum_variation_ratio": summary.drum_variation.variation_ratio(),
        "percussion_gain": summary.drum_variation.percussion_gain,
        "instruments": &summary.instruments,
        "parts": summary.parts.iter().map(|part| json!({
            "name": &part.name,
            "role": part.role.as_str(),
            "instrument": &part.instrument,
            "events": part.events
        })).collect::<Vec<_>>(),
        "rendered_events": summary.rendered_events,
        "peak": summary.peak,
        "rms": summary.rms,
        "spectral_centroid_hz": summary.spectral_centroid_hz
    })
}

fn out_url(state: &AppState, path: &StdPath) -> Option<String> {
    path.strip_prefix(state.out_dir.as_path())
        .ok()
        .map(|rel| format!("out/{}", rel.to_string_lossy().replace('\\', "/")))
}

async fn download_music_source_url(
    raw: &str,
    path: &StdPath,
    auth: &MusicSourceAuth,
) -> Result<String, String> {
    let url = validate_public_music_url(raw)?;
    validate_public_music_url_dns(&url).await?;
    if prefers_ytdlp(&url) {
        match download_with_ytdlp(url.as_str().to_string(), path.to_path_buf(), auth).await {
            Ok(kind) => return Ok(format!("{kind}; access={}", auth.effective_mode().as_str())),
            Err(ytdlp_error) => match download_direct_media(&url, path, auth).await {
                Ok(kind) => return Ok(format!("{kind}; yt-dlp fallback reason: {ytdlp_error}")),
                Err(direct_error) => {
                    return Err(format!(
                            "could not download public media link. yt-dlp: {ytdlp_error}; direct HTTP: {direct_error}"
                        ));
                }
            },
        }
    }

    match download_direct_media(&url, path, auth).await {
        Ok(kind) => Ok(kind),
        Err(direct_error) => match download_with_ytdlp(
            url.as_str().to_string(),
            path.to_path_buf(),
            auth,
        )
        .await
        {
            Ok(kind) => Ok(format!(
                "{kind}; access={}; direct HTTP fallback reason: {direct_error}",
                auth.effective_mode().as_str()
            )),
            Err(ytdlp_error) => Err(format!(
                "could not download public media link. direct HTTP: {direct_error}; yt-dlp: {ytdlp_error}"
            )),
        },
    }
}

fn validate_public_music_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|e| format!("invalid source_url: {e}"))?;
    validate_public_music_url_parts(&url)?;
    Ok(url)
}

async fn validate_public_music_url_dns(url: &reqwest::Url) -> Result<(), String> {
    let host = url
        .host_str()
        .ok_or_else(|| "source_url must include a public host".to_string())?;
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "source_url must use a URL scheme with a known port".to_string())?;
    let addrs = tokio::net::lookup_host((host, port)).await.map_err(|e| {
        format!(
            "source_url host `{}` could not be resolved: {e}",
            truncate_for_error(host, 120)
        )
    })?;
    validate_music_resolved_addrs(host, addrs.map(|addr| addr.ip()))
}

fn validate_music_resolved_addrs<I>(host: &str, addrs: I) -> Result<(), String>
where
    I: IntoIterator<Item = IpAddr>,
{
    let mut saw_addr = false;
    for ip in addrs {
        saw_addr = true;
        if is_blocked_music_ip(ip) {
            return Err(format!(
                "source_url host `{}` resolves to localhost/private network",
                truncate_for_error(host, 120)
            ));
        }
    }
    if !saw_addr {
        return Err(format!(
            "source_url host `{}` resolved to no addresses",
            truncate_for_error(host, 120)
        ));
    }
    Ok(())
}

fn validate_public_music_url_parts(url: &reqwest::Url) -> Result<(), String> {
    match url.scheme() {
        "http" | "https" => {}
        _ => return Err("source_url must use http or https".to_string()),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "source_url must not embed credentials; use the dedicated auth fields".to_string(),
        );
    }
    let host = url
        .host_str()
        .ok_or_else(|| "source_url must include a public host".to_string())?
        .to_ascii_lowercase();
    if is_blocked_music_host(&host) {
        return Err(
            "source_url must point to a public resource, not localhost/private network".to_string(),
        );
    }
    Ok(())
}

fn is_blocked_music_host(host: &str) -> bool {
    let normalized = host.trim_matches(['[', ']']);
    if normalized == "localhost"
        || normalized.ends_with(".localhost")
        || normalized.ends_with(".local")
        || normalized.ends_with(".internal")
        || normalized == "metadata.google.internal"
    {
        return true;
    }
    normalized
        .parse::<IpAddr>()
        .map(is_blocked_music_ip)
        .unwrap_or(false)
}

fn is_blocked_music_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => {
            let octets = addr.octets();
            addr.is_loopback()
                || addr.is_private()
                || addr.is_link_local()
                || addr.is_broadcast()
                || addr.is_unspecified()
                || addr.is_multicast()
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        }
        IpAddr::V6(addr) => {
            let segments = addr.segments();
            addr.is_loopback()
                || addr.is_unspecified()
                || addr.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
        }
    }
}

fn music_redirect_policy(
    source_url: &reqwest::Url,
    authenticated: bool,
) -> reqwest::redirect::Policy {
    let source_host = source_url.host_str().map(|host| host.to_ascii_lowercase());
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 8 {
            return attempt.error("too many redirects");
        }
        if validate_public_music_url_parts(attempt.url()).is_err() {
            return attempt.error("redirect target is not a public http/https URL");
        }
        if authenticated {
            let next_host = attempt
                .url()
                .host_str()
                .map(|host| host.to_ascii_lowercase());
            if next_host != source_host {
                return attempt
                    .error("authenticated source_url redirects must stay on the original host");
            }
        }
        attempt.follow()
    })
}

fn prefers_ytdlp(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let social_host = [
        "youtube.com",
        "youtu.be",
        "facebook.com",
        "fb.watch",
        "instagram.com",
        "x.com",
        "twitter.com",
        "tiktok.com",
        "soundcloud.com",
        "vimeo.com",
    ]
    .iter()
    .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));
    social_host || !looks_like_direct_media_url(url)
}

fn looks_like_direct_media_url(url: &reqwest::Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    [
        ".mp4", ".m4v", ".mov", ".webm", ".mkv", ".mp3", ".m4a", ".wav", ".aac", ".ogg",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
}

async fn download_direct_media(
    url: &reqwest::Url,
    path: &StdPath,
    auth: &MusicSourceAuth,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(MUSIC_DOWNLOAD_TIMEOUT_SECS))
        .user_agent("dd-des-rs-music-sample-seed/0.1")
        .redirect(music_redirect_policy(
            url,
            auth.effective_mode() == MusicSourceAuthMode::Authenticated,
        ))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;
    let mut request = client.get(url.clone());
    if let Some(value) = &auth.auth_header {
        let header_value =
            HeaderValue::from_str(value).map_err(|e| format!("invalid source_auth_header: {e}"))?;
        let header_name = auth
            .auth_header_name
            .clone()
            .unwrap_or(header::AUTHORIZATION);
        request = request.header(header_name, header_value);
    }
    if let Some(value) = &auth.cookie_header {
        let header_value = HeaderValue::from_str(value)
            .map_err(|e| format!("invalid source_cookie_header: {e}"))?;
        request = request.header(header::COOKIE, header_value);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("GET failed: {}", e.without_url()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("GET returned HTTP {status}"));
    }
    if let Some(len) = response.content_length() {
        if len > MAX_MUSIC_UPLOAD_BYTES as u64 {
            return Err(format!(
                "resource is too large ({len} bytes; max {MAX_MUSIC_UPLOAD_BYTES})"
            ));
        }
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !looks_like_direct_media_url(url)
        && !content_type.starts_with("video/")
        && !content_type.starts_with("audio/")
        && !content_type.contains("octet-stream")
    {
        return Err(format!(
            "direct HTTP resource is not advertised as audio/video (content-type {content_type:?})"
        ));
    }
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| format!("failed to create downloaded media file: {e}"))?;
    let mut response = response;
    let mut downloaded = 0usize;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("failed to read body: {}", e.without_url()))?
    {
        downloaded = downloaded
            .checked_add(chunk.len())
            .ok_or_else(|| "downloaded media size overflowed".to_string())?;
        if downloaded > MAX_MUSIC_UPLOAD_BYTES {
            let _ = tokio::fs::remove_file(path).await;
            return Err(format!(
                "resource is too large ({downloaded} bytes; max {MAX_MUSIC_UPLOAD_BYTES})"
            ));
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("failed to write downloaded media: {e}"))?;
    }
    file.flush()
        .await
        .map_err(|e| format!("failed to flush downloaded media: {e}"))?;
    Ok(format!(
        "direct-http; access={}",
        auth.effective_mode().as_str()
    ))
}

async fn download_with_ytdlp(
    url: String,
    path: PathBuf,
    auth: &MusicSourceAuth,
) -> Result<String, String> {
    let cookies_file = auth.cookies_file.clone();
    let auth_header_name = auth.auth_header_name.clone();
    let auth_header = auth.auth_header.clone();
    tokio::task::spawn_blocking(move || {
        run_ytdlp_download(
            &url,
            &path,
            cookies_file.as_deref(),
            auth_header_name.as_ref(),
            auth_header.as_deref(),
        )
    })
    .await
    .unwrap_or_else(|e| Err(format!("yt-dlp task failed: {e}")))
}

fn run_ytdlp_download(
    url: &str,
    path: &StdPath,
    cookies_file: Option<&StdPath>,
    auth_header_name: Option<&HeaderName>,
    auth_header: Option<&str>,
) -> Result<String, String> {
    let mut attempts = Vec::new();
    if let Ok(bin) = env::var("DES_YTDLP_BIN") {
        if !bin.trim().is_empty() {
            attempts.push(YtDlpCommand::Binary(bin));
        }
    }
    attempts.push(YtDlpCommand::Binary("yt-dlp".to_string()));
    attempts.push(YtDlpCommand::Binary("youtube-dl".to_string()));
    attempts.push(YtDlpCommand::PythonModule);

    let mut args = vec![
        "--no-playlist".to_string(),
        "--force-overwrites".to_string(),
        "--max-filesize".to_string(),
        format!("{}m", (MAX_MUSIC_UPLOAD_BYTES / (1024 * 1024)).max(1)),
        "--merge-output-format".to_string(),
        "mp4".to_string(),
        "--remux-video".to_string(),
        "mp4".to_string(),
        "-f".to_string(),
        "b[ext=mp4]/bv*[ext=mp4]+ba[ext=m4a]/best".to_string(),
        "-o".to_string(),
        path.display().to_string(),
    ];
    if let Some(cookies_file) = cookies_file {
        args.extend(["--cookies".to_string(), cookies_file.display().to_string()]);
    }
    if let Some(value) = auth_header {
        let name = auth_header_name
            .map(|name| name.as_str())
            .unwrap_or(header::AUTHORIZATION.as_str());
        args.extend(["--add-header".to_string(), format!("{name}: {value}")]);
    }
    args.push(url.to_string());

    let mut errors = Vec::new();
    let redacted_url = redacted_source_url_value(url);
    for attempt in attempts {
        match run_ytdlp_attempt(&attempt, &args) {
            Ok(()) => {
                if path.exists() {
                    return Ok(match attempt {
                        YtDlpCommand::Binary(name) => format!("yt-dlp:{name}"),
                        YtDlpCommand::PythonModule => "yt-dlp:python3 -m yt_dlp".to_string(),
                    });
                }
                errors.push(format!(
                    "{} exited successfully but did not create {}",
                    attempt.label(),
                    path.display()
                ));
            }
            Err(e) => errors.push(format!(
                "{}: {}",
                attempt.label(),
                sanitize_url_in_error(&e, url, &redacted_url)
            )),
        }
    }
    Err(errors.join("; "))
}

enum YtDlpCommand {
    Binary(String),
    PythonModule,
}

impl YtDlpCommand {
    fn label(&self) -> String {
        match self {
            YtDlpCommand::Binary(name) => name.clone(),
            YtDlpCommand::PythonModule => "python3 -m yt_dlp".to_string(),
        }
    }
}

fn run_ytdlp_attempt(command: &YtDlpCommand, args: &[String]) -> Result<(), String> {
    let mut cmd = match command {
        YtDlpCommand::Binary(name) => Command::new(name),
        YtDlpCommand::PythonModule => {
            let mut cmd = Command::new("python3");
            cmd.arg("-m").arg("yt_dlp");
            cmd
        }
    };
    let mut child = cmd
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|e| format!("failed to collect output: {e}"))?;
                if output.status.success() {
                    return Ok(());
                }
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                return Err(format!(
                    "exit {}: {}{}",
                    output.status,
                    truncate_for_error(stderr.trim(), 700),
                    if stdout.trim().is_empty() {
                        "".to_string()
                    } else {
                        format!("; stdout: {}", truncate_for_error(stdout.trim(), 300))
                    }
                ));
            }
            Ok(None) => {
                if started.elapsed() > Duration::from_secs(MUSIC_DOWNLOAD_TIMEOUT_SECS) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("timed out after {MUSIC_DOWNLOAD_TIMEOUT_SECS}s"));
                }
                thread::sleep(Duration::from_millis(250));
            }
            Err(e) => return Err(format!("wait failed: {e}")),
        }
    }
}

fn truncate_for_error(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in value.chars().enumerate() {
        if i >= max_chars {
            out.push_str("...");
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn music_source_url_validation_blocks_private_and_secret_bearing_urls() {
        for raw in [
            "ftp://example.com/seed.mp4",
            "http://localhost/seed.mp4",
            "http://127.1.2.3/seed.mp4",
            "http://10.1.2.3/seed.mp4",
            "http://172.20.1.2/seed.mp4",
            "http://192.168.0.2/seed.mp4",
            "http://169.254.169.254/latest/meta-data",
            "http://100.64.0.1/seed.mp4",
            "http://[::1]/seed.mp4",
            "https://user:pass@example.com/seed.mp4",
            "https://example.local/seed.mp4",
            "https://metadata.google.internal/computeMetadata/v1/",
        ] {
            assert!(
                validate_public_music_url(raw).is_err(),
                "expected {raw} to be rejected"
            );
        }

        assert!(validate_public_music_url("https://example.com/path/seed.mp4").is_ok());
    }

    #[test]
    fn music_source_dns_validation_blocks_private_resolutions() {
        assert!(validate_music_resolved_addrs(
            "media.example",
            ["93.184.216.34".parse::<IpAddr>().unwrap()]
        )
        .is_ok());
        assert!(validate_music_resolved_addrs(
            "media.example",
            [
                "93.184.216.34".parse::<IpAddr>().unwrap(),
                "10.1.2.3".parse::<IpAddr>().unwrap()
            ]
        )
        .is_err());
        assert!(
            validate_music_resolved_addrs("media.example", std::iter::empty::<IpAddr>()).is_err()
        );
    }

    #[test]
    fn music_source_url_redaction_removes_credentials_and_query() {
        assert_eq!(
            redacted_source_url_value("https://user:pass@example.com/watch?v=secret"),
            "https://example.com/watch?redacted=1"
        );
        assert_eq!(
            sanitize_url_in_error(
                "failed for https://example.com/watch?v=secret",
                "https://example.com/watch?v=secret",
                "https://example.com/watch?redacted=1"
            ),
            "failed for https://example.com/watch?redacted=1"
        );
    }

    #[test]
    fn music_auth_header_name_validation_accepts_auth_and_rejects_bad_names() {
        let header = clean_music_auth_header_name_field(" Auth ".to_string())
            .unwrap()
            .unwrap();
        assert_eq!(header.as_str(), "auth");
        assert!(clean_music_auth_header_name_field("Bad Header".to_string()).is_err());
        assert!(clean_music_auth_header_name_field("Auth:\nsecret".to_string()).is_err());
    }
}
