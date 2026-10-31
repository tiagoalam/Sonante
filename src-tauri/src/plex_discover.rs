//! Read-only music discovery. Only constructed local PMS paths are followed;
//! server-provided hub/metadata keys are display data, never request targets.
use super::*;
use serde_json::Value;
use std::collections::HashSet;

const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_HUBS: usize = 8;
const MAX_ITEMS: usize = 12;
const MAX_CHILDREN: usize = 24;
const MAX_HERO_CANDIDATES: usize = 20;
const MAX_HERO_WINDOW: usize = 140;

#[derive(Debug, Clone, Serialize)]
pub struct PlexMixTrack {
    pub server_id: String,
    pub library_id: String,
    // Optional navigation only; playback and persisted playlists keep PlexTrack.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_rating_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_rating_key: Option<String>,
    #[serde(flatten)]
    pub track: PlexTrack,
}
#[derive(Debug, Clone, Serialize)]
pub struct PlexLibraryMixPage {
    pub server_id: String,
    pub library_id: String,
    pub language: String,
    pub tracks: Vec<PlexMixTrack>,
    pub partial: bool,
    pub has_more: bool,
}

// PMS API Info: track=10, sort keys include random, limit bounds the returned
// metadata type. Get items in the section is read-only. No fixed-window shuffle.
fn mix_path(section: &str, limit: usize) -> Result<String, String> {
    if !valid_id(section) || !(1..=100).contains(&limit) {
        return Err(invalid());
    }
    Ok(format!("/library/sections/{section}/all?type=10&sort=random&limit={limit}&X-Plex-Container-Start=0&X-Plex-Container-Size={limit}"))
}

// Annual choices are server-advertised album metadata, never addedAt.
const YEAR_PAGE: usize = 64;
const YEAR_PAGES: usize = 4;
#[derive(Debug, Clone, Serialize)]
pub struct PlexMixYears {
    pub server_id: String,
    pub library_id: String,
    pub language: String,
    pub years: Vec<u32>,
    pub partial: bool,
}
fn decade_path(section: &str, limit: usize, decade: u32) -> Result<String, String> {
    if !(1000..=9990).contains(&decade) || decade % 10 != 0 {
        return Err(invalid());
    }
    // PMS Media Queries: qualified ancestor fields and inclusive integer operators.
    Ok(format!(
        "{}&album.year%3E={decade}&album.year%3C={}",
        mix_path(section, limit)?,
        decade + 9
    ))
}
fn value_objects(value: Option<&Value>) -> Vec<&Value> {
    match value {
        Some(Value::Array(a)) => a.iter().take(256).collect(),
        Some(v @ Value::Object(_)) => vec![v],
        _ => vec![],
    }
}
fn album_year_field(c: &Value) -> bool {
    value_objects(c.get("Meta"))
        .into_iter()
        .flat_map(|m| value_objects(m.get("Type")))
        .chain(value_objects(c.get("Type")))
        .chain(value_objects(c.get("Directory")))
        .any(|owner| {
            (owner["type"].as_str() == Some("album") || item_number(owner.get("type")) == Some(9))
                && value_objects(owner.get("Field"))
                    .into_iter()
                    .any(|f| f["key"] == "album.year" && f["type"] == "integer")
        })
}
fn item_number(value: Option<&Value>) -> Option<u64> {
    value.and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
}
fn year_reference(c: &Value, section: &str, base: &str) -> Option<String> {
    let roots: Vec<&Value> = value_objects(c.get("Meta"))
        .into_iter()
        .flat_map(|m| value_objects(m.get("Type")))
        .chain(value_objects(c.get("Type")))
        .chain(value_objects(c.get("Directory")))
        .chain(std::iter::once(c))
        .collect();
    let filters = roots
        .iter()
        .filter(|r| {
            !matches!(item_number(r.get("type")), Some(8 | 10))
                && !matches!(r["type"].as_str(), Some("artist" | "track"))
        })
        .flat_map(|r| value_objects(r.get("Filter")))
        .chain(roots.iter().copied().filter(|r| r.get("filter").is_some()));
    for f in filters {
        if f["filter"] != "year" || f["filterType"] != "integer" {
            continue;
        }
        let Some(key) = f["key"].as_str() else {
            continue;
        };
        if key.contains(['%', '#', '\\']) || key.chars().any(char::is_whitespace) {
            continue;
        }
        let Ok(origin) = reqwest::Url::parse(base) else {
            continue;
        };
        let Ok(parent) = origin.join(&format!("/library/sections/{section}/")) else {
            continue;
        };
        let Ok(target) = parent.join(key) else {
            continue;
        };
        let pairs: Vec<_> = target.query_pairs().collect();
        if target.origin() != origin.origin()
            || !target.username().is_empty()
            || target.password().is_some()
            || target.path() != format!("/library/sections/{section}/year")
            || pairs.len() != 1
            || pairs[0].0 != "type"
            || pairs[0].1 != "9"
        {
            continue;
        }
        return Some(format!("{}?type=9", target.path()));
    }
    None
}
fn parse_year_page(
    json: &Value,
    server: &str,
    section: &str,
    offset: usize,
) -> Result<(Vec<u32>, bool, bool, usize), String> {
    let c = container(json, server, section)?;
    if item_number(c.get("offset")).is_some_and(|n| n != offset as u64) {
        return Err(invalid());
    }
    let records = match c.get("Directory") {
        Some(Value::Array(a)) if a.len() <= YEAR_PAGE => a.as_slice(),
        None if c["size"].as_u64() == Some(0) => &[],
        _ => return Err(invalid()),
    };
    let mut years = vec![];
    for r in records {
        if let Some(n) = item_number(r.get("key")).filter(|n| (1000..=9999).contains(n)) {
            years.push(n as u32);
        }
    }
    let partial = years.len() != records.len();
    let more = match item_number(c.get("totalSize")) {
        Some(total) => total > (offset + records.len()) as u64,
        None => !records.is_empty(),
    };
    Ok((years, more, partial, records.len()))
}

#[derive(Default)]
struct YearWindow {
    years: std::collections::BTreeSet<u32>,
    offset: usize,
    pages: usize,
    partial: bool,
}
impl YearWindow {
    fn accept(
        &mut self,
        result: Result<(Vec<u32>, bool, bool, usize), String>,
    ) -> Result<bool, String> {
        self.pages += 1;
        let (values, more, rejected, observed) = match result {
            Ok(value) => value,
            Err(code) if self.years.is_empty() => return Err(code),
            Err(_) => {
                self.partial = true;
                return Ok(false);
            }
        };
        let before = self.years.len();
        self.years.extend(values);
        self.offset += observed;
        self.partial |= rejected;
        if !more {
            return Ok(false);
        }
        if self.years.len() == before || self.pages == YEAR_PAGES {
            self.partial = true;
            return Ok(false);
        }
        Ok(true)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PlexStyleChoice {
    pub id: String,
    pub title: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct PlexMixStyles {
    pub server_id: String,
    pub library_id: String,
    pub language: String,
    pub field: String,
    pub choices: Vec<PlexStyleChoice>,
    pub partial: bool,
}
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PlexStyleSelection {
    pub field: String,
    pub ids: Vec<String>,
}
fn style_path(
    section: &str,
    limit: usize,
    selection: &PlexStyleSelection,
) -> Result<String, String> {
    if !["artist.style", "album.style", "track.style"].contains(&selection.field.as_str())
        || selection.ids.is_empty()
        || selection.ids.len() > YEAR_PAGE * YEAR_PAGES
        || selection.ids.iter().any(|id| !valid_id(id))
    {
        return Err(invalid());
    }
    Ok(format!(
        "{}&{}={}",
        mix_path(section, limit)?,
        selection.field,
        selection.ids.join(",")
    ))
}
fn style_types(c: &Value) -> Vec<&Value> {
    value_objects(c.get("Meta"))
        .into_iter()
        .flat_map(|m| value_objects(m.get("Type")))
        .chain(value_objects(c.get("Type")))
        .chain(value_objects(c.get("Directory")))
        .collect()
}
fn style_fields(c: &Value) -> Vec<(&'static str, u64)> {
    // Scope comes from an explicit qualified Field on its musical Type, never genre.
    [("artist", 8), ("album", 9), ("track", 10)]
        .into_iter()
        .filter_map(|(level, typ)| {
            let field = match level {
                "artist" => "artist.style",
                "album" => "album.style",
                _ => "track.style",
            };
            style_types(c)
                .into_iter()
                .any(|owner| {
                    (item_number(owner.get("type")) == Some(typ) || owner["type"] == level)
                        && value_objects(owner.get("Field")).into_iter().any(|f| {
                            f["key"] == field
                                && matches!(f["type"].as_str(), Some("tag" | "integer"))
                        })
                })
                .then_some((field, typ))
        })
        .collect()
}
fn style_reference(c: &Value, section: &str, base: &str, field: &str, typ: u64) -> Option<String> {
    let roots = style_types(c);
    let filters = roots
        .iter()
        .filter(|owner| {
            item_number(owner.get("type")) == Some(typ)
                || owner["type"].as_str() == field.split('.').next()
        })
        .flat_map(|owner| value_objects(owner.get("Filter")))
        .chain(roots.iter().copied().filter(|f| f.get("filter").is_some()))
        .chain(value_objects(c.get("Filter")));
    for f in filters {
        if !matches!(f["filterType"].as_str(), Some("string" | "integer"))
            || !(f["filter"] == "style" || f["filter"] == field)
        {
            continue;
        }
        let Some(key) = f["key"].as_str() else {
            continue;
        };
        if key.contains(['%', '#', '\\'])
            || key.chars().any(|ch| ch.is_whitespace() || ch.is_control())
        {
            continue;
        }
        let origin = reqwest::Url::parse(base).ok()?;
        let parent = origin.join(&format!("/library/sections/{section}/")).ok()?;
        let Ok(target) = parent.join(key) else {
            continue;
        };
        let pairs: Vec<_> = target.query_pairs().collect();
        if target.origin() != origin.origin()
            || !target.username().is_empty()
            || target.password().is_some()
            || ![
                format!("/library/sections/{section}/style"),
                format!("/library/sections/{section}/{field}"),
            ]
            .contains(&target.path().to_string())
            || pairs.len() != 1
            || pairs[0].0 != "type"
            || pairs[0].1 != typ.to_string()
        {
            continue;
        }
        return Some(format!("{}?type={typ}", target.path()));
    }
    None
}
fn parse_style_page(
    json: &Value,
    server: &str,
    section: &str,
    offset: usize,
    tokens: &[&str],
) -> Result<(Vec<PlexStyleChoice>, bool, bool, usize), String> {
    let c = container(json, server, section)?;
    if item_number(c.get("offset")).is_some_and(|n| n != offset as u64) {
        return Err(invalid());
    }
    let records = match c.get("Directory") {
        Some(Value::Array(a)) if a.len() <= YEAR_PAGE => a.as_slice(),
        None if c["size"].as_u64() == Some(0) => &[],
        _ => return Err(invalid()),
    };
    let choices: Vec<_> = records
        .iter()
        .filter_map(|r| {
            let id = r["key"].as_str().filter(|id| valid_id(id))?.to_string();
            Some(PlexStyleChoice {
                id,
                title: text(r, "title", tokens)?,
            })
        })
        .collect();
    let partial = choices.len() != records.len();
    let more = item_number(c.get("totalSize")).map_or(!records.is_empty(), |total| {
        total > (offset + records.len()) as u64
    });
    Ok((choices, more, partial, records.len()))
}
#[derive(Default)]
struct StyleWindow {
    choices: std::collections::BTreeMap<String, PlexStyleChoice>,
    offset: usize,
    pages: usize,
    partial: bool,
}
impl StyleWindow {
    fn accept(
        &mut self,
        result: Result<(Vec<PlexStyleChoice>, bool, bool, usize), String>,
    ) -> Result<bool, String> {
        self.pages += 1;
        let (values, more, partial, observed) = match result {
            Ok(value) => value,
            Err(code) if self.choices.is_empty() => return Err(code),
            Err(_) => {
                self.partial = true;
                return Ok(false);
            }
        };
        let before = self.choices.len();
        for choice in values {
            if let Some(old) = self.choices.get(&choice.id) {
                self.partial |= old.title != choice.title;
            } else {
                self.choices.insert(choice.id.clone(), choice);
            }
        }
        self.offset += observed;
        self.partial |= partial;
        if !more {
            return Ok(false);
        }
        if self.choices.len() == before || self.pages == YEAR_PAGES {
            self.partial = true;
            return Ok(false);
        }
        Ok(true)
    }
}

#[derive(Debug, Default, Serialize)]
struct MixParseCounts {
    observed: usize,
    accepted: usize,
    container_library_match: Option<bool>,
    truncated: bool,
    rejected: std::collections::BTreeMap<&'static str, usize>,
}
impl MixParseCounts {
    fn reject(&mut self, reason: &'static str) {
        *self.rejected.entry(reason).or_default() += 1;
    }
}
// Inspect structure only after rejection. This does not change admission or
// expose any field values. Every reason is a fixed, local enum-like label.
fn mix_rejection_reason(
    item: &Value,
    section: &str,
    tokens: &[&str],
    inherited_section: bool,
) -> &'static str {
    if item.get("librarySectionID").is_none() {
        if !inherited_section {
            return "library_missing";
        }
    } else {
        match item_section(item) {
            None => return "library_invalid",
            Some(id) if !valid_id(&id) => return "library_invalid",
            Some(id) if id != section => return "foreign_library",
            _ => {}
        }
    }
    if item["type"].as_str() != Some("track") {
        return "type_missing_or_not_track";
    }
    let safe_id = |key| {
        item[key].as_str().is_some_and(|id| {
            valid_id(id)
                && !tokens
                    .iter()
                    .any(|token| !token.is_empty() && id.contains(token))
        })
    };
    if !safe_id("ratingKey") {
        return "track_id_missing_or_invalid";
    }
    if text(item, "title", tokens).is_none() {
        return "title_missing_or_unsafe";
    }
    if !safe_id("parentRatingKey") {
        return "album_id_missing_or_invalid";
    }
    if text(item, "parentTitle", tokens).is_none() {
        return "album_title_missing_or_unsafe";
    }
    let Some(media) = item["Media"].as_array().and_then(|list| list.first()) else {
        return "media_missing";
    };
    let Some(part) = media["Part"].as_array().and_then(|list| list.first()) else {
        return "part_missing";
    };
    if part["key"].as_str().is_none_or(str::is_empty) {
        return "part_key_missing";
    }
    "parser_rejected"
}
fn mix_diagnostic(
    failure: Option<&PlexDiscoverFailure>,
    status: Option<u16>,
    counts: &MixParseCounts,
) -> Value {
    serde_json::json!({"stage":failure.map(|f|f.stage).unwrap_or("parse"),"category":failure.map(|f|f.category).unwrap_or("success"),"status":failure.and_then(|f|f.status).or(status),"counts":counts})
}
fn log_mix_diagnostic(
    failure: Option<&PlexDiscoverFailure>,
    status: Option<u16>,
    counts: &MixParseCounts,
) {
    // One aggregate per normal operation, never a per-record log or extra read.
    // Debug-only and automatic: no restart with diagnostic flags is needed.
    if cfg!(debug_assertions) {
        eprintln!(
            "[Sonante] plex.mix.diagnostic {}",
            mix_diagnostic(failure, status, counts)
        );
    }
}
#[cfg(test)]
fn parse_mix(
    client: &PlexClient,
    json: &Value,
    connection: &ResolvedPlexConnection,
    section: &str,
    language: &str,
    limit: usize,
    tokens: &[&str],
) -> Result<PlexLibraryMixPage, String> {
    parse_mix_observed(
        client,
        json,
        connection,
        section,
        language,
        limit,
        tokens,
        &mut MixParseCounts::default(),
    )
}
fn parse_mix_observed(
    client: &PlexClient,
    json: &Value,
    connection: &ResolvedPlexConnection,
    section: &str,
    language: &str,
    limit: usize,
    tokens: &[&str],
    counts: &mut MixParseCounts,
) -> Result<PlexLibraryMixPage, String> {
    mix_path(section, limit)?;
    let c = container(json, &connection.server_id, section).map_err(|code| {
        counts.reject("container_rejected");
        code
    })?;
    counts.container_library_match = item_section(c).map(|id| id == section);
    let records = match c.get("Metadata") {
        Some(Value::Array(records)) => records.as_slice(),
        None if c["size"].as_u64() == Some(0) => &[],
        _ => {
            counts.reject("metadata_missing_or_invalid");
            return Err(invalid());
        }
    };
    let mut tracks = vec![];
    let mut seen = HashSet::new();
    let mut rejected = records.len() > limit;
    counts.observed = records.len().min(limit);
    counts.truncated = records.len() > limit;
    for record in records.iter().take(limit) {
        // get_library_mix already validates the selected server/generation via
        // discover_json and discover_tokens. Only a genuinely absent per-track
        // field may inherit this explicitly matched container; null/invalid or
        // conflicting fields remain subject to parse_item's existing check.
        let inherited_section = record.get("librarySectionID").is_none()
            && counts.container_library_match == Some(true);
        let safe = parse_item(record, connection, section, !inherited_section, tokens)
            .filter(|item| item.kind == "track");
        let parsed = safe.as_ref().and_then(|_| client.parse_track_item(connection, record));
        let Some(mut track) = parsed else {
            counts.reject(mix_rejection_reason(
                record,
                section,
                tokens,
                inherited_section,
            ));
            rejected = true;
            continue;
        };
        let MediaLocator::Plex {
            part_key,
            file_path,
            ..
        } = &mut track.media_locator
        else {
            counts.reject("locator_kind");
            rejected = true;
            continue;
        };
        let Ok(decoded) = urlencoding::decode(part_key) else {
            counts.reject("part_key_encoding");
            rejected = true;
            continue;
        };
        if !is_valid_plex_path(part_key)
            || !part_key.starts_with("/library/parts/")
            || decoded.contains(['?', '#', '\\'])
            || decoded.chars().any(char::is_control)
            || decoded.split('/').any(|part| matches!(part, "." | ".."))
            || tokens
                .iter()
                .any(|token| !token.is_empty() && decoded.contains(token))
        {
            counts.reject("unsafe_part_key");
            rejected = true;
            continue;
        }
        // Display strings use the same credential-safe readers as Discover.
        track.title = text(record, "title", tokens).ok_or_else(invalid)?;
        track.album_title = text(record, "parentTitle", tokens);
        track.artist = text(record, "grandparentTitle", tokens);
        track.thumb = image(record, "thumb", connection, tokens)
            .or_else(|| image(record, "parentThumb", connection, tokens));
        if file_path.as_ref().is_some_and(|path| {
            !path.starts_with('/')
                || path.contains("://")
                || tokens
                    .iter()
                    .any(|token| !token.is_empty() && path.contains(token))
        }) {
            *file_path = None;
        }
        if !seen.insert(track.rating_key.clone()) {
            counts.reject("duplicate");
            rejected = true;
            continue;
        }
        let album = safe.as_ref().and_then(|item| item.album.as_ref());
        // The flattened mix DTO owns the separately validated navigation key.
        // Do not serialize a duplicate field from the generic track parser.
        track.album_rating_key = None;
        tracks.push(PlexMixTrack {
            server_id: connection.server_id.clone(),
            library_id: section.into(),
            album_rating_key: album.map(|album| album.rating_key.clone()),
            artist_rating_key: album.and_then(|album| album.artist_rating_key.clone()),
            track,
        });
        counts.accepted += 1;
    }
    if tracks.is_empty() && !records.is_empty() {
        return Err(invalid());
    }
    Ok(PlexLibraryMixPage {
        server_id: connection.server_id.clone(),
        library_id: section.into(),
        language: language_tag(language).map_err(|e| e.public_code())?.into(),
        tracks,
        partial: rejected,
        has_more: c["totalSize"]
            .as_u64()
            .map(|n| n > records.len() as u64)
            .unwrap_or(records.len() >= limit),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct PlexDiscoverItem {
    pub server_id: String,
    pub library_id: String,
    pub kind: String,
    pub rating_key: String,
    pub title: String,
    pub subtitle: String,
    pub thumb: Option<PlexImageRef>,
    pub album: Option<PlexAlbum>,
    pub added_at: Option<u64>,
    pub last_viewed_at: Option<u64>,
}
#[derive(Debug, Clone, Serialize)]
pub struct PlexDiscoverHub {
    pub artist_recommendation: bool,
    pub artist_candidates: Vec<PlexDiscoverItem>,
    pub context: Option<String>,
    pub recent_family: Option<&'static str>,
    pub id: String,
    pub title: String,
    pub items: Vec<PlexDiscoverItem>,
    pub partial: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct PlexDiscoverPage {
    pub language: String,
    pub server_id: String,
    pub library_id: String,
    pub hubs: Vec<PlexDiscoverHub>,
    pub partial: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct PlexDiscoverRecent {
    pub language: String,
    pub server_id: String,
    pub library_id: String,
    pub items: Vec<PlexDiscoverItem>,
    pub partial: bool,
    pub dates_missing: bool,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlexDiscoverFailure {
    pub stage: &'static str,
    pub category: &'static str,
    pub status: Option<u16>,
}
impl PlexDiscoverFailure {
    pub fn new(stage: &'static str, category: &'static str) -> Self {
        Self {
            stage,
            category,
            status: None,
        }
    }
    fn from_error(stage: &'static str, error: PlexError) -> Self {
        let (category, status) = match error.kind {
            PlexErrorKind::Unauthorized => ("authentication", Some(401)),
            PlexErrorKind::HttpError(403) => ("permission", Some(403)),
            PlexErrorKind::NotFound => ("http", Some(404)),
            PlexErrorKind::HttpError(405) | PlexErrorKind::ServerError(501) => (
                "unsupported",
                Some(if error.kind == PlexErrorKind::HttpError(405) {
                    405
                } else {
                    501
                }),
            ),
            PlexErrorKind::HttpError(status) | PlexErrorKind::ServerError(status) => {
                ("http", Some(status))
            }
            PlexErrorKind::Timeout => ("timeout", None),
            PlexErrorKind::Transport => ("transport", None),
            PlexErrorKind::IdentityMismatch => ("identity", None),
            PlexErrorKind::ConfigurationChanged
            | PlexErrorKind::ServerSelectionRequired
            | PlexErrorKind::ServerNotFound
            | PlexErrorKind::StateUnavailable => ("context", None),
            PlexErrorKind::InvalidResponse => ("response", None),
            _ => ("unknown", None),
        };
        Self {
            stage,
            category,
            status,
        }
    }
    fn with_status(mut self, status: Option<u16>) -> Self {
        if status.is_some() {
            self.status = status;
        }
        self
    }
    fn public_code(&self) -> String {
        match self.category {
            "authentication" | "permission" => "PLEX_DISCOVER_AUTH",
            "identity" | "membership" | "context" => "PLEX_DISCOVER_CONTEXT",
            "response" => "PLEX_DISCOVER_INVALID",
            _ => "PLEX_DISCOVER_UNAVAILABLE",
        }
        .into()
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct PlexDiscoverArtistContent {
    pub server_id: String,
    pub library_id: String,
    pub artist_id: String,
    pub albums: Vec<PlexAlbum>,
    pub top_tracks: Vec<PlexTrack>,
}
fn validate_artist_parent(
    json: &Value,
    server: &str,
    section: &str,
    id: &str,
) -> Result<(), String> {
    let parent = container(json, server, section)?["Metadata"]
        .as_array()
        .filter(|items| items.len() == 1)
        .and_then(|items| items.first())
        .ok_or_else(invalid)?;
    if parent["ratingKey"].as_str() != Some(id)
        || parent["type"].as_str() != Some("artist")
        || item_section(parent).as_deref() != Some(section)
    {
        return Err("PLEX_DISCOVER_CONTEXT".into());
    }
    Ok(())
}
fn language_tag(value: &str) -> Result<&'static str, PlexDiscoverFailure> {
    match value {
        "pt-BR" => Ok("pt-BR"),
        "en" => Ok("en"),
        _ => Err(PlexDiscoverFailure::new("request", "context")),
    }
}
fn discover_http() -> Result<&'static reqwest::Client, String> {
    static CLIENT: std::sync::OnceLock<Result<reqwest::Client, String>> =
        std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(12))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "PLEX_DISCOVER_UNAVAILABLE".into())
        })
        .as_ref()
        .map_err(Clone::clone)
}
fn discover_request(
    http: &reqwest::Client,
    connection: &ResolvedPlexConnection,
    path: &str,
    language: &str,
) -> Result<RequestBuilder, PlexDiscoverFailure> {
    let language = language_tag(language)?;
    Ok(http
        .get(format!("{}{}", connection.base_url, path))
        .header("X-Plex-Token", &connection.token)
        .header("Accept", "application/json")
        .header("X-Plex-Language", language)
        .header("X-Plex-Client-Identifier", PLEX_CLIENT_ID))
}

fn append_body(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), PlexError> {
    if body.len().saturating_add(chunk.len()) > MAX_BODY {
        return Err(PlexError::invalid_response("consultar Discover"));
    }
    body.extend_from_slice(chunk);
    Ok(())
}
fn invalid() -> String {
    "PLEX_DISCOVER_INVALID".into()
}
fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 20 && value.bytes().all(|c| c.is_ascii_digit())
}
fn text(value: &Value, key: &str, tokens: &[&str]) -> Option<String> {
    let value = value.get(key)?.as_str()?.trim();
    let decoded = urlencoding::decode(value).ok();
    if value.is_empty()
        || value.chars().count() > 256
        || value.chars().any(char::is_control)
        || contains_plex_token(value)
        || tokens.iter().any(|token| {
            !token.is_empty()
                && (value.contains(token)
                    || decoded.as_ref().is_some_and(|value| value.contains(token)))
        })
    {
        return None;
    }
    Some(value.to_owned())
}
fn item_section(item: &Value) -> Option<String> {
    let id = item.get("librarySectionID")?;
    id.as_str()
        .map(str::to_owned)
        .or_else(|| id.as_u64().map(|id| id.to_string()))
}
fn container<'a>(json: &'a Value, server: &str, section: &str) -> Result<&'a Value, String> {
    let value = json
        .get("MediaContainer")
        .filter(|v| v.is_object())
        .ok_or_else(invalid)?;
    if value
        .get("machineIdentifier")
        .is_some_and(|v| v.as_str() != Some(server))
        || value.get("librarySectionID").is_some()
            && item_section(value).as_deref() != Some(section)
    {
        return Err(invalid());
    }
    Ok(value)
}
fn image(
    item: &Value,
    key: &str,
    connection: &ResolvedPlexConnection,
    tokens: &[&str],
) -> Option<PlexImageRef> {
    let path = item.get(key)?.as_str()?;
    let decoded = urlencoding::decode(path).ok()?;
    if decoded.contains(['?', '#', '\\'])
        || decoded.chars().any(char::is_control)
        || decoded.split('/').any(|part| matches!(part, "." | ".."))
        || tokens
            .iter()
            .any(|token| !token.is_empty() && decoded.contains(token))
    {
        return None;
    }
    // Further constrain to library artwork; never accept provider or remote URLs.
    if !path.starts_with("/library/") {
        return None;
    }
    PlexImageRef::from_connection(connection, path)
}
fn parse_item(
    item: &Value,
    connection: &ResolvedPlexConnection,
    section: &str,
    strict_section: bool,
    tokens: &[&str],
) -> Option<PlexDiscoverItem> {
    if (strict_section || item.get("librarySectionID").is_some())
        && item_section(item).as_deref() != Some(section)
    {
        return None;
    }
    let kind = item.get("type")?.as_str()?;
    if !matches!(kind, "album" | "artist" | "collection" | "track") {
        return None;
    }
    let rating_key = item.get("ratingKey")?.as_str()?;
    if !valid_id(rating_key)
        || tokens
            .iter()
            .any(|token| !token.is_empty() && rating_key.contains(token))
    {
        return None;
    }
    let title = text(item, "title", tokens)?;
    let subtitle = match kind {
        "album" => text(item, "parentTitle", tokens).unwrap_or_default(),
        "track" => text(item, "parentTitle", tokens)?,
        _ => String::new(),
    };
    let thumb = image(item, "thumb", connection, tokens);
    let album = if kind == "album" || kind == "track" {
        let album_id = if kind == "track" {
            item.get("parentRatingKey")?.as_str()?
        } else {
            rating_key
        };
        if !valid_id(album_id)
            || tokens
                .iter()
                .any(|token| !token.is_empty() && album_id.contains(token))
        {
            return None;
        }
        Some(PlexAlbum {
            server_id: Some(connection.server_id.clone()),
            library_id: Some(section.to_string()),
            rating_key: album_id.into(),
            title: if kind == "track" {
                subtitle.clone()
            } else {
                title.clone()
            },
            artist: if kind == "track" {
                text(item, "grandparentTitle", tokens).unwrap_or_default()
            } else {
                subtitle.clone()
            },
            artist_rating_key: item
                .get(if kind == "track" {
                    "grandparentRatingKey"
                } else {
                    "parentRatingKey"
                })
                .and_then(Value::as_str)
                .filter(|id| {
                    valid_id(id)
                        && !tokens
                            .iter()
                            .any(|token| !token.is_empty() && id.contains(token))
                })
                .map(str::to_owned),
            year: item
                .get("year")
                .and_then(Value::as_u64)
                .and_then(|year| u32::try_from(year).ok()),
            genres: vec![],
            thumb: if kind == "track" {
                image(item, "parentThumb", connection, tokens).or_else(|| thumb.clone())
            } else {
                thumb.clone()
            },
        })
    } else {
        None
    };
    Some(PlexDiscoverItem {
        server_id: connection.server_id.clone(),
        library_id: section.into(),
        kind: kind.into(),
        rating_key: rating_key.into(),
        title,
        subtitle,
        thumb,
        album,
        added_at: timestamp(item, "addedAt"),
        last_viewed_at: timestamp(item, "lastViewedAt"),
    })
}
fn timestamp(item: &Value, key: &str) -> Option<u64> {
    item.get(key)?
        .as_u64()
        .filter(|n| *n > 0 && *n <= 9_007_199_254_740_991)
}
// DEV collection only: no family inference or private metadata. References are
// aliases shared across this response sample; equality never implies semantics.
#[derive(Default)]
struct ContractReferences {
    ids: Vec<String>,
}
impl ContractReferences {
    fn alias(&mut self, id: &str, tokens: &[&str]) -> Option<String> {
        if !valid_id(id)
            || tokens
                .iter()
                .any(|token| !token.is_empty() && id.contains(token))
        {
            return None;
        }
        let index = if let Some(index) = self.ids.iter().position(|value| value == id) {
            index
        } else {
            if self.ids.len() >= 512 {
                return None;
            }
            self.ids.push(id.to_owned());
            self.ids.len() - 1
        };
        Some(format!("ref{}", index + 1))
    }
}
fn contract_label(
    hub: &Value,
    field: &str,
    tokens: &[&str],
    refs: &mut ContractReferences,
) -> (Option<String>, Vec<String>, usize) {
    let Some(label) = text(hub, field, tokens) else {
        return (None, vec![], usize::from(hub.get(field).is_some()));
    };
    let vocabulary = [
        "hub",
        "home",
        "music",
        "artist",
        "artists",
        "album",
        "albums",
        "recent",
        "recentlyplayed",
        "recentlyviewed",
        "recentlyadded",
        "played",
        "viewed",
        "recently",
        "lastplayed",
        "lastviewed",
        "custom",
        "collection",
        "collections",
        "more",
        "by",
        "from",
        "moreby",
        "morefrom",
        "related",
        "similar",
        "recommended",
        "recommendation",
        "recommendations",
        "popular",
        "top",
        "random",
        "genre",
        "genres",
        "label",
        "labels",
        "studio",
        "style",
        "styles",
    ];
    if label.split('.').count() > 8 {
        return (None, vec![], 1);
    }
    let mut references = vec![];
    let mut omitted = 0;
    let parts: Vec<_> = label
        .split('.')
        .map(|part| {
            if let Some(alias) = refs.alias(part, tokens) {
                references.push(alias.clone());
                alias
            } else if vocabulary.contains(&part.to_ascii_lowercase().as_str()) {
                part.to_owned()
            } else {
                omitted += 1;
                "{omitted}".into()
            }
        })
        .collect();
    (Some(parts.join(".")), references, omitted)
}
fn contract_kind(value: Option<&str>) -> &'static str {
    match value {
        Some("artist") => "artist",
        Some("album") => "album",
        Some("track") => "track",
        Some("collection") => "collection",
        Some("mixed") => "mixed",
        None => "missing",
        _ => "other",
    }
}
fn hub_contract_shape(
    hub: &Value,
    index: usize,
    connection: &ResolvedPlexConnection,
    section: &str,
    tokens: &[&str],
    refs: &mut ContractReferences,
) -> Value {
    let (identifier, identifier_refs, identifier_omitted) =
        contract_label(hub, "hubIdentifier", tokens, refs);
    let (context, context_refs, context_omitted) = contract_label(hub, "context", tokens, refs);
    let records: Vec<_> = hub
        .get("Metadata")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_ITEMS)
        .collect();
    let mut item_types = std::collections::BTreeMap::<&str, usize>::new();
    let mut parent_types = std::collections::BTreeMap::<&str, usize>::new();
    let mut parents = std::collections::BTreeMap::<String, usize>::new();
    let (mut albums, mut valid_parents, mut thumbs_present, mut thumbs_usable) = (0, 0, 0, 0);
    for item in &records {
        *item_types
            .entry(contract_kind(item["type"].as_str()))
            .or_default() += 1;
        *parent_types
            .entry(contract_kind(item["parentType"].as_str()))
            .or_default() += 1;
        if item["type"].as_str() == Some("album") {
            albums += 1;
            if let Some(alias) = item["parentRatingKey"]
                .as_str()
                .and_then(|id| refs.alias(id, tokens))
            {
                valid_parents += 1;
                *parents.entry(alias).or_default() += 1;
            }
            thumbs_present += usize::from(item.get("parentThumb").is_some());
            thumbs_usable += usize::from(image(item, "parentThumb", connection, tokens).is_some());
        }
    }
    let same_parent = albums > 0 && valid_parents == albums && parents.len() == 1;
    let common_parent = if same_parent {
        parents.keys().next().cloned()
    } else {
        None
    };
    let matches_parent = |alias: &str| common_parent.as_ref().map(|parent| parent == alias);
    let describe_refs = |values: Vec<String>| {
        values.into_iter().map(|alias| serde_json::json!({"same_value_as_common_parent":matches_parent(&alias),"alias":alias})).collect::<Vec<_>>()
    };
    let mut route = "missing_or_redacted";
    let mut route_target = Value::Null;
    let mut query = std::collections::BTreeMap::<String, String>::new();
    let mut query_targets = vec![];
    let mut query_parsed = false;
    let mut omitted = 0;
    if let Some(key) = text(hub, "key", tokens) {
        let (path, parameters) = key.split_once('?').unwrap_or((&key, ""));
        let parts: Vec<_> = path.split('/').collect();
        let (shape, target) = match parts.as_slice() {
            ["", "library", "sections", id, "all"] => ("section.all", Some(("section", *id))),
            ["", "library", "sections", id, "recentlyViewed"] => {
                ("section.recentlyViewed", Some(("section", *id)))
            }
            ["", "library", "sections", id, "recentlyPlayed"] => {
                ("section.recentlyPlayed", Some(("section", *id)))
            }
            ["", "hubs", "sections", id, "recentlyPlayed"] => {
                ("section.hubs.recentlyPlayed", Some(("section", *id)))
            }
            ["", "library", "metadata", id, "children"] => {
                ("metadata.children", Some(("metadata", *id)))
            }
            ["", "library", "metadata", id] => ("metadata.item", Some(("metadata", *id))),
            ["", "hubs", "metadata", id] => ("metadata.hubs", Some(("metadata", *id))),
            ["", "hubs", "home", "recentlyAdded"] => ("home.recentlyAdded", None),
            _ => ("other", None),
        };
        route = shape;
        if let Some((role, id)) = target {
            if let Some(alias) = refs.alias(id, tokens) {
                route_target = serde_json::json!({"role":role,"alias":alias,"matches_requested_library":if role=="section" {Some(id==section)} else {None},"matches_common_parent":if role == "metadata" { matches_parent(&alias) } else { None }});
            } else {
                route = "other";
            }
        }
        if let Ok(url) = reqwest::Url::parse(&format!("http://contract.invalid/?{parameters}")) {
            query_parsed = true;
            for (field, value) in url.query_pairs().take(16) {
                let safe = match field.as_ref() {
                    "type" if ["8", "9", "10"].contains(&value.as_ref()) => Some(value.to_string()),
                    "sort"
                        if [
                            "lastViewedAt:desc",
                            "lastViewedAt:asc",
                            "addedAt:desc",
                            "lastPlayedAt:desc",
                            "artist.lastViewedAt:desc",
                            "album.lastViewedAt:desc",
                            "viewCount:desc",
                            "titleSort",
                            "titleSort:asc",
                            "random",
                            "year:desc",
                        ]
                        .contains(&value.as_ref()) =>
                    {
                        Some(value.to_string())
                    }
                    "unwatched" | "viewOffset" | "viewCount" | "viewCount>" | "viewCount>="
                        if ["0", "1"].contains(&value.as_ref()) =>
                    {
                        Some(value.to_string())
                    }
                    "artist.id" | "artist.ratingKey" | "artistRatingKey" | "parentRatingKey"
                    | "parent.id" | "artist" => {
                        let alias = refs.alias(&value, tokens);
                        if let Some(alias) = &alias {
                            query_targets.push(serde_json::json!({"field":field,"alias":alias,"matches_common_parent":matches_parent(alias)}));
                        }
                        alias
                    }
                    _ => None,
                };
                if let Some(value) = safe {
                    if query.insert(field.to_string(), value).is_some() {
                        omitted += 1;
                    }
                } else {
                    omitted += 1;
                }
            }
            omitted += url.query_pairs().skip(16).count();
        } else {
            omitted += 1;
        }
    }
    let mut dates = serde_json::Map::new();
    for field in ["addedAt", "lastViewedAt", "lastPlayedAt", "viewedAt"] {
        dates.insert(field.into(), serde_json::json!({"present":records.iter().filter(|item|item.get(field).is_some()).count(),"valid_numeric":records.iter().filter(|item|timestamp(item, field).is_some()).count()}));
    }
    serde_json::json!({"hub_index":index,"hub_identifier":identifier,"context":context,"omitted_label_parts":{"hub_identifier":identifier_omitted,"context":context_omitted},"identifier_references":describe_refs(identifier_refs),"context_references":describe_refs(context_refs),"hub_type":contract_kind(hub["type"].as_str()),"key_route":route,"route_target":route_target,"query":query,"query_target_references":query_targets,"omitted_parameters":omitted,"query_complete":query_parsed && omitted==0,"key_structure_omitted":matches!(route,"missing_or_redacted"|"other"),"items_observed":records.len(),"items_truncated":hub["Metadata"].as_array().is_some_and(|items|items.len()>MAX_ITEMS),"metadata_state":if hub["Metadata"].is_array(){"array"}else if hub.get("Metadata").is_some(){"invalid"}else{"missing"},"item_types":item_types,"parent_types":parent_types,"albums_observed":albums,"albums_with_valid_parent":valid_parents,"distinct_album_parents":parents.len(),"album_parent_aliases":parents,"all_albums_same_parent":same_parent,"common_parent_alias":common_parent,"parent_thumb":{"present":thumbs_present,"usable":thumbs_usable},"date_fields":dates})
}
fn hub_contract_samples(
    json: &Value,
    connection: &ResolvedPlexConnection,
    section: &str,
    tokens: &[&str],
) -> Vec<Value> {
    let mut refs = ContractReferences::default();
    json["MediaContainer"]["Hub"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_HUBS)
        .enumerate()
        .map(|(index, hub)| hub_contract_shape(hub, index, connection, section, tokens, &mut refs))
        .collect()
}
#[derive(Default)]
struct ContractCollectionBudget {
    started: Option<std::time::Instant>,
    sources: Vec<(String, String)>,
}
impl ContractCollectionBudget {
    fn admit(&mut self, server: &str, section: &str, now: std::time::Instant) -> Option<usize> {
        if self.started.map_or(true, |start| {
            now.saturating_duration_since(start) >= std::time::Duration::from_secs(60)
        }) {
            self.started = Some(now);
            self.sources.clear();
        }
        if self.sources.len() >= 8
            || self
                .sources
                .iter()
                .any(|source| source.0 == server && source.1 == section)
        {
            return None;
        }
        self.sources.push((server.to_owned(), section.to_owned()));
        Some(self.sources.len())
    }
}
fn observe_hub_contracts(
    json: &Value,
    connection: &ResolvedPlexConnection,
    section: &str,
    tokens: &[&str],
) {
    if !crate::diagnostics::dev_enabled() {
        return;
    }
    if !json["MediaContainer"]["Hub"]
        .as_array()
        .is_some_and(|hubs| !hubs.is_empty())
    {
        return;
    }
    static BUDGET: std::sync::OnceLock<std::sync::Mutex<ContractCollectionBudget>> =
        std::sync::OnceLock::new();
    let alias = BUDGET
        .get_or_init(Default::default)
        .lock()
        .ok()
        .and_then(|mut budget| {
            budget.admit(&connection.server_id, section, std::time::Instant::now())
        });
    // One bounded record/source/minute, <=8 sources, 8 hubs and12 items/hub.
    // Alias assignment/formatting/output happens after releasing the mutex.
    if let Some(alias) = alias {
        let samples = hub_contract_samples(json, connection, section, tokens);
        eprintln!(
            "[Sonante] dev.plex.hub_contract {}",
            serde_json::json!({"library_alias":alias,"hubs":samples})
        );
    }
}
// Classify the declared query contract, never the display title or a guessed
// music hub identifier. Album date sorts and the observed played-artist query
// are distinct families.
// Filtered recommendations and collection/provider paths cannot match. The
// server key is parsed as data only; it is never followed as a request target.
fn recent_family(hub: &Value, section: &str, tokens: &[&str]) -> Option<&'static str> {
    if hub["type"].as_str() == Some("artist") {
        // Observed in two music libraries: the numeric hub suffix is opaque,
        // not a library identity. Only this complete artist query is equivalent.
        if text(hub, "context", tokens).as_deref() != Some("hub.music.recent.played")
            || !text(hub, "hubIdentifier", tokens)?
                .strip_prefix("music.recent.played.")
                .is_some_and(valid_id)
        {
            return None;
        }
        let key = text(hub, "key", tokens)?;
        let (path, query) = key.split_once('?')?;
        if path != format!("/library/sections/{section}/all") || query.contains('#') {
            return None;
        }
        let url = reqwest::Url::parse(&format!("http://contract.invalid/?{query}")).ok()?;
        let mut seen = HashSet::new();
        for (key, value) in url.query_pairs() {
            if !seen.insert(key.to_string()) {
                return None;
            }
            match (key.as_ref(), value.as_ref()) {
                ("sort", "lastViewedAt:desc") | ("type", "8") | ("viewCount>", "1") => (),
                _ => return None,
            }
        }
        return (seen.len() == 3).then_some("played_artists");
    }
    if hub["type"].as_str() != Some("album") {
        return None;
    }
    let key = text(hub, "key", tokens)?;
    // The official global music sample also proves this exact built-in triple.
    // Section-scoped payload validation still applies; no global GET is made.
    if text(hub, "hubIdentifier", tokens).as_deref() == Some("home.music.recent")
        && text(hub, "context", tokens).as_deref() == Some("hub.home.music.recent")
        && key == "/hubs/home/recentlyAdded?type=8"
    {
        return Some("added");
    }
    let (path, query) = key.split_once('?')?;
    if path != format!("/library/sections/{section}/all") || query.contains('#') {
        return None;
    }
    let url = reqwest::Url::parse(&format!("http://contract.invalid/?{query}")).ok()?;
    let mut sort = None;
    let mut seen = HashSet::new();
    for (key, value) in url.query_pairs() {
        if !seen.insert(key.to_string()) {
            return None;
        }
        match (key.as_ref(), value.as_ref()) {
            ("sort", "addedAt:desc") => sort = Some("added"),
            ("sort", "lastViewedAt:desc") => sort = Some("played"),
            ("type", "9") | ("unwatched", "0") | ("viewOffset", "0") => (),
            _ => return None,
        }
    }
    // Played-only filters are not equivalent to an unfiltered added shelf.
    if sort == Some("added") && (seen.contains("unwatched") || seen.contains("viewOffset")) {
        return None;
    }
    sort
}
fn items(
    value: &Value,
    connection: &ResolvedPlexConnection,
    section: &str,
    strict: bool,
    tokens: &[&str],
    maximum: usize,
) -> Result<(Vec<PlexDiscoverItem>, bool), String> {
    let list = match value.get("Metadata") {
        Some(Value::Array(list)) => list,
        None if value.get("size").and_then(Value::as_u64) == Some(0) => return Ok((vec![], false)),
        _ => return Err(invalid()),
    };
    let mut seen = HashSet::new();
    let mut result = vec![];
    let mut partial = list.len() > maximum;
    for value in list.iter().take(maximum) {
        if let Some(item) = parse_item(value, connection, section, strict, tokens) {
            if seen.insert((item.kind.clone(), item.rating_key.clone())) {
                result.push(item);
            } else {
                partial = true;
            }
        } else {
            partial = true;
        }
    }
    partial |= value.get("more").and_then(Value::as_bool) == Some(true)
        || value
            .get("totalSize")
            .and_then(Value::as_u64)
            .is_some_and(|n| n > result.len() as u64);
    Ok((result, partial))
}
// Parent references are derived only from admitted album metadata. Artist
// artwork uses parentThumb exclusively; album covers are never substituted.
fn recommended_artist(
    item: &Value,
    connection: &ResolvedPlexConnection,
    section: &str,
    tokens: &[&str],
) -> Option<PlexDiscoverItem> {
    let album = parse_item(item, connection, section, false, tokens)?;
    if album.kind != "album"
        || item
            .get("parentType")
            .is_some_and(|kind| !kind.is_null() && kind.as_str() != Some("artist"))
    {
        return None;
    }
    Some(PlexDiscoverItem {
        server_id: connection.server_id.clone(),
        library_id: section.into(),
        kind: "artist".into(),
        rating_key: album.album?.artist_rating_key?,
        title: text(item, "parentTitle", tokens)?,
        subtitle: String::new(),
        thumb: image(item, "parentThumb", connection, tokens),
        album: None,
        added_at: None,
        last_viewed_at: None,
    })
}
fn artist_recommendation(
    hub: &Value,
    connection: &ResolvedPlexConnection,
    section: &str,
    tokens: &[&str],
) -> bool {
    // Observed artist-centered presentation contract. Extra filters are opaque
    // and allowed here only because no URL is reconstructed/followed.
    if hub["type"].as_str() != Some("album")
        || text(hub, "context", tokens).as_deref() != Some("hub.music.recent.artist")
        || !text(hub, "hubIdentifier", tokens)
            .and_then(|id| id.strip_prefix("music.recent.artist.").map(valid_id))
            .unwrap_or(false)
    {
        return false;
    }
    let Some(key) = text(hub, "key", tokens) else {
        return false;
    };
    let Some((path, query)) = key.split_once('?') else {
        return false;
    };
    if path != format!("/library/sections/{section}/all") || query.contains('#') {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(&format!("http://contract.invalid/?{query}")) else {
        return false;
    };
    let (mut artist, mut album_type) = (None, false);
    for (field, value) in url.query_pairs() {
        match field.as_ref() {
            "type" => {
                if album_type || value != "9" {
                    return false;
                }
                album_type = true;
            }
            "artist.id" => {
                if artist.is_some() || !valid_id(&value) {
                    return false;
                }
                artist = Some(value.into_owned());
            }
            _ => (), // Only display already-returned items; unknown filters stay opaque.
        }
    }
    let Some(artist) = artist.filter(|_| album_type) else {
        return false;
    };
    let Some(records) = hub["Metadata"].as_array().filter(|items| !items.is_empty()) else {
        return false;
    };
    records.iter().take(MAX_ITEMS).all(|record| {
        recommended_artist(record, connection, section, tokens)
            .is_some_and(|parent| parent.rating_key == artist)
    })
}
fn parse_hubs(
    json: &Value,
    connection: &ResolvedPlexConnection,
    section: &str,
    tokens: &[&str],
) -> Result<PlexDiscoverPage, String> {
    let c = container(json, &connection.server_id, section)?;
    let hubs = match c.get("Hub") {
        Some(Value::Array(hubs)) => hubs,
        None if c.get("size").and_then(Value::as_u64) == Some(0) => {
            return Ok(PlexDiscoverPage {
                language: "en".into(),
                server_id: connection.server_id.clone(),
                library_id: section.into(),
                hubs: vec![],
                partial: false,
            })
        }
        _ => return Err(invalid()),
    };
    let mut parsed = vec![];
    let mut seen = HashSet::new();
    let mut partial = hubs.len() > MAX_HUBS;
    for hub in hubs.iter().take(MAX_HUBS) {
        // Identifiers identify rows, not items; paths in key/hubKey are never followed.
        let Some(id) = text(hub, "hubIdentifier", tokens) else {
            partial = true;
            continue;
        };
        let Some(title) = text(hub, "title", tokens) else {
            partial = true;
            continue;
        };
        if !seen.insert(id.clone()) {
            partial = true;
            continue;
        }
        match items(hub, connection, section, false, tokens, MAX_ITEMS) {
            Ok((items, row_partial)) if !items.is_empty() => {
                partial |= row_partial;
                let mut parents = HashSet::new();
                let artist_candidates = hub["Metadata"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(MAX_ITEMS)
                    .filter(|raw| {
                        items.iter().any(|item| {
                            item.kind == "album"
                                && raw["ratingKey"].as_str() == Some(item.rating_key.as_str())
                                && item
                                    .album
                                    .as_ref()
                                    .and_then(|album| album.artist_rating_key.as_deref())
                                    == raw["parentRatingKey"].as_str()
                        })
                    })
                    .filter_map(|item| recommended_artist(item, connection, section, tokens))
                    .filter(|artist| parents.insert(artist.rating_key.clone()))
                    .collect();
                parsed.push(PlexDiscoverHub {
                    artist_recommendation: artist_recommendation(hub, connection, section, tokens),
                    artist_candidates,
                    context: text(hub, "context", tokens),
                    recent_family: recent_family(hub, section, tokens).filter(|family| {
                        items.iter().all(|item| {
                            item.kind
                                == if *family == "played_artists" {
                                    "artist"
                                } else {
                                    "album"
                                }
                        })
                    }),
                    id,
                    title,
                    items,
                    partial: row_partial,
                });
            }
            Ok((_, row_partial)) => partial |= row_partial,
            Err(_) => partial = true,
        }
    }
    if !hubs.is_empty() && parsed.is_empty() && partial {
        return Err(invalid());
    }
    Ok(PlexDiscoverPage {
        language: "en".into(),
        server_id: connection.server_id.clone(),
        library_id: section.into(),
        hubs: parsed,
        partial,
    })
}
fn recent_path(section: &str, window: usize) -> Result<String, String> {
    if !valid_id(section) || !(MAX_HERO_CANDIDATES..=MAX_HERO_WINDOW).contains(&window) {
        return Err("PLEX_DISCOVER_CONTEXT".into());
    }
    // Separate bounded album query; recommendation hubs stay capped at12.
    Ok(format!("/library/sections/{section}/all?type=9&sort=addedAt%3Adesc&limit={window}&X-Plex-Container-Start=0&X-Plex-Container-Size={window}"))
}
fn parse_recent(
    json: &Value,
    connection: &ResolvedPlexConnection,
    section: &str,
    language: &str,
    tokens: &[&str],
    window: usize,
) -> Result<PlexDiscoverRecent, String> {
    recent_path(section, window)?;
    let c = container(json, &connection.server_id, section)?;
    if c.get("offset").is_some_and(|n| n.as_u64() != Some(0)) {
        return Err(invalid());
    }
    let (parsed, _) = items(c, connection, section, false, tokens, window)?;
    let mut albums: Vec<_> = parsed
        .into_iter()
        .filter(|item| item.kind == "album")
        .collect();
    let returned = c
        .get("Metadata")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if returned > 0 && albums.is_empty() {
        return Err(invalid());
    }
    // Normal pagination/totalSize>window is expected, not an operational failure.
    let partial = returned > window
        || albums.len() < returned.min(window)
        || (returned < window
            && c.get("totalSize")
                .and_then(Value::as_u64)
                .is_some_and(|n| n > returned as u64));
    let dates_missing = albums.iter().any(|item| item.added_at.is_none());
    // Missing dates keep the requested server-order tail in an individual
    // section; All must omit them because a cross-library date rank is unknown.
    albums.sort_by(|a, b| b.added_at.cmp(&a.added_at));
    Ok(PlexDiscoverRecent {
        language: language_tag(language).map_err(|e| e.public_code())?.into(),
        server_id: connection.server_id.clone(),
        library_id: section.into(),
        items: albums,
        partial,
        dates_missing,
    })
}
fn request_error(error: PlexError) -> String {
    match error.kind {
        PlexErrorKind::Unauthorized | PlexErrorKind::HttpError(403) => "PLEX_DISCOVER_AUTH",
        PlexErrorKind::ConfigurationChanged | PlexErrorKind::IdentityMismatch => {
            "PLEX_DISCOVER_CONTEXT"
        }
        PlexErrorKind::InvalidResponse => "PLEX_DISCOVER_INVALID",
        _ => "PLEX_DISCOVER_UNAVAILABLE",
    }
    .into()
}

impl PlexClient {
    pub async fn get_library_mix_years(
        &self,
        server: &str,
        section: &str,
        language: &str,
    ) -> Result<PlexMixYears, String> {
        let (details, connection, _) = self
            .discover_json(
                server,
                section,
                &format!("/library/sections/{section}?includeDetails=1"),
                language,
                "years.details",
            )
            .await
            .map_err(|e| e.public_code())?;
        let c = container(&details, server, section)?;
        if !album_year_field(c) {
            return Err(invalid());
        }
        let reference = if let Some(key) = year_reference(c, section, &connection.base_url) {
            key
        } else {
            let (filters, connection, _) = self
                .discover_json(
                    server,
                    section,
                    &format!("/library/sections/{section}/filters"),
                    language,
                    "years.filters",
                )
                .await
                .map_err(|e| e.public_code())?;
            year_reference(
                container(&filters, server, section)?,
                section,
                &connection.base_url,
            )
            .ok_or_else(invalid)?
        };
        let mut window = YearWindow::default();
        for _ in 0..YEAR_PAGES {
            let offset = window.offset;
            let path = format!(
                "{reference}&X-Plex-Container-Start={offset}&X-Plex-Container-Size={YEAR_PAGE}"
            );
            let response = self
                .discover_json(server, section, &path, language, "years.choices")
                .await
                .map_err(|e| e.public_code());
            let parsed =
                response.and_then(|(json, _, _)| parse_year_page(&json, server, section, offset));
            if !window.accept(parsed)? {
                break;
            }
        }
        Ok(PlexMixYears {
            server_id: server.into(),
            library_id: section.into(),
            language: language_tag(language).map_err(|e| e.public_code())?.into(),
            years: window.years.into_iter().collect(),
            partial: window.partial,
        })
    }
    pub async fn get_library_mix_styles(
        &self,
        server: &str,
        section: &str,
        language: &str,
    ) -> Result<PlexMixStyles, String> {
        let (details, connection, _) = self
            .discover_json(
                server,
                section,
                &format!("/library/sections/{section}?includeDetails=1"),
                language,
                "styles.details",
            )
            .await
            .map_err(|e| e.public_code())?;
        let capabilities = style_fields(container(&details, server, section)?);
        if capabilities.is_empty() {
            return Err("PLEX_DISCOVER_STYLE_UNSUPPORTED".into());
        }
        let mut selected = capabilities.iter().find_map(|(field, typ)| {
            style_reference(
                container(&details, server, section).ok()?,
                section,
                &connection.base_url,
                field,
                *typ,
            )
            .map(|reference| (*field, reference))
        });
        if selected.is_none() {
            let (filters, connection, _) = self
                .discover_json(
                    server,
                    section,
                    &format!("/library/sections/{section}/filters"),
                    language,
                    "styles.filters",
                )
                .await
                .map_err(|e| e.public_code())?;
            let c = container(&filters, server, section)?;
            selected = capabilities.iter().find_map(|(field, typ)| {
                style_reference(c, section, &connection.base_url, field, *typ)
                    .map(|reference| (*field, reference))
            });
        }
        let (field, reference) = selected.ok_or("PLEX_DISCOVER_STYLE_UNSUPPORTED")?;
        let mut window = StyleWindow::default();
        for _ in 0..YEAR_PAGES {
            let offset = window.offset;
            let path = format!(
                "{reference}&X-Plex-Container-Start={offset}&X-Plex-Container-Size={YEAR_PAGE}"
            );
            let response = self
                .discover_json(server, section, &path, language, "styles.choices")
                .await
                .map_err(|e| e.public_code());
            let parsed = response.and_then(|(json, connection, _)| {
                let tokens = self.discover_tokens(&connection)?;
                parse_style_page(
                    &json,
                    server,
                    section,
                    offset,
                    &tokens.iter().map(String::as_str).collect::<Vec<_>>(),
                )
            });
            if !window.accept(parsed)? {
                break;
            }
        }
        Ok(PlexMixStyles {
            server_id: server.into(),
            library_id: section.into(),
            language: language_tag(language).map_err(|e| e.public_code())?.into(),
            field: field.into(),
            choices: window.choices.into_values().collect(),
            partial: window.partial,
        })
    }
    pub async fn get_library_style_mix(
        &self,
        server: &str,
        section: &str,
        language: &str,
        limit: usize,
        selection: &PlexStyleSelection,
    ) -> Result<PlexLibraryMixPage, String> {
        self.library_mix_request(
            server,
            section,
            language,
            limit,
            style_path(section, limit, selection)?,
        )
        .await
    }
    pub async fn get_library_decade_mix(
        &self,
        server: &str,
        section: &str,
        language: &str,
        limit: usize,
        decade: u32,
    ) -> Result<PlexLibraryMixPage, String> {
        let path = decade_path(section, limit, decade)?;
        self.library_mix_request(server, section, language, limit, path)
            .await
    }
    pub async fn get_library_mix(
        &self,
        server: &str,
        section: &str,
        language: &str,
        limit: usize,
    ) -> Result<PlexLibraryMixPage, String> {
        let path = mix_path(section, limit).map_err(|code| {
            log_mix_diagnostic(
                Some(&PlexDiscoverFailure::new("request", "response")),
                None,
                &MixParseCounts::default(),
            );
            code
        })?;
        self.library_mix_request(server, section, language, limit, path)
            .await
    }
    async fn library_mix_request(
        &self,
        server: &str,
        section: &str,
        language: &str,
        limit: usize,
        path: String,
    ) -> Result<PlexLibraryMixPage, String> {
        let mut counts = MixParseCounts::default();
        let (json, connection, status) = self
            .discover_json(server, section, &path, language, "mix.query")
            .await
            .map_err(|failure| {
                log_mix_diagnostic(Some(&failure), None, &counts);
                failure.public_code()
            })?;
        let tokens = self.discover_tokens(&connection).map_err(|code| {
            log_mix_diagnostic(
                Some(&PlexDiscoverFailure::new("connection", "context")),
                Some(status),
                &counts,
            );
            code
        })?;
        let result = parse_mix_observed(
            self,
            &json,
            &connection,
            section,
            language,
            limit,
            &tokens.iter().map(String::as_str).collect::<Vec<_>>(),
            &mut counts,
        );
        let failure = result.as_ref().err().map(|code| {
            scoped_container(&json, server, section, "parse")
                .err()
                .unwrap_or_else(|| {
                    PlexDiscoverFailure::new(
                        "parse",
                        if code == "PLEX_DISCOVER_CONTEXT" {
                            "context"
                        } else {
                            "response"
                        },
                    )
                })
        });
        log_mix_diagnostic(failure.as_ref(), Some(status), &counts);
        result
    }
    async fn discover_json(
        &self,
        server: &str,
        section: &str,
        path: &str,
        language: &str,
        stage: &'static str,
    ) -> Result<(Value, ResolvedPlexConnection, u16), PlexDiscoverFailure> {
        if validate_plex_server_id(server).is_err()
            || !valid_id(section)
            || !self.navigation_server_is_selected(server)
        {
            return Err(PlexDiscoverFailure::new(stage, "context"));
        }
        let language = language_tag(language)?;
        let operation = "consultar Discover";
        let manager = &self.connection_manager;
        let mut connection = manager
            .resolve()
            .await
            .map_err(|e| PlexDiscoverFailure::from_error("connection", e))?;
        let http =
            discover_http().map_err(|_| PlexDiscoverFailure::new("connection", "transport"))?;
        for attempt in 0..2 {
            if connection.server_id != server {
                return Err(PlexDiscoverFailure::new(stage, "context"));
            }
            let mut http_status = None;
            let response = async {
                let mut response = discover_request(http, &connection, path, language)
                    .map_err(|_| PlexError::invalid_response(operation))?
                    .send()
                    .await
                    .map_err(|error| PlexError::from_transport(operation, &error))?;
                http_status = Some(response.status().as_u16());
                if !response.status().is_success() {
                    return Err(PlexError::from_status(operation, response.status()));
                }
                if response
                    .content_length()
                    .is_some_and(|n| n > MAX_BODY as u64)
                {
                    return Err(PlexError::invalid_response(operation));
                }
                let mut body = vec![];
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|error| PlexError::from_transport(operation, &error))?
                {
                    append_body(&mut body, &chunk)?;
                }
                serde_json::from_slice::<Value>(&body)
                    .map_err(|_| PlexError::invalid_response(operation))
            }
            .await;
            match response {
                Ok(json) => {
                    let current = manager
                        .snapshot()
                        .map_err(|e| PlexDiscoverFailure::from_error(stage, e))?;
                    if current.generation != connection.generation
                        || current
                            .identity
                            .as_ref()
                            .map(|i| i.machine_identifier.as_str())
                            != Some(server)
                    {
                        return Err(PlexDiscoverFailure::new(stage, "context"));
                    }
                    scoped_container(&json, server, section, stage)
                        .map_err(|e| e.with_status(http_status))?;
                    return Ok((
                        json,
                        connection,
                        http_status.ok_or_else(|| PlexDiscoverFailure::new(stage, "response"))?,
                    ));
                }
                Err(error) if attempt == 0 && error.is_route_unavailable() => {
                    connection = manager
                        .refresh(connection.generation)
                        .await
                        .map_err(|e| PlexDiscoverFailure::from_error(stage, e))?
                }
                Err(error) => {
                    return Err(
                        PlexDiscoverFailure::from_error(stage, error).with_status(http_status)
                    )
                }
            }
        }
        Err(PlexDiscoverFailure::new(stage, "transport"))
    }
    fn discover_tokens<'a>(
        &self,
        connection: &'a ResolvedPlexConnection,
    ) -> Result<Vec<String>, String> {
        let snapshot = self.connection_manager.snapshot().map_err(request_error)?;
        if snapshot.generation != connection.generation
            || snapshot
                .identity
                .as_ref()
                .map(|i| i.machine_identifier.as_str())
                != Some(connection.server_id.as_str())
        {
            return Err("PLEX_DISCOVER_CONTEXT".into());
        }
        Ok(vec![
            connection.token.clone(),
            snapshot.account_token,
            snapshot.resolved_token.unwrap_or_default(),
        ])
    }
    pub async fn get_discover(
        &self,
        server: &str,
        section: &str,
        language: &str,
    ) -> Result<PlexDiscoverPage, String> {
        let path = format!("/hubs/sections/{section}?count={MAX_ITEMS}");
        let (json, connection, _) = self
            .discover_json(server, section, &path, language, "hubs")
            .await
            .map_err(|e| e.public_code())?;
        let tokens = self.discover_tokens(&connection)?;
        let mut page = parse_hubs(
            &json,
            &connection,
            section,
            &tokens.iter().map(String::as_str).collect::<Vec<_>>(),
        )?;
        page.language = language_tag(language).map_err(|e| e.public_code())?.into();
        observe_hub_contracts(
            &json,
            &connection,
            section,
            &tokens.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        Ok(page)
    }
    pub async fn get_discover_recent(
        &self,
        server: &str,
        section: &str,
        language: &str,
        window: usize,
    ) -> Result<PlexDiscoverRecent, String> {
        let path = recent_path(section, window)?;
        let (json, connection, _) = self
            .discover_json(server, section, &path, language, "recent")
            .await
            .map_err(|e| e.public_code())?;
        let tokens = self.discover_tokens(&connection)?;
        parse_recent(
            &json,
            &connection,
            section,
            language,
            &tokens.iter().map(String::as_str).collect::<Vec<_>>(),
            window,
        )
    }
    pub async fn get_discover_artist(
        &self,
        server: &str,
        section: &str,
        id: &str,
        language: &str,
    ) -> Result<PlexDiscoverArtistContent, String> {
        if !valid_id(id) {
            return Err(invalid());
        }
        let (json, connection, _) = self
            .discover_json(
                server,
                section,
                &format!("/library/metadata/{id}"),
                language,
                "artist",
            )
            .await
            .map_err(|e| e.public_code())?;
        validate_artist_parent(&json, server, section, id)?;
        self.discover_tokens(&connection)?;
        // Reuse the full ArtistView readers and their existing top-track fallback.
        // Sequential reads keep this user navigation within one IPC slot.
        let albums = self.get_artist_albums(id).await?;
        self.discover_tokens(&connection)?;
        let top_tracks = self.get_artist_top_tracks(id).await?;
        self.discover_tokens(&connection)?;
        if albums.iter().any(|a| !valid_id(&a.rating_key) || a.artist_rating_key.as_deref()!=Some(id) || a.thumb.as_ref().is_some_and(|image|image.server_id!=server))
            || top_tracks.iter().any(|track| !matches!(&track.media_locator,MediaLocator::Plex {server_id,..} if server_id==server)) {return Err(invalid());}
        Ok(PlexDiscoverArtistContent {
            server_id: server.into(),
            library_id: section.into(),
            artist_id: id.into(),
            albums,
            top_tracks,
        })
    }
    pub async fn get_discover_children(
        &self,
        server: &str,
        section: &str,
        kind: &str,
        id: &str,
        language: &str,
    ) -> Result<PlexDiscoverHub, String> {
        if !valid_id(id) || kind != "collection" {
            return Err(invalid());
        }
        let (json, connection, _) = self
            .discover_json(
                server,
                section,
                &format!("/library/metadata/{id}"),
                language,
                "children",
            )
            .await
            .map_err(|e| e.public_code())?;
        let parent = json["MediaContainer"]["Metadata"]
            .as_array()
            .filter(|items| items.len() == 1)
            .and_then(|items| items.first())
            .ok_or_else(invalid)?;
        if parent["ratingKey"].as_str() != Some(id)
            || parent["type"].as_str() != Some(kind)
            || item_section(parent).as_deref() != Some(section)
        {
            return Err(invalid());
        }
        let tokens = self.discover_tokens(&connection)?;
        let refs: Vec<_> = tokens.iter().map(String::as_str).collect();
        let title = text(parent, "title", &refs).ok_or_else(invalid)?;
        let path = format!("/library/collections/{id}/children");
        let path = format!("{path}?X-Plex-Container-Start=0&X-Plex-Container-Size={MAX_CHILDREN}");
        let generation = connection.generation;
        let (json, connection, _) = self
            .discover_json(server, section, &path, language, "children")
            .await
            .map_err(|e| e.public_code())?;
        if connection.generation != generation {
            return Err("PLEX_DISCOVER_CONTEXT".into());
        }
        let tokens = self.discover_tokens(&connection)?;
        let refs: Vec<_> = tokens.iter().map(String::as_str).collect();
        let (items, partial) = items(
            container(&json, server, section)?,
            &connection,
            section,
            false,
            &refs,
            MAX_CHILDREN,
        )?;
        Ok(PlexDiscoverHub {
            artist_recommendation: false,
            artist_candidates: vec![],
            context: None,
            recent_family: None,
            id: format!("{kind}.{id}"),
            title,
            items: items
                .into_iter()
                .filter(|item| item.kind == "album")
                .collect(),
            partial,
        })
    }
}

fn scoped_container<'a>(
    json: &'a Value,
    server: &str,
    section: &str,
    stage: &'static str,
) -> Result<&'a Value, PlexDiscoverFailure> {
    let value = json
        .get("MediaContainer")
        .filter(|v| v.is_object())
        .ok_or_else(|| PlexDiscoverFailure::new(stage, "response"))?;
    if value
        .get("machineIdentifier")
        .is_some_and(|v| v.as_str() != Some(server))
    {
        return Err(PlexDiscoverFailure::new(stage, "identity"));
    }
    if value.get("librarySectionID").is_some() && item_section(value).as_deref() != Some(section) {
        return Err(PlexDiscoverFailure::new(stage, "membership"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn decade_query_uses_inclusive_album_year_bounds_and_existing_track_limits() {
        let path = decade_path("7", 100, 1990).unwrap();
        let url = reqwest::Url::parse(&format!("http://fixture.invalid{path}")).unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(pairs.get("album.year>").unwrap(), "1990");
        assert_eq!(pairs.get("album.year<").unwrap(), "1999");
        assert_eq!(pairs.get("type").unwrap(), "10");
        assert_eq!(pairs.get("sort").unwrap(), "random");
        assert!(!path.contains("addedAt"));
        for decade in [0, 999, 1991, 10000] {
            assert!(decade_path("7", 100, decade).is_err());
        }
        assert!(decade_path("7", 101, 1990).is_err());
        assert!(decade_path("../7", 10, 1990).is_err());
    }
    #[test]
    fn year_capabilities_and_announced_references_are_section_scoped() {
        let details = json!({"Meta":{"Type":[{"type":"album","Field":[{"key":"album.year","type":"integer"}],"Filter":[{"filter":"year","filterType":"integer","key":"/library/sections/7/year?type=9"}]}]}});
        assert!(album_year_field(&details));
        assert_eq!(
            year_reference(&details, "7", "https://safe.invalid"),
            Some("/library/sections/7/year?type=9".into())
        );
        let mut entry = details["Meta"]["Type"][0].clone();
        entry["type"] = json!(10);
        assert!(!album_year_field(&json!({"Directory":[entry]})));
        for key in [
            "/library/sections/8/year?type=9",
            "/library/sections/7/all?type=9",
            "/library/sections/7/year?type=10",
            "/library/sections/7/year?type=9&type=9",
            "/library/sections/7/year?type=9&token=secret",
            "https://foreign.invalid/library/sections/7/year?type=9",
        ] {
            let filters = json!({"Directory":[{"filter":"year","filterType":"integer","key":key}]});
            assert!(year_reference(&filters, "7", "https://safe.invalid").is_none());
        }
    }
    #[test]
    fn year_capabilities_accept_observed_root_type_and_preserve_previous_formats() {
        let owner = json!({"type":"9","Field":[{"key":"album.year","type":"integer"}],"Filter":[{"filter":"year","filterType":"integer","key":"/library/sections/7/year?type=9"}]});
        // Observed PMS shape: Type is directly inside MediaContainer, Meta absent.
        let response = json!({"MediaContainer":{"librarySectionID":7,"Type":[owner.clone()]}});
        let observed = container(&response, "server-a", "7").unwrap();
        assert!(observed.get("Meta").is_none());
        assert!(observed["Type"].is_array());
        assert!(album_year_field(observed));
        assert_eq!(
            year_reference(observed, "7", "https://safe.invalid"),
            Some("/library/sections/7/year?type=9".into())
        );
        for typ in [json!(9), json!("9"), json!("album")] {
            let mut declared = owner.clone();
            declared["type"] = typ;
            for c in [
                json!({"Type":[declared.clone()]}),
                json!({"Type":declared.clone()}),
                json!({"Meta":{"Type":[declared.clone()]}}),
                json!({"Directory":[declared.clone()]}),
            ] {
                assert!(album_year_field(&c));
                assert_eq!(
                    year_reference(&c, "7", "https://safe.invalid"),
                    Some("/library/sections/7/year?type=9".into())
                );
            }
        }
    }
    #[test]
    fn root_year_capabilities_keep_type_field_and_reference_rejections() {
        let owner = json!({"type":9,"Field":[{"key":"album.year","type":"integer"}],"Filter":[{"filter":"year","filterType":"integer","key":"/library/sections/7/year?type=9"}]});
        for typ in [
            json!(8),
            json!(10),
            json!("artist"),
            json!("track"),
            json!(null),
            json!(true),
        ] {
            let mut changed = owner.clone();
            changed["type"] = typ;
            assert!(!album_year_field(&json!({"Type":[changed]})));
        }
        for field in [
            json!({"key":"track.year","type":"integer"}),
            json!({"key":"album.decade","type":"integer"}),
            json!({"key":"addedAt","type":"integer"}),
            json!({"key":"album.year","type":"string"}),
            json!({"key":"album.year","type":null}),
        ] {
            let mut changed = owner.clone();
            changed["Field"] = json!([field]);
            assert!(!album_year_field(&json!({"Type":[changed]})));
        }
        for key in [
            "/library/sections/8/year?type=9",
            "/library/sections/7/year?type=10",
            "/library/sections/7/year?type=9&type=9",
            "/library/sections/7/year?type=9&token=secret",
            "https://foreign.invalid/library/sections/7/year?type=9",
            "/library/sections/7/all?type=9",
            "/library/sections/7/year",
            "/library/sections/7/year?type=9#fragment",
        ] {
            let mut changed = owner.clone();
            changed["Filter"][0]["key"] = json!(key);
            let c = json!({"Type":[changed]});
            assert!(album_year_field(&c));
            assert!(year_reference(&c, "7", "https://safe.invalid").is_none());
        }
        for (name, kind) in [("year", "string"), ("decade", "integer")] {
            let mut changed = owner.clone();
            changed["Filter"][0]["filter"] = json!(name);
            changed["Filter"][0]["filterType"] = json!(kind);
            assert!(
                year_reference(&json!({"Type":[changed]}), "7", "https://safe.invalid").is_none()
            );
        }
    }
    #[test]
    fn year_pages_reject_foreign_scope_and_keep_missing_years_explicit() {
        let page = json!({"MediaContainer":{"librarySectionID":7,"offset":0,"totalSize":80,"Directory":[{"key":"1989"},{"key":"1990"},{"key":"1999"},{"key":"2000"},{"key":null},{"key":true}]}});
        let (years, more, partial, observed) = parse_year_page(&page, "s", "7", 0).unwrap();
        assert_eq!(years, vec![1989, 1990, 1999, 2000]);
        assert!(more && partial);
        assert_eq!(observed, 6);
        assert!(parse_year_page(&page, "s", "8", 0).is_err());
        assert!(parse_year_page(&page, "s", "7", 64).is_err());
        assert!(parse_year_page(
            &json!({"MediaContainer":{"machineIdentifier":"foreign","size":0}}),
            "s",
            "7",
            0
        )
        .is_err());
    }
    #[test]
    fn year_pagination_advances_by_actual_count_caps_and_preserves_partial_success() {
        let mut window = YearWindow::default();
        for page in 0..YEAR_PAGES {
            let values = (0..32).map(|i| 1800 + (page * 32 + i) as u32).collect();
            let more = window.accept(Ok((values, true, false, 32))).unwrap();
            assert_eq!(window.offset, (page + 1) * 32);
            assert_eq!(more, page + 1 < YEAR_PAGES);
        }
        assert!(window.partial);
        assert_eq!(window.years.len(), 128);
        let mut window = YearWindow::default();
        assert!(window.accept(Ok((vec![1990], true, false, 1))).unwrap());
        assert!(!window.accept(Err(invalid())).unwrap());
        assert!(window.partial);
        assert!(window.years.contains(&1990));
        let mut window = YearWindow::default();
        assert!(window.accept(Ok((vec![1990], true, false, 1))).unwrap());
        assert!(!window.accept(Ok((vec![1990], true, false, 1))).unwrap());
        assert!(window.partial);
        assert!(YearWindow::default().accept(Err(invalid())).is_err());
        let mut complete = YearWindow::default();
        assert!(!complete.accept(Ok((vec![1990], false, false, 1))).unwrap());
        assert!(!complete.partial);
    }
    fn style_owner() -> Value {
        json!({"type":8,"Field":[{"key":"artist.style","type":"tag"}],"Filter":[{"filter":"style","filterType":"string","key":"/library/sections/7/style?type=8"}]})
    }
    #[test]
    fn styles_require_explicit_musical_style_scope_not_genre() {
        let owner = style_owner();
        for c in [
            json!({"Type":[owner.clone()]}),
            json!({"Meta":{"Type":[owner.clone()]}}),
            json!({"Directory":[owner.clone()]}),
        ] {
            assert_eq!(style_fields(&c), vec![("artist.style", 8)]);
            assert_eq!(
                style_reference(&c, "7", "https://safe.invalid", "artist.style", 8),
                Some("/library/sections/7/style?type=8".into())
            );
        }
        for (typ, field) in [(9, "album.style"), (10, "track.style")] {
            let c = json!({"Type":[{"type":typ,"Field":[{"key":field,"type":"tag"}]}]});
            assert_eq!(style_fields(&c), vec![(field, typ)]);
        }
        for (typ, field, kind) in [
            (8, "artist.genre", "tag"),
            (9, "artist.style", "tag"),
            (8, "artist.style", "string"),
        ] {
            assert!(style_fields(
                &json!({"Type":[{"type":typ,"Field":[{"key":field,"type":kind}]}]})
            )
            .is_empty());
        }
    }
    #[test]
    fn style_references_cannot_cross_origin_section_type_or_parameters() {
        for key in [
            "https://foreign.invalid/library/sections/7/style?type=8",
            "/library/sections/8/style?type=8",
            "/library/sections/7/genre?type=8",
            "/library/sections/7/style?type=9",
            "/library/sections/7/style?type=8&token=secret",
            "/library/sections/7/style?type=8&type=8",
            "/library/sections/7/style?type=8#fragment",
        ] {
            let mut owner = style_owner();
            owner["Filter"][0]["key"] = json!(key);
            assert!(style_reference(
                &json!({"Type":[owner]}),
                "7",
                "https://safe.invalid",
                "artist.style",
                8
            )
            .is_none());
        }
        let filters = json!({"Directory":[{"filter":"style","filterType":"string","key":"/library/sections/7/style?type=8"}]});
        assert!(
            style_reference(&filters, "7", "https://safe.invalid", "artist.style", 8).is_some()
        );
        assert!(style_reference(&filters, "7", "https://safe.invalid", "album.style", 9).is_none());
    }
    #[test]
    fn style_query_uses_source_local_ids_and_qualified_tag_equality() {
        let selection = PlexStyleSelection {
            field: "artist.style".into(),
            ids: vec!["123".into(), "456".into()],
        };
        let path = style_path("7", 100, &selection).unwrap();
        assert!(path.contains("type=10&sort=random&limit=100"));
        assert!(path.ends_with("&artist.style=123,456"));
        assert!(!path.contains("genre"));
        for bad in [
            PlexStyleSelection {
                field: "genre".into(),
                ids: vec!["123".into()],
            },
            PlexStyleSelection {
                field: "artist.style".into(),
                ids: vec!["123&sort=title".into()],
            },
            PlexStyleSelection {
                field: "artist.style".into(),
                ids: vec![],
            },
        ] {
            assert!(style_path("7", 100, &bad).is_err());
        }
        assert!(style_path("7", 101, &selection).is_err());
    }
    #[test]
    fn style_page_preserves_titles_and_ids_rejects_private_data_and_foreign_scope() {
        let page = json!({"MediaContainer":{"librarySectionID":7,"offset":0,"totalSize":70,"Directory":[{"key":"123","title":"A long musical style name"},{"key":"124","title":"token-secret"},{"key":"/unsafe","title":"Invalid"}]}});
        let (choices, more, partial, observed) =
            parse_style_page(&page, "s", "7", 0, &["token-secret"]).unwrap();
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].id, "123");
        assert_eq!(choices[0].title, "A long musical style name");
        assert!(more && partial);
        assert_eq!(observed, 3);
        assert!(parse_style_page(&page, "s", "8", 0, &[]).is_err());
        assert!(parse_style_page(&page, "s", "7", 64, &[]).is_err());
    }
    #[test]
    fn style_pagination_caps_without_losing_partial_success() {
        let mut window = StyleWindow::default();
        for page in 0..YEAR_PAGES {
            let values = (0..32)
                .map(|n| PlexStyleChoice {
                    id: (page * 32 + n).to_string(),
                    title: "Style".into(),
                })
                .collect();
            assert_eq!(
                window.accept(Ok((values, true, false, 32))).unwrap(),
                page + 1 < YEAR_PAGES
            );
            assert_eq!(window.offset, (page + 1) * 32);
        }
        assert!(window.partial);
        assert_eq!(window.choices.len(), 128);
        let mut window = StyleWindow::default();
        assert!(window
            .accept(Ok((
                vec![PlexStyleChoice {
                    id: "1".into(),
                    title: "Style".into()
                }],
                true,
                false,
                1
            )))
            .unwrap());
        assert!(!window.accept(Err(invalid())).unwrap());
        assert_eq!(window.choices.len(), 1);
        assert!(window.partial);
    }
    fn mix_track(id: &str) -> Value {
        json!({"type":"track","ratingKey":id,"title":"Synthetic track","parentRatingKey":"12","parentTitle":"Synthetic album","grandparentTitle":"Synthetic artist","librarySectionID":"7","duration":120000,"Media":[{"Part":[{"key":format!("/library/parts/{id}/file.flac"),"file":"/synthetic/music/file.flac"}]}]})
    }
    #[test]
    fn mix_query_uses_documented_random_track_contract_and_hard_limits() {
        assert_eq!(mix_path("7",100).unwrap(),"/library/sections/7/all?type=10&sort=random&limit=100&X-Plex-Container-Start=0&X-Plex-Container-Size=100");
        for (id, limit) in [("7", 0), ("7", 101), ("../unsafe", 10), ("7?token", 10)] {
            assert!(mix_path(id, limit).is_err());
        }
    }
    #[test]
    fn mix_diagnostics_identify_required_fields_without_relaxing_origin_or_exposing_values() {
        let client = super::super::tests::media_client_for_test(&AppConfig::default());
        for (field, reason) in [
            ("librarySectionID", "library_missing"),
            ("type", "type_missing_or_not_track"),
            ("ratingKey", "track_id_missing_or_invalid"),
            ("title", "title_missing_or_unsafe"),
            ("parentRatingKey", "album_id_missing_or_invalid"),
            ("parentTitle", "album_title_missing_or_unsafe"),
            ("Media", "media_missing"),
        ] {
            let mut record = mix_track("99");
            record.as_object_mut().unwrap().remove(field);
            let mut counts = MixParseCounts::default();
            let result = parse_mix_observed(
                &client,
                &json!({"MediaContainer":{"Metadata":[record]}}),
                &connection(),
                "7",
                "en",
                100,
                &["fixture-secret"],
                &mut counts,
            );
            assert!(result.is_err());
            assert_eq!(counts.observed, 1);
            assert_eq!(counts.accepted, 0);
            assert_eq!(counts.rejected.get(reason), Some(&1));
            let failure = PlexDiscoverFailure::new("parse", "response");
            let payload = mix_diagnostic(Some(&failure), Some(200), &counts);
            assert_eq!(payload["stage"], "parse");
            assert_eq!(payload["status"], 200);
            let serialized = payload.to_string();
            for value in [
                "Synthetic track",
                "Synthetic album",
                "fixture-secret",
                "server-a",
                "/library/parts/99/file.flac",
            ] {
                assert!(!serialized.contains(value));
            }
        }
    }
    #[test]
    fn mix_diagnostics_preserve_http_status_and_partial_rejection_counts() {
        let failure = PlexDiscoverFailure::from_error(
            "mix.query",
            PlexError::from_status("private operation", StatusCode::FORBIDDEN),
        );
        let payload = mix_diagnostic(Some(&failure), None, &MixParseCounts::default());
        assert_eq!(payload["stage"], "mix.query");
        assert_eq!(payload["category"], "permission");
        assert_eq!(payload["status"], 403);
        assert!(!payload.to_string().contains("private operation"));
        let client = super::super::tests::media_client_for_test(&AppConfig::default());
        let mut foreign = mix_track("100");
        foreign["librarySectionID"] = json!("8");
        let mut counts = MixParseCounts::default();
        let page = parse_mix_observed(
            &client,
            &json!({"MediaContainer":{"Metadata":[mix_track("99"),foreign]}}),
            &connection(),
            "7",
            "en",
            100,
            &[],
            &mut counts,
        )
        .unwrap();
        assert!(page.partial);
        assert_eq!(counts.accepted, 1);
        assert_eq!(counts.rejected.get("foreign_library"), Some(&1));
    }
    #[test]
    fn mix_inherits_only_absent_track_library_from_an_explicit_matching_container() {
        let client = super::super::tests::media_client_for_test(&AppConfig::default());
        let mut missing = mix_track("99");
        missing.as_object_mut().unwrap().remove("librarySectionID");
        assert!(
            parse_item(&missing, &connection(), "7", true, &[]).is_none(),
            "shared strict admission stays unchanged"
        );
        for library in [json!("7"), json!(7)] {
            let records = (1..=100)
                .map(|id| {
                    let mut track = mix_track(&id.to_string());
                    track.as_object_mut().unwrap().remove("librarySectionID");
                    track
                })
                .collect::<Vec<_>>();
            let mut counts = MixParseCounts::default();
            let page = parse_mix_observed(&client, &json!({"MediaContainer":{"machineIdentifier":"server-a","librarySectionID":library,"Metadata":records}}), &connection(), "7", "en", 100, &[], &mut counts).unwrap();
            assert_eq!(page.tracks.len(), 100);
            assert!(!page.partial);
            assert_eq!(counts.accepted, 100);
            assert!(counts.rejected.is_empty());
            assert!(page
                .tracks
                .iter()
                .all(|track| track.server_id == "server-a" && track.library_id == "7"));
        }
        // Explicit per-track origin is sufficient when the container omits it.
        let page = parse_mix(
            &client,
            &json!({"MediaContainer":{"Metadata":[mix_track("99")]}}),
            &connection(),
            "7",
            "en",
            100,
            &[],
        )
        .unwrap();
        assert_eq!(page.tracks.len(), 1);
        let mut counts = MixParseCounts::default();
        assert!(parse_mix_observed(
            &client,
            &json!({"MediaContainer":{"Metadata":[missing]}}),
            &connection(),
            "7",
            "en",
            100,
            &[],
            &mut counts
        )
        .is_err());
        assert_eq!(counts.rejected.get("library_missing"), Some(&1));
    }
    #[test]
    fn mix_never_inherits_present_invalid_or_conflicting_library_fields() {
        let client = super::super::tests::media_client_for_test(&AppConfig::default());
        for (value, reason) in [
            (json!(null), "library_invalid"),
            (json!(false), "library_invalid"),
            (json!(7.5), "library_invalid"),
            (json!({}), "library_invalid"),
            (json!(""), "library_invalid"),
            (json!("invalid"), "library_invalid"),
            (json!("8"), "foreign_library"),
            (json!(8), "foreign_library"),
        ] {
            let mut record = mix_track("99");
            record["librarySectionID"] = value;
            let mut counts = MixParseCounts::default();
            assert!(parse_mix_observed(
                &client,
                &json!({"MediaContainer":{"librarySectionID":"7","Metadata":[record]}}),
                &connection(),
                "7",
                "en",
                100,
                &[],
                &mut counts
            )
            .is_err());
            assert_eq!(counts.accepted, 0);
            assert_eq!(counts.rejected.get(reason), Some(&1));
        }
        let mut missing = mix_track("99");
        missing.as_object_mut().unwrap().remove("librarySectionID");
        for library in [
            json!(null),
            json!(false),
            json!("invalid"),
            json!("8"),
            json!(8),
        ] {
            for record in [missing.clone(), mix_track("99")] {
                assert!(parse_mix(
                    &client,
                    &json!({"MediaContainer":{"librarySectionID":library,"Metadata":[record]}}),
                    &connection(),
                    "7",
                    "en",
                    100,
                    &[]
                )
                .is_err());
            }
        }
        assert!(parse_mix(&client,&json!({"MediaContainer":{"machineIdentifier":"other","librarySectionID":"7","Metadata":[missing]}}),&connection(),"7","en",100,&[]).is_err());
    }
    #[test]
    fn mix_inherited_library_preserves_all_other_track_and_locator_filters() {
        let client = super::super::tests::media_client_for_test(&AppConfig::default());
        for (field, reason) in [
            ("type", "type_missing_or_not_track"),
            ("ratingKey", "track_id_missing_or_invalid"),
            ("title", "title_missing_or_unsafe"),
            ("parentRatingKey", "album_id_missing_or_invalid"),
            ("parentTitle", "album_title_missing_or_unsafe"),
            ("Media", "media_missing"),
        ] {
            let mut record = mix_track("99");
            record.as_object_mut().unwrap().remove("librarySectionID");
            record.as_object_mut().unwrap().remove(field);
            let mut counts = MixParseCounts::default();
            assert!(parse_mix_observed(
                &client,
                &json!({"MediaContainer":{"librarySectionID":"7","Metadata":[record]}}),
                &connection(),
                "7",
                "en",
                100,
                &[],
                &mut counts
            )
            .is_err());
            assert_eq!(counts.rejected.get(reason), Some(&1));
            assert_eq!(counts.accepted, 0);
        }
        for key in [
            "https://foreign.invalid/file",
            "/library/parts/99/file?X-Plex-Token=fixture-secret",
            "/library/parts/99/../file",
        ] {
            let mut record = mix_track("99");
            record.as_object_mut().unwrap().remove("librarySectionID");
            record["Media"][0]["Part"][0]["key"] = json!(key);
            assert!(parse_mix(
                &client,
                &json!({"MediaContainer":{"librarySectionID":"7","Metadata":[record]}}),
                &connection(),
                "7",
                "en",
                100,
                &["fixture-secret"]
            )
            .is_err());
        }
        // A rejected record does not discard an independently valid track.
        let mut missing = mix_track("99");
        missing.as_object_mut().unwrap().remove("librarySectionID");
        let mut invalid = missing.clone();
        invalid["ratingKey"] = json!("invalid");
        let page = parse_mix(
            &client,
            &json!({"MediaContainer":{"librarySectionID":"7","Metadata":[missing,invalid]}}),
            &connection(),
            "7",
            "en",
            100,
            &[],
        )
        .unwrap();
        assert_eq!(page.tracks.len(), 1);
        assert!(page.partial);
    }
    #[test]
    fn mix_navigation_uses_safe_pms_parents_without_changing_track_acceptance() {
        let client = super::super::tests::media_client_for_test(&AppConfig::default());
        let parse = |track, tokens: &[&str]| {
            parse_mix(&client, &json!({"MediaContainer":{"Metadata":[track]}}), &connection(), "7", "en", 100, tokens).unwrap()
        };
        let mut track = mix_track("99");
        track["grandparentRatingKey"] = json!("501");
        let page = parse(track.clone(), &[]);
        assert_eq!(page.tracks[0].album_rating_key.as_deref(), Some("12"));
        assert_eq!(page.tracks[0].artist_rating_key.as_deref(), Some("501"));
        assert_eq!(page.tracks[0].server_id, "server-a");
        assert_eq!(page.tracks[0].library_id, "7");
        let dto = serde_json::to_value(&page.tracks[0]).unwrap();
        assert_eq!(dto["album_rating_key"], "12");
        assert_eq!(dto["artist_rating_key"], "501");
        let playback = serde_json::to_value(&page.tracks[0].track).unwrap();
        assert!(playback.get("album_rating_key").is_none());
        assert!(playback.get("artist_rating_key").is_none());
        track.as_object_mut().unwrap().remove("grandparentRatingKey");
        assert!(parse(track.clone(), &[]).tracks[0].artist_rating_key.is_none());
        for value in [Value::Null, json!(42), json!(false), json!({"id":"501"}), json!(""), json!(" 501"), json!("../501"), json!("https://private.invalid/?token=fixture-secret"), json!("501?X-Plex-Token=fixture-secret"), json!("%35%30%31"), json!("１２"), json!("123456789012345678901")] {
            track["grandparentRatingKey"] = value;
            let page = parse(track.clone(), &["fixture-secret"]);
            assert_eq!(page.tracks.len(), 1);
            assert!(!page.partial);
            assert!(page.tracks[0].artist_rating_key.is_none());
            let dto = serde_json::to_string(&page).unwrap();
            assert!(!dto.contains("fixture-secret") && !dto.contains("private.invalid"));
        }
        track["grandparentRatingKey"] = json!("501");
        let page = parse(track, &["501"]);
        assert_eq!(page.tracks.len(), 1);
        assert!(page.tracks[0].artist_rating_key.is_none());
        assert!(serde_json::to_value(&page.tracks[0]).unwrap().get("artist_rating_key").is_none());
    }

    #[test]
    fn mix_parser_retains_source_locator_metadata_caps_and_deduplicates_identity() {
        let client = super::super::tests::media_client_for_test(&AppConfig::default());
        let values = vec![mix_track("99"), mix_track("99"), mix_track("100")];
        let parsed = parse_mix(
            &client,
            &json!({"MediaContainer":{"Metadata":values,"totalSize":400}}),
            &connection(),
            "7",
            "en",
            100,
            &[],
        )
        .unwrap();
        assert_eq!(parsed.tracks.len(), 2);
        assert!(parsed.partial && parsed.has_more);
        assert_eq!(parsed.tracks[0].library_id, "7");
        assert_eq!(parsed.tracks[0].server_id, "server-a");
        assert_eq!(
            parsed.tracks[0].track.artist.as_deref(),
            Some("Synthetic artist")
        );
        assert_eq!(
            parsed.tracks[0].track.album_title.as_deref(),
            Some("Synthetic album")
        );
        let serialized = serde_json::to_string(&parsed).unwrap();
        assert!(!serialized.contains("fixture-secret"));
        assert!(!serialized.contains("http"));
        let capped=parse_mix(&client,&json!({"MediaContainer":{"Metadata":(1..=150).map(|i|mix_track(&i.to_string())).collect::<Vec<_>>()}}),&connection(),"7","pt-BR",100,&[]).unwrap();
        assert_eq!(capped.tracks.len(), 100);
        assert!(capped.partial);
    }
    #[test]
    fn mix_parser_distinguishes_empty_invalid_partial_membership_and_secret_data() {
        let client = super::super::tests::media_client_for_test(&AppConfig::default());
        let parse = |value| {
            parse_mix(
                &client,
                &value,
                &connection(),
                "7",
                "en",
                100,
                &["fixture-secret"],
            )
        };
        assert!(parse(json!({"MediaContainer":{"size":0}}))
            .unwrap()
            .tracks
            .is_empty());
        assert!(parse(json!({"MediaContainer":{"Metadata":null}})).is_err());
        for field in ["librarySectionID", "ratingKey", "title"] {
            let mut value = mix_track("99");
            value[field] = json!(if field == "librarySectionID" {
                "8"
            } else {
                "fixture-secret"
            });
            assert!(parse(json!({"MediaContainer":{"Metadata":[value]}})).is_err());
        }
        for key in [
            "https://foreign.invalid/file?X-Plex-Token=fixture-secret",
            "/library/parts/99/file?X-Plex-Token=fixture-secret",
            "/library/parts/99/../file",
        ] {
            let mut value = mix_track("99");
            value["Media"][0]["Part"][0]["key"] = json!(key);
            assert!(parse(json!({"MediaContainer":{"Metadata":[value]}})).is_err());
        }
        let mut foreign = mix_track("100");
        foreign["librarySectionID"] = json!("8");
        let value =
            parse(json!({"MediaContainer":{"Metadata":[mix_track("99"),foreign]}})).unwrap();
        assert_eq!(value.tracks.len(), 1);
        assert!(value.partial);
        assert!(parse(json!({"MediaContainer":{"machineIdentifier":"other","size":0}})).is_err());
    }
    fn connection() -> ResolvedPlexConnection {
        ResolvedPlexConnection {
            server_id: "server-a".into(),
            base_url: "http://fixture.invalid".into(),
            token: "fixture-secret".into(),
            generation: 1,
        }
    }
    fn album(id: &str) -> Value {
        json!({"type":"album","ratingKey":id,"title":"Synthetic album","parentTitle":"Synthetic artist","librarySectionID":"7","thumb":format!("/library/metadata/{id}/thumb/1")})
    }
    fn page(items: Vec<Value>) -> Value {
        json!({"MediaContainer":{"librarySectionID":7,"Hub":[{"hubIdentifier":"music.recent","title":"Server's real row title","Metadata":items}]}})
    }
    #[test]
    fn discover_observed_played_artist_contract_preserves_opaque_suffix_and_origins() {
        // Synthetic metadata, faithful to the maintainer's two-library contract.
        let mut value: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/plex-discover-recent-played-artists.json"
        ))
        .unwrap();
        let first = parse_hubs(&value, &connection(), "7", &[]).unwrap();
        assert_eq!(first.hubs[0].recent_family, Some("played_artists"));
        assert_eq!(first.hubs[0].items.len(), 12);
        assert!(first.hubs[0].items.iter().all(|item| item.kind == "artist"
            && item.library_id == "7"
            && item.server_id == connection().server_id
            && item.last_viewed_at.is_some()));
        assert_eq!(first.hubs[0].items[0].title, first.hubs[0].items[1].title);
        assert_ne!(
            first.hubs[0].items[0].rating_key,
            first.hubs[0].items[1].rating_key
        );
        assert_eq!(
            first.hubs[1].recent_family, None,
            "same title does not classify an unknown group"
        );
        value["MediaContainer"]["librarySectionID"] = json!(8);
        for hub in value["MediaContainer"]["Hub"].as_array_mut().unwrap() {
            hub["key"] = json!(hub["key"]
                .as_str()
                .unwrap()
                .replace("/sections/7/", "/sections/8/"));
            for item in hub["Metadata"].as_array_mut().unwrap() {
                item["librarySectionID"] = json!(8);
            }
        }
        let second = parse_hubs(&value, &connection(), "8", &[]).unwrap();
        assert_eq!(
            second.hubs[0].recent_family,
            Some("played_artists"),
            "numeric suffix91 is not the library key"
        );
        assert_eq!(
            first.hubs[0].items[0].rating_key,
            second.hubs[0].items[0].rating_key
        );
        assert_eq!(second.hubs[0].items[0].library_id, "8");
        assert_eq!(second.hubs[1].recent_family, None);
    }
    #[test]
    fn discover_played_artist_contract_rejects_extra_filters_missing_pairs_and_mixed_kinds() {
        let value: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/plex-discover-recent-played-artists.json"
        ))
        .unwrap();
        let hub = &value["MediaContainer"]["Hub"][0];
        for key in [
            "/library/sections/8/all?sort=lastViewedAt:desc&type=8&viewCount%3E=1",
            "/library/sections/7/all?sort=lastViewedAt:desc&type=8&viewCount%3E%3D=1",
            "/library/sections/7/all?sort=lastViewedAt:desc&type=8&viewCount%3E=0",
            "/library/sections/7/all?sort=lastViewedAt:desc&type=9&viewCount%3E=1",
            "/library/sections/7/all?sort=addedAt:desc&type=8&viewCount%3E=1",
            "/library/sections/7/all?sort=lastViewedAt:desc&type=8&viewCount%3E=1&genre=Synthetic",
            "/library/sections/7/all?sort=lastViewedAt:desc&type=8&viewCount%3E=1&type=8",
            "/library/sections/7/all?sort=lastViewedAt:desc&type=8",
            "/library/sections/7/all?sort=lastViewedAt:desc&type=8&viewCount%3E=1#fragment",
        ] {
            let mut changed = hub.clone();
            changed["key"] = json!(key);
            assert_eq!(recent_family(&changed, "7", &[]), None);
        }
        for (field, replacement) in [
            ("context", "hub.music.recent"),
            ("hubIdentifier", "music.recent.played.custom"),
            ("hubIdentifier", "custom.music.recent.played.91"),
            ("hubIdentifier", "music.recent.played.91.7"),
            ("type", "album"),
        ] {
            let mut changed = hub.clone();
            changed[field] = json!(replacement);
            assert_eq!(recent_family(&changed, "7", &[]), None);
        }
        let mut changed = value.clone();
        changed["MediaContainer"]["Hub"][0]["Metadata"][0] = album("500");
        assert_eq!(
            parse_hubs(&changed, &connection(), "7", &[]).unwrap().hubs[0].recent_family,
            None
        );
        let mut changed = value.clone();
        changed["MediaContainer"]["Hub"][0]["key"] =
            json!("/library/sections/7/all?viewCount%3E=1&type=8&sort=lastViewedAt%3Adesc");
        assert_eq!(
            parse_hubs(&changed, &connection(), "7", &[]).unwrap().hubs[0].recent_family,
            Some("played_artists")
        );
    }
    #[test]
    fn discover_full_artist_parent_requires_exact_source_type_and_identity() {
        let base = json!({"MediaContainer":{"librarySectionID":7,"machineIdentifier":"server-a","Metadata":[{"ratingKey":"88","type":"artist","librarySectionID":7}]}});
        assert!(validate_artist_parent(&base, "server-a", "7", "88").is_ok());
        for (field, value) in [
            ("ratingKey", json!("89")),
            ("type", json!("album")),
            ("librarySectionID", json!(8)),
        ] {
            let mut changed = base.clone();
            changed["MediaContainer"]["Metadata"][0][field] = value;
            assert!(validate_artist_parent(&changed, "server-a", "7", "88").is_err());
        }
        assert!(validate_artist_parent(&base, "other-server", "7", "88").is_err());
        let mut changed = base.clone();
        changed["MediaContainer"]["Metadata"] = json!([]);
        assert!(validate_artist_parent(&changed, "server-a", "7", "88").is_err());
    }
    #[test]
    fn discover_artist_recommendations_use_observed_contract_with_opaque_filter_and_missing_parent_type(
    ) {
        let value: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/plex-discover-artist-recommendations.json"
        ))
        .unwrap();
        let page = parse_hubs(&value, &connection(), "7", &[]).unwrap();
        assert!(page.hubs[0].artist_recommendation);
        assert!(!page.hubs[1].artist_recommendation);
        assert!(!page.hubs[2].artist_recommendation);
        for (index, hub) in page.hubs.iter().enumerate() {
            assert_eq!(hub.items.len(), 1);
            assert_eq!(hub.items[0].kind, "album");
            assert_eq!(hub.artist_candidates.len(), 1);
            let artist = &hub.artist_candidates[0];
            assert_eq!(artist.kind, "artist");
            assert_eq!(artist.server_id, "server-a");
            assert_eq!(artist.library_id, "7");
            assert_eq!(artist.rating_key, (501 + index).to_string());
            assert_eq!(
                artist.thumb.as_ref().unwrap().path,
                format!("/library/metadata/{}/thumb/1", 501 + index)
            );
            assert_ne!(
                artist.thumb, hub.items[0].thumb,
                "album cover is not artist portrait"
            );
        }
        let mut value = value.clone();
        value["MediaContainer"]["Hub"][0]["Metadata"][0]["parentThumb"] =
            json!("https://unsafe.invalid/image");
        let result = parse_hubs(&value, &connection(), "7", &[]).unwrap();
        assert!(result.hubs[0].artist_recommendation);
        assert!(result.hubs[0].artist_candidates[0].thumb.is_none());
    }
    #[test]
    fn discover_artist_recommendations_reject_identity_conflicts_and_unknown_families() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/plex-discover-artist-recommendations.json"
        ))
        .unwrap();
        for (field, replacement) in [
            ("context", json!("hub.music.recent.label")),
            ("hubIdentifier", json!("music.more.by.7")),
            ("type", json!("collection")),
            ("key", json!("/library/sections/8/all?type=9&artist.id=501")),
            (
                "key",
                json!("/library/sections/7/all?type=9&artist.id=501&artist.id=501"),
            ),
            ("key", json!("/library/sections/7/all?type=9&artist.id=502")),
            (
                "key",
                json!("/library/sections/7/all?type=9&artist.id=invalid"),
            ),
            ("key", json!("/library/sections/7/all?type=8&artist.id=501")),
        ] {
            let mut hub = fixture["MediaContainer"]["Hub"][0].clone();
            hub[field] = replacement;
            assert!(!artist_recommendation(&hub, &connection(), "7", &[]));
        }
        for (field, replacement) in [
            ("parentRatingKey", json!("invalid")),
            ("parentType", json!("album")),
            ("librarySectionID", json!(8)),
            ("type", json!("artist")),
        ] {
            let mut hub = fixture["MediaContainer"]["Hub"][0].clone();
            hub["Metadata"][0][field] = replacement;
            assert!(!artist_recommendation(&hub, &connection(), "7", &[]));
            assert!(recommended_artist(&hub["Metadata"][0], &connection(), "7", &[]).is_none());
        }
        let mut hub = fixture["MediaContainer"]["Hub"][0].clone();
        let mut other = hub["Metadata"][0].clone();
        other["ratingKey"] = json!("201");
        other["parentRatingKey"] = json!("502");
        hub["Metadata"].as_array_mut().unwrap().push(other);
        assert!(!artist_recommendation(&hub, &connection(), "7", &[]));
        hub["Metadata"][1]["parentRatingKey"] = json!("501");
        assert!(artist_recommendation(&hub, &connection(), "7", &[]));
        let response = json!({"MediaContainer":{"librarySectionID":7,"Hub":[hub]}});
        assert_eq!(
            parse_hubs(&response, &connection(), "7", &[]).unwrap().hubs[0]
                .artist_candidates
                .len(),
            1
        );
    }
    #[test]
    fn hub_contract_collection_album_parents_targets_and_aliases_preserve_real_equality() {
        // Structural synthetic example only, not evidence of a More-by family.
        let mut hub = json!({"hubIdentifier":"music.more.by.812345","context":"hub.music.more.by.812345","type":"album","title":"Private artist","key":"/library/metadata/812345/children?type=9&artist.id=812345&parentRatingKey=912345","Metadata":[{"type":"album","parentType":"artist","parentRatingKey":"812345","title":"Private album A","parentTitle":"Private artist","parentThumb":"/library/metadata/812345/thumb/1700000000"},{"type":"album","parentType":"artist","parentRatingKey":"812345","title":"Private album B","parentThumb":"https://private.invalid/image"}]});
        let shape = hub_contract_shape(
            &hub,
            3,
            &connection(),
            "7",
            &[],
            &mut ContractReferences::default(),
        );
        assert_eq!(shape["hub_index"], 3);
        assert_eq!(shape["hub_type"], "album");
        assert_eq!(shape["parent_types"]["artist"], 2);
        assert_eq!(shape["albums_with_valid_parent"], 2);
        assert_eq!(shape["distinct_album_parents"], 1);
        assert_eq!(shape["all_albums_same_parent"], true);
        assert_eq!(shape["parent_thumb"], json!({"present":2,"usable":1}));
        assert_eq!(shape["key_route"], "metadata.children");
        assert_eq!(shape["route_target"]["role"], "metadata");
        assert_eq!(shape["route_target"]["matches_common_parent"], true);
        assert_eq!(
            shape["identifier_references"][0]["same_value_as_common_parent"],
            true
        );
        assert_eq!(
            shape["context_references"][0]["same_value_as_common_parent"],
            true
        );
        assert_eq!(
            shape["query_target_references"][0]["matches_common_parent"],
            true
        );
        assert_eq!(
            shape["query_target_references"][1]["matches_common_parent"],
            false
        );
        assert_ne!(
            shape["query"]["artist.id"],
            shape["query"]["parentRatingKey"]
        );
        for private in [
            "812345",
            "912345",
            "1700000000",
            "Private",
            "private.invalid",
            "/library/",
            "https:",
        ] {
            assert!(!shape.to_string().contains(private));
        }
        hub["key"] = json!("/library/metadata/912345/children?artist.id=912345");
        let shape = hub_contract_shape(
            &hub,
            0,
            &connection(),
            "7",
            &[],
            &mut ContractReferences::default(),
        );
        assert_eq!(
            shape["route_target"]["matches_common_parent"], false,
            "different references are not collapsed to one placeholder"
        );
        hub["Metadata"][1]["parentRatingKey"] = json!("912345");
        let shape = hub_contract_shape(
            &hub,
            0,
            &connection(),
            "7",
            &[],
            &mut ContractReferences::default(),
        );
        assert_eq!(shape["distinct_album_parents"], 2);
        assert_eq!(shape["all_albums_same_parent"], false);
        assert!(shape["common_parent_alias"].is_null());
        assert!(shape["route_target"]["matches_common_parent"].is_null());
        hub["Metadata"][1]["parentRatingKey"] = json!("invalid-private-id");
        let shape = hub_contract_shape(
            &hub,
            0,
            &connection(),
            "7",
            &[],
            &mut ContractReferences::default(),
        );
        assert_eq!(shape["albums_with_valid_parent"], 1);
        assert_eq!(shape["distinct_album_parents"], 1);
        assert_eq!(shape["all_albums_same_parent"], false);
        hub["Metadata"] = json!([]);
        assert_eq!(
            hub_contract_shape(
                &hub,
                0,
                &connection(),
                "7",
                &[],
                &mut ContractReferences::default()
            )["all_albums_same_parent"],
            false
        );
    }
    #[test]
    fn hub_contract_collection_bounds_cover_all_hubs_with_shared_aliases_and_no_names() {
        let hub = json!({"hubIdentifier":"music.more.from.812345","context":"hub.music.artist.812345","type":"album","key":"/library/sections/7/all?type=9&artist.id=812345&genre=PrivateGenre&sort=PrivateSort","Metadata":(0..100).map(|i|json!({"type":"album","parentType":"artist","parentRatingKey":"812345","parentTitle":"Private Person","ratingKey":i.to_string(),"parentThumb":"/library/metadata/812345/thumb/1"})).collect::<Vec<_>>()});
        let mut json = json!({"MediaContainer":{"Hub":vec![hub;20]}});
        json["MediaContainer"]["Hub"][1]["hubIdentifier"] = json!("music.more.from.912345");
        json["MediaContainer"]["Hub"][1]["Metadata"][0]["parentRatingKey"] = json!("912345");
        let samples = hub_contract_samples(&json, &connection(), "7", &[]);
        assert_eq!(samples.len(), 8);
        for (index, sample) in samples.iter().enumerate() {
            assert_eq!(sample["hub_index"], index);
            assert_eq!(sample["items_observed"], 12);
            assert_eq!(sample["items_truncated"], true);
            assert_eq!(sample["omitted_parameters"], 2);
        }
        assert_eq!(
            samples[0]["common_parent_alias"], samples[2]["common_parent_alias"],
            "same reference across hubs keeps its alias"
        );
        assert_ne!(
            samples[0]["hub_identifier"], samples[1]["hub_identifier"],
            "different suffixes retain different aliases without interpreting them"
        );
        assert_eq!(samples[1]["distinct_album_parents"], 2);
        for private in ["812345", "912345", "Private", "/library/", "genre"] {
            assert!(!serde_json::to_string(&samples).unwrap().contains(private));
        }
        let mut refs = ContractReferences::default();
        for id in 1..=512 {
            assert!(refs.alias(&id.to_string(), &[]).is_some());
        }
        assert!(refs.alias("513", &[]).is_none());
        assert_eq!(refs.ids.len(), 512);
        assert_eq!(refs.alias("1", &[]).as_deref(), Some("ref1"));
    }
    #[test]
    fn hub_contract_collection_redacts_arbitrary_filters_and_signals_incomplete_fields() {
        let hub = json!({"hubIdentifier":"music.PrivateName.812345","context":"hub.music.more.by","type":"PrivateType","key":"/library/sections/8/all?artist.id=812345&type=9&artist=PrivateName&sort=PrivateSort&genre=PrivateGenre","Metadata":[{"type":"album","parentType":"PrivateParentType","parentRatingKey":"812345","parentThumb":"/library/metadata/812345/thumb/fixture-secret","title":"PrivateTitle"}]});
        let shape = hub_contract_shape(
            &hub,
            0,
            &connection(),
            "7",
            &["fixture-secret"],
            &mut ContractReferences::default(),
        );
        assert_eq!(shape["hub_identifier"], "music.{omitted}.ref1");
        assert_eq!(shape["omitted_label_parts"]["hub_identifier"], 1);
        assert_eq!(shape["hub_type"], "other");
        assert_eq!(shape["parent_types"]["other"], 1);
        assert_eq!(shape["route_target"]["matches_requested_library"], false);
        assert_eq!(shape["omitted_parameters"], 3);
        assert_eq!(shape["query_complete"], false);
        assert_eq!(shape["parent_thumb"]["usable"], 0);
        for private in ["Private", "812345", "fixture-secret", "genre"] {
            assert!(!shape.to_string().contains(private));
        }
        let mut malformed = hub.clone();
        malformed["Metadata"] = json!(null);
        assert_eq!(
            hub_contract_shape(
                &malformed,
                0,
                &connection(),
                "7",
                &[],
                &mut ContractReferences::default()
            )["metadata_state"],
            "invalid"
        );
        malformed["key"] =
            json!("/library/sections/7/all?type=9&artist.id=812345&artist.id=912345");
        let shape = hub_contract_shape(
            &malformed,
            0,
            &connection(),
            "7",
            &[],
            &mut ContractReferences::default(),
        );
        assert_eq!(shape["omitted_parameters"], 1);
        assert_eq!(
            shape["query_target_references"].as_array().unwrap().len(),
            2
        );
        assert_ne!(
            shape["query_target_references"][0]["alias"],
            shape["query_target_references"][1]["alias"]
        );
        // Section IDs and metadata IDs use different semantic namespaces even
        // when their numeric text happens to match. No parent-target claim.
        malformed["key"] = json!("/library/sections/812345/all?type=9");
        malformed["Metadata"] = json!([{ "type":"album", "parentRatingKey":"812345" }]);
        let shape = hub_contract_shape(
            &malformed,
            0,
            &connection(),
            "812345",
            &[],
            &mut ContractReferences::default(),
        );
        assert_eq!(shape["route_target"]["matches_requested_library"], true);
        assert!(shape["route_target"]["matches_common_parent"].is_null());
        assert_eq!(shape["query_complete"], true);
    }
    #[test]
    fn hub_contract_collection_is_sanitized_structural_data_not_family_evidence() {
        // Synthetic namespace probes test privacy, not a real Plex artist hub.
        let hub = json!({"hubIdentifier":"music.recentlyplayed.7","context":"hub.music.recentlyplayed","type":"artist","title":"Private artist/header","key":"/library/sections/7/all?sort=lastViewedAt%3Adesc&type=8&accountID=987654&genre=PrivateGenre","Metadata":[{"type":"artist","title":"Private Artist","ratingKey":"876543","librarySectionID":7,"lastViewedAt":1700000011,"viewedAt":"unknown"}]});
        let shape = hub_contract_shape(
            &hub,
            0,
            &connection(),
            "7",
            &[],
            &mut ContractReferences::default(),
        );
        assert_eq!(shape["hub_identifier"], "music.recentlyplayed.ref1");
        assert_eq!(shape["key_route"], "section.all");
        assert_eq!(shape["query"]["type"], "8");
        assert_eq!(shape["query"]["sort"], "lastViewedAt:desc");
        assert_eq!(shape["omitted_parameters"], 2);
        assert_eq!(shape["item_types"]["artist"], 1);
        assert_eq!(shape["date_fields"]["lastViewedAt"]["valid_numeric"], 1);
        assert_eq!(shape["date_fields"]["viewedAt"]["valid_numeric"], 0);
        let serialized = shape.to_string();
        for private in [
            "Private",
            "987654",
            "876543",
            "1700000011",
            "accountID",
            "genre",
            "/library/",
            "http:",
        ] {
            assert!(!serialized.contains(private));
        }
        // Recognition is intentionally unchanged while the real contract is missing.
        assert_eq!(recent_family(&hub, "7", &[]), None);
        let response = json!({"MediaContainer":{"librarySectionID":7,"Hub":[hub]}});
        let parsed = parse_hubs(&response, &connection(), "7", &[]).unwrap();
        assert_eq!(parsed.hubs[0].items[0].kind, "artist");
        assert_eq!(parsed.hubs[0].recent_family, None);
    }
    #[test]
    fn hub_contract_collection_redacts_unknown_ids_credentials_and_bounds_metadata() {
        let mut hub = json!({"hubIdentifier":"music.PrivatePerson.7","context":"hub.music.fixture-secret","type":"album","key":"/library/sections/7/all?sort=lastViewedAt:desc&X-Plex-Token=fixture-secret","Metadata":(0..100).map(|i|json!({"type":"artist","ratingKey":i.to_string(),"title":"Private Person","lastViewedAt":1700000000})).collect::<Vec<_>>()});
        let shape = hub_contract_shape(
            &hub,
            0,
            &connection(),
            "7",
            &["fixture-secret"],
            &mut ContractReferences::default(),
        );
        assert_eq!(shape["hub_identifier"], "music.{omitted}.ref1");
        assert_eq!(shape["omitted_label_parts"]["hub_identifier"], 1);
        assert!(shape["context"].is_null());
        assert_eq!(shape["key_route"], "missing_or_redacted");
        assert_eq!(shape["key_structure_omitted"], true);
        assert_eq!(shape["query_complete"], false);
        assert_eq!(shape["items_observed"], 12);
        assert_eq!(
            shape["hub_type"], "album",
            "mixed declarations are observed without assuming artist type"
        );
        assert!(shape["query"].as_object().unwrap().is_empty());
        let serialized = shape.to_string();
        for private in ["fixture-secret", "Private", "1700000000", "X-Plex-Token"] {
            assert!(!serialized.contains(private));
        }
        hub["key"] = json!("https://private.invalid/path?sort=lastViewedAt:desc");
        assert_eq!(
            hub_contract_shape(
                &hub,
                0,
                &connection(),
                "7",
                &[],
                &mut ContractReferences::default()
            )["key_route"],
            "other"
        );
        hub["Metadata"] = json!([{ "type":"album" }]);
        assert_eq!(
            hub_contract_shape(
                &hub,
                0,
                &connection(),
                "7",
                &[],
                &mut ContractReferences::default()
            )["item_types"]["album"],
            1
        );
    }
    #[test]
    fn hub_contract_collection_rate_and_retained_source_keys_are_bounded() {
        let now = std::time::Instant::now();
        let mut budget = ContractCollectionBudget::default();
        assert_eq!(budget.admit("server-a", "7", now), Some(1));
        assert_eq!(budget.admit("server-a", "7", now), None);
        for id in 8..15 {
            assert!(budget.admit("server-a", &id.to_string(), now).is_some());
        }
        assert_eq!(budget.sources.len(), 8);
        assert_eq!(budget.admit("server-b", "7", now), None);
        assert_eq!(
            budget.admit("server-b", "7", now + std::time::Duration::from_secs(60)),
            Some(1)
        );
        assert_eq!(budget.sources.len(), 1);
    }
    #[test]
    fn discover_recent_request_is_album_only_date_sorted_and_bounded_separately() {
        let path = recent_path("7", 20).unwrap();
        assert_eq!(path, "/library/sections/7/all?type=9&sort=addedAt%3Adesc&limit=20&X-Plex-Container-Start=0&X-Plex-Container-Size=20");
        for section in ["", "../7", "7?token=secret", "-7", "7/1"] {
            assert!(recent_path(section, 20).is_err());
        }
        assert_eq!(MAX_ITEMS, 12, "ordinary hubs are unchanged");
        assert_eq!(MAX_HERO_CANDIDATES, 20);
    }
    #[test]
    fn discover_recent_window_grows_only_for_the_hero_and_rejects_out_of_bounds_requests() {
        for window in [20, 21, 32, 44, 116, 140] {
            let path = recent_path("7", window).unwrap();
            assert!(path.contains(&format!("limit={window}&")));
            assert!(path.ends_with(&format!("X-Plex-Container-Size={window}")));
            assert!(path.contains("X-Plex-Container-Start=0"));
        }
        for window in [0, 10, 19, 141, usize::MAX] {
            assert!(recent_path("7", window).is_err());
            assert!(parse_recent(
                &json!({"MediaContainer":{"size":0}}),
                &connection(),
                "7",
                "en",
                &[],
                window
            )
            .is_err());
        }
        let records: Vec<_> = (0..160)
            .map(|i| {
                let mut value = album(&(100 + i).to_string());
                value["addedAt"] = json!(1000 - i);
                value
            })
            .collect();
        let response = json!({"MediaContainer":{"librarySectionID":7,"Metadata":records}});
        let parsed = parse_recent(&response, &connection(), "7", "en", &[], 140).unwrap();
        assert_eq!(parsed.items.len(), 140);
        assert!(parsed.partial);
        assert_eq!(MAX_ITEMS, 12);
        assert_eq!(MAX_BODY, 2 * 1024 * 1024);
    }
    #[test]
    fn discover_recent_pool_preserves_twenty_album_origins_and_expected_pagination() {
        let records: Vec<_> = (0..20)
            .map(|i| {
                let mut v = album(&(100 + i).to_string());
                v["addedAt"] = json!(1000 - i);
                v
            })
            .collect();
        let json = json!({"MediaContainer":{"offset":0,"size":20,"totalSize":5000,"librarySectionID":7,"Metadata":records}});
        let parsed = parse_recent(&json, &connection(), "7", "pt-BR", &[], 20).unwrap();
        assert_eq!(parsed.items.len(), 20);
        assert!(!parsed.partial, "a catalog larger than20 is not failure");
        assert!(!parsed.dates_missing);
        assert_eq!(parsed.language, "pt-BR");
        assert_eq!(
            parsed.items.last().unwrap().rating_key,
            "119",
            "candidates13-20 exist"
        );
        assert!(parsed.items.iter().all(|i| i.server_id == "server-a"
            && i.library_id == "7"
            && i.kind == "album"
            && i.album.as_ref().unwrap().rating_key == i.rating_key));
        let mut unbounded = json.clone();
        unbounded["MediaContainer"]["Metadata"]
            .as_array_mut()
            .unwrap()
            .push(album("999"));
        assert_eq!(
            parse_recent(&unbounded, &connection(), "7", "en", &[], 20)
                .unwrap()
                .items
                .len(),
            20
        );
    }
    #[test]
    fn discover_recent_empty_partial_dates_membership_and_failures_are_explicit() {
        let parse = |j| parse_recent(&j, &connection(), "7", "en", &["fixture-secret"], 20);
        assert!(parse(json!({"MediaContainer":{"size":0}}))
            .unwrap()
            .items
            .is_empty());
        let mut dated = album("12");
        dated["addedAt"] = json!(300);
        let mut foreign = album("13");
        foreign["librarySectionID"] = json!(8);
        let mut track = album("14");
        track["type"] = json!("track");
        track["parentRatingKey"] = json!("20");
        let mut secret = album("15");
        secret["title"] = json!("fixture-secret");
        let partial =
            parse(json!({"MediaContainer":{"Metadata":[album("16"),foreign,track,secret,dated]}}))
                .unwrap();
        assert_eq!(partial.items.len(), 2);
        assert!(partial.partial);
        assert!(partial.dates_missing);
        assert_eq!(partial.items[0].rating_key, "12");
        assert_eq!(partial.items[1].added_at, None);
        for j in [
            json!({"MediaContainer":{"Metadata":null}}),
            json!({"MediaContainer":{"offset":20,"Metadata":[]}}),
            json!({"MediaContainer":{"librarySectionID":8,"Metadata":[]}}),
            json!({"MediaContainer":{"machineIdentifier":"other-server","Metadata":[]}}),
            json!({"MediaContainer":{"Metadata":[{"type":"artist","ratingKey":"123","title":"Artist"}]}}),
        ] {
            assert!(
                parse(j).is_err(),
                "incompatible response is not empty success"
            );
        }
        let short =
            parse(json!({"MediaContainer":{"totalSize":20,"Metadata":[album("12")]}})).unwrap();
        assert!(short.partial);
    }
    #[test]
    fn discover_recent_families_require_the_whole_section_date_contract() {
        // Synthetic opaque identifiers intentionally do not assume music IDs.
        let hub = json!({"hubIdentifier":"opaque-server-id", "context":"opaque-context", "title":"Arbitrary localized title", "type":"album", "key":"/library/sections/7/all?sort=addedAt%3Adesc&type=9"});
        assert_eq!(recent_family(&hub, "7", &[]), Some("added"));
        let official = json!({"type":"album", "hubIdentifier":"home.music.recent", "context":"hub.home.music.recent", "key":"/hubs/home/recentlyAdded?type=8"});
        assert_eq!(recent_family(&official, "7", &[]), Some("added"));
        let mut unknown = official.clone();
        unknown["hubIdentifier"] = json!("custom.collection");
        assert_eq!(recent_family(&unknown, "7", &[]), None);
        let mut played = hub.clone();
        played["key"] =
            json!("/library/sections/7/all?sort=lastViewedAt:desc&unwatched=0&viewOffset=0");
        assert_eq!(recent_family(&played, "7", &[]), Some("played"));
        for key in [
            "/library/sections/8/all?sort=addedAt:desc",
            "/library/collections/12/children?sort=addedAt:desc",
            "https://remote.invalid/library/sections/7/all?sort=addedAt:desc",
            "/library/sections/7/all?sort=addedAt:desc&genre=12",
            "/library/sections/7/all?sort=addedAt:desc&sort=lastViewedAt:desc",
            "/library/sections/7/all?sort=addedAt:asc",
            "/library/sections/7/all?sort=addedAt:desc&X-Plex-Token=fixture-secret",
            "/library/sections/7/all?sort=addedAt:desc&unwatched=0",
        ] {
            let mut custom = hub.clone();
            custom["key"] = json!(key);
            assert_eq!(recent_family(&custom, "7", &["fixture-secret"]), None);
        }
        let mut custom = hub.clone();
        custom["type"] = json!("collection");
        assert_eq!(recent_family(&custom, "7", &[]), None);
        custom["type"] = json!("album");
        custom["key"] = Value::Null;
        assert_eq!(recent_family(&custom, "7", &[]), None);
    }
    #[test]
    fn discover_dates_are_optional_numeric_epoch_fields_without_substitution() {
        let mut value = page(vec![album("12")]);
        let h = &mut value["MediaContainer"]["Hub"][0];
        h["type"] = json!("album");
        h["key"] = json!("/library/sections/7/all?type=9&sort=addedAt:desc");
        h["Metadata"][0]["addedAt"] = json!(1476942219u64);
        h["Metadata"][0]["lastViewedAt"] = json!(1484680617u64);
        let parsed = parse_hubs(&value, &connection(), "7", &[]).unwrap();
        assert_eq!(parsed.hubs[0].recent_family, Some("added"));
        assert_eq!(parsed.hubs[0].items[0].added_at, Some(1476942219));
        assert_eq!(parsed.hubs[0].items[0].last_viewed_at, Some(1484680617));
        for n in [
            Value::Null,
            json!(0),
            json!(-1),
            json!(1.5),
            json!("1484680617"),
            json!(u64::MAX),
        ] {
            assert_eq!(timestamp(&json!({"addedAt":n}), "addedAt"), None);
        }
        assert_eq!(timestamp(&json!({"updatedAt":1476942219}), "addedAt"), None);
    }
    #[test]
    fn discover_hubs_preserve_server_titles_identity_and_never_follow_keys() {
        let mut value = page(vec![album("12")]);
        value["MediaContainer"]["Hub"][0]["key"] =
            json!("https://untrusted.invalid/?X-Plex-Token=secret");
        let result = parse_hubs(&value, &connection(), "7", &["fixture-secret"]).unwrap();
        assert_eq!(result.hubs[0].title, "Server's real row title");
        assert_eq!(result.server_id, "server-a");
        assert_eq!(result.library_id, "7");
        assert_eq!(
            result.hubs[0].items[0].album.as_ref().unwrap().rating_key,
            "12"
        );
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("untrusted"));
    }
    #[test]
    fn discover_partial_rows_keep_valid_items_and_reject_foreign_libraries() {
        let mut other = album("13");
        other["librarySectionID"] = json!("8");
        let mut value = page(vec![
            album("12"),
            other,
            json!({"type":"movie","ratingKey":"14","title":"Other"}),
        ]);
        value["MediaContainer"]["Hub"]
            .as_array_mut()
            .unwrap()
            .push(json!({"hubIdentifier":"broken","title":"Broken","Metadata":null}));
        let result = parse_hubs(&value, &connection(), "7", &[]).unwrap();
        assert_eq!(result.hubs.len(), 1);
        assert_eq!(result.hubs[0].items.len(), 1);
        assert!(result.partial);
        value["MediaContainer"]["librarySectionID"] = json!(8);
        assert!(parse_hubs(&value, &connection(), "7", &[]).is_err());
        value["MediaContainer"]["librarySectionID"] = json!(7);
        value["MediaContainer"]["machineIdentifier"] = json!("server-b");
        assert!(parse_hubs(&value, &connection(), "7", &[]).is_err());
    }
    #[test]
    fn discover_bounds_deduplication_and_empty_are_explicit() {
        let value = page((1..=100).map(|id| album(&id.to_string())).collect());
        let result = parse_hubs(&value, &connection(), "7", &[]).unwrap();
        assert_eq!(result.hubs[0].items.len(), MAX_ITEMS);
        assert!(result.partial);
        let value = page(vec![album("12"), album("12")]);
        assert_eq!(
            parse_hubs(&value, &connection(), "7", &[]).unwrap().hubs[0]
                .items
                .len(),
            1
        );
        assert!(parse_hubs(
            &json!({"MediaContainer":{"size":0}}),
            &connection(),
            "7",
            &[]
        )
        .unwrap()
        .hubs
        .is_empty());
        assert!(parse_hubs(&json!({"MediaContainer":{}}), &connection(), "7", &[]).is_err());
        let mut body = vec![0; MAX_BODY];
        assert!(append_body(&mut body, &[1]).is_err());
        assert_eq!(body.len(), MAX_BODY);
    }
    #[test]
    fn discover_credentials_paths_and_item_membership_are_rejected() {
        let mut value = album("12");
        value["title"] = json!("fixture-secret");
        assert!(parse_item(&value, &connection(), "7", false, &["fixture-secret"]).is_none());
        value = album("12");
        value["thumb"] = json!("https://foreign.invalid/file");
        assert!(parse_item(&value, &connection(), "7", false, &[])
            .unwrap()
            .thumb
            .is_none());
        for path in [
            "/library/%2e%2e/status",
            "/library/metadata/12/thumb/%66ixture-secret",
            "/library/metadata/12/thumb/1%3fprivate",
        ] {
            value["thumb"] = json!(path);
            assert!(
                parse_item(&value, &connection(), "7", false, &["fixture-secret"])
                    .unwrap()
                    .thumb
                    .is_none()
            );
        }
        value["ratingKey"] = json!("../unsafe");
        assert!(parse_item(&value, &connection(), "7", false, &[]).is_none());
        value = album("12");
        value.as_object_mut().unwrap().remove("librarySectionID");
        assert!(parse_item(&value, &connection(), "7", true, &[]).is_none());
        assert_eq!(
            request_error(PlexError {
                operation: "fixture",
                kind: PlexErrorKind::Unauthorized
            }),
            "PLEX_DISCOVER_AUTH"
        );
        assert!(!valid_id("7?X-Plex-Token=secret"));
        assert!(!valid_id("-1"));
    }
    #[test]
    fn discover_track_navigation_targets_parent_album_not_automatic_playback() {
        let value = json!({"type":"track","ratingKey":"99","title":"Song","parentRatingKey":"12","parentTitle":"Album","grandparentTitle":"Artist","librarySectionID":"7"});
        let item = parse_item(&value, &connection(), "7", true, &[]).unwrap();
        assert_eq!(item.kind, "track");
        assert_eq!(item.album.unwrap().rating_key, "12");
    }
    #[test]
    fn discover_errors_and_all_invalid_rows_are_never_reported_as_empty() {
        for (status, expected) in [
            (401, "PLEX_DISCOVER_AUTH"),
            (403, "PLEX_DISCOVER_AUTH"),
            (404, "PLEX_DISCOVER_UNAVAILABLE"),
            (503, "PLEX_DISCOVER_UNAVAILABLE"),
        ] {
            assert_eq!(
                request_error(PlexError::from_status(
                    "fixture",
                    StatusCode::from_u16(status).unwrap()
                )),
                expected
            );
        }
        for kind in [PlexErrorKind::Transport, PlexErrorKind::Timeout] {
            assert_eq!(
                request_error(PlexError {
                    operation: "fixture",
                    kind
                }),
                "PLEX_DISCOVER_UNAVAILABLE"
            );
        }
        assert!(parse_hubs(
            &page(vec![
                json!({"type":"album","ratingKey":"../foreign","title":"Bad"})
            ]),
            &connection(),
            "7",
            &[]
        )
        .is_err());
        let mut secret = album("12");
        secret["title"] = json!("%66ixture-secret");
        assert!(parse_item(&secret, &connection(), "7", false, &["fixture-secret"]).is_none());
        let many = json!({"MediaContainer":{"Hub":(0..100).map(|i|json!({"hubIdentifier":format!("row.{i}"),"title":"Synthetic row","Metadata":[album("12")]})).collect::<Vec<_>>()}});
        let result = parse_hubs(&many, &connection(), "7", &[]).unwrap();
        assert_eq!(result.hubs.len(), MAX_HUBS);
        assert!(result.partial);
        let children =
            json!({"Metadata":(0..100).map(|i|album(&i.to_string())).collect::<Vec<_>>()});
        let (items, partial) =
            items(&children, &connection(), "7", false, &[], MAX_CHILDREN).unwrap();
        assert_eq!(items.len(), MAX_CHILDREN);
        assert!(partial);
    }
    #[test]
    fn discover_language_header_is_used_and_invalid_language_cannot_build_a_request() {
        let http = reqwest::Client::new();
        for language in ["pt-BR", "en"] {
            let request =
                discover_request(&http, &connection(), "/hubs/sections/7?count=12", language)
                    .unwrap()
                    .build()
                    .unwrap();
            assert_eq!(request.headers()["X-Plex-Language"], language);
            assert_eq!(
                request.headers()["X-Plex-Client-Identifier"],
                PLEX_CLIENT_ID
            );
            assert_eq!(request.headers()["Accept"], "application/json");
            assert!(!request.url().as_str().contains("fixture-secret"));
        }
        assert!(discover_request(
            &http,
            &connection(),
            "/hubs/sections/7",
            "pt-BR\r\nX-Plex-Token: secret"
        )
        .is_err());
    }
    #[test]
    fn discover_request_failure_preserves_stage_category_status_without_raw_data() {
        for (status, category) in [
            (401, "authentication"),
            (403, "permission"),
            (404, "http"),
            (405, "unsupported"),
            (429, "http"),
            (500, "http"),
            (501, "unsupported"),
        ] {
            let failure = PlexDiscoverFailure::from_error(
                "hubs",
                PlexError::from_status("fixture", StatusCode::from_u16(status).unwrap()),
            );
            assert_eq!(failure.stage, "hubs");
            assert_eq!(failure.category, category);
            assert_eq!(failure.status, Some(status));
            let value = serde_json::to_value(&failure).unwrap();
            assert_eq!(value.as_object().unwrap().len(), 3);
        }
        for (kind, category) in [
            (PlexErrorKind::Timeout, "timeout"),
            (PlexErrorKind::Transport, "transport"),
            (PlexErrorKind::IdentityMismatch, "identity"),
            (PlexErrorKind::ConfigurationChanged, "context"),
        ] {
            assert_eq!(
                PlexDiscoverFailure::from_error(
                    "recent",
                    PlexError {
                        operation: "fixture-secret",
                        kind
                    }
                )
                .category,
                category
            );
        }
        let failure = PlexDiscoverFailure::new("recent", "response").with_status(Some(200));
        assert_eq!(failure.status, Some(200));
        let text = serde_json::to_string(&failure).unwrap();
        assert!(!text.contains("fixture-secret"));
        assert!(!text.contains("operation"));
    }
    #[test]
    fn discover_shared_response_scope_rejects_foreign_identity_membership_and_invalid_body() {
        for (value, category) in [
            (
                json!({"MediaContainer":{"machineIdentifier":"server-b","size":0}}),
                "identity",
            ),
            (
                json!({"MediaContainer":{"librarySectionID":8,"size":0}}),
                "membership",
            ),
            (json!({"MediaContainer":null}), "response"),
        ] {
            let failure = scoped_container(&value, "server-a", "7", "hubs").unwrap_err();
            assert_eq!(failure.stage, "hubs");
            assert_eq!(failure.category, category);
        }
        assert!(scoped_container(
            &json!({"MediaContainer":{"size":0}}),
            "server-a",
            "7",
            "recent"
        )
        .is_ok());
    }
    #[test]
    fn discover_hub_context_is_display_contract_data_and_never_a_request_target() {
        let mut value = page(vec![album("12")]);
        value["MediaContainer"]["Hub"][0]["context"] = json!("hub.home.music.recent");
        let parsed = parse_hubs(&value, &connection(), "7", &[]).unwrap();
        assert_eq!(
            parsed.hubs[0].context.as_deref(),
            Some("hub.home.music.recent")
        );
        value["MediaContainer"]["Hub"][0]["context"] = json!("fixture-secret");
        assert_eq!(
            parse_hubs(&value, &connection(), "7", &["fixture-secret"])
                .unwrap()
                .hubs[0]
                .context,
            None
        );
    }
}
