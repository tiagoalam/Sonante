//! On-demand, read-only OPRA catalog access for the Linux EQ browser.
//!
//! The full catalog is fetched only on first use or explicit refresh, from a
//! fixed HTTPS origin. A validated snapshot is kept in a private app cache;
//! profile records are displayed with their OPRA/source attribution but are
//! never converted, saved, or applied by this module.

use crate::opra_catalog::{self, Catalog, OpraFilterType, MAX_CATALOG_BYTES};
use crate::opra_compat::{self, Verdict};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

const CATALOG_URL: &str =
    "https://raw.githubusercontent.com/opra-project/OPRA/main/dist/database_v1.jsonl";
const CACHE_DIR: &str = "opra-catalog-v1";
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const MAX_QUERY_BYTES: usize = 160;
const MAX_PRODUCTS: usize = 30;
const MAX_PROFILES: usize = 80;

#[derive(Clone)]
struct Snapshot {
    catalog: Arc<Catalog>,
    sha256: String,
}

static SNAPSHOT: OnceLock<Mutex<Option<Snapshot>>> = OnceLock::new();

#[derive(Debug, Serialize)]
pub(crate) struct CatalogSearch {
    pub snapshot_sha256: String,
    pub dataset_license: &'static str,
    pub products: Vec<ProductHit>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct ProductHit {
    pub product_id: String,
    pub vendor_id: Option<String>,
    pub product_name: String,
    pub vendor_name: Option<String>,
    pub product_type: &'static str,
    pub subtype: &'static str,
    pub profile_count: usize,
    pub profiles: Vec<ProfileHit>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ProfileHit {
    pub profile_id: String,
    pub author: String,
    pub details: Option<String>,
    /// Untrusted metadata only. The UI renders this as text and never opens it.
    pub source_link: Option<String>,
    pub overall_gain_db: f64,
    pub bands: Vec<BandHit>,
    pub compatibility: &'static str,
    pub compatibility_reasons: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BandHit {
    pub filter_type: &'static str,
    pub frequency: f64,
    pub gain_db: Option<f64>,
    pub q: Option<f64>,
    pub slope: Option<u8>,
}

#[tauri::command]
pub(crate) async fn search_opra_profiles(
    query: String,
    refresh: bool,
    app: AppHandle,
) -> Result<CatalogSearch, String> {
    let query = query.trim();
    if query.len() < 2 || query.len() > MAX_QUERY_BYTES || query.chars().any(char::is_control) {
        return Err("OPRA_QUERY_INVALID".into());
    }
    let snapshot = load_snapshot(&app, refresh).await?;
    Ok(search(&snapshot, query))
}

async fn load_snapshot(app: &AppHandle, refresh: bool) -> Result<Snapshot, String> {
    let cache = SNAPSHOT.get_or_init(|| Mutex::new(None));
    let mut cached = cache.lock().await;
    if !refresh {
        if let Some(value) = cached.as_ref() {
            return Ok(value.clone());
        }
        let path = cache_file(app)?;
        match read_cache(&path) {
            Ok(Some(bytes)) => {
                if let Some(value) = parse_cached_snapshot(Some(&bytes)) {
                    *cached = Some(value.clone());
                    return Ok(value);
                }
                // Keep the invalid file untouched until a complete replacement
                // has been fetched, parsed, and atomically written below.
            }
            Ok(None) => {}
            Err(error) => return Err(error),
        }
    }

    refresh_snapshot(&mut cached, fetch_catalog, |bytes| {
        write_cache(&cache_file(app)?, bytes)
    })
    .await
}

fn parse_cached_snapshot(bytes: Option<&[u8]>) -> Option<Snapshot> {
    bytes.and_then(|bytes| parse_snapshot(bytes).ok())
}

fn catalog_status_error(status: reqwest::StatusCode) -> Option<&'static str> {
    if status == reqwest::StatusCode::OK {
        None
    } else if status.is_redirection() {
        Some("OPRA_REDIRECT_REJECTED")
    } else if status.is_success() {
        Some("OPRA_CATALOG_INCOMPLETE")
    } else {
        Some("OPRA_CATALOG_UNAVAILABLE")
    }
}

fn catalog_size_error(size: u64) -> Option<&'static str> {
    (size > MAX_CATALOG_BYTES as u64).then_some("OPRA_CATALOG_TOO_LARGE")
}

fn catalog_body_error(bytes: &[u8]) -> Option<&'static str> {
    catalog_size_error(bytes.len() as u64)
        .or_else(|| bytes.is_empty().then_some("OPRA_CATALOG_EMPTY"))
}

fn network_error(is_timeout: bool) -> &'static str {
    if is_timeout {
        "OPRA_NETWORK_TIMEOUT"
    } else {
        "OPRA_NETWORK_UNAVAILABLE"
    }
}

fn parse_snapshot(bytes: &[u8]) -> Result<Snapshot, String> {
    let catalog = opra_catalog::parse_catalog(bytes).map_err(|error| {
        format!(
            "{} line={} kind={:?}",
            parse_error_code(error.kind),
            error
                .line
                .map_or_else(|| "none".to_string(), |line| line.to_string()),
            error.kind
        )
    })?;
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    Ok(Snapshot {
        catalog: Arc::new(catalog),
        sha256,
    })
}

fn parse_error_code(kind: opra_catalog::ParseErrorKind) -> &'static str {
    use opra_catalog::ParseErrorKind as Kind;
    match kind {
        Kind::EmptyCatalog => "OPRA_CATALOG_EMPTY",
        Kind::CatalogTooLarge | Kind::LineTooLarge | Kind::TooManyRecords | Kind::LimitExceeded => {
            "OPRA_CATALOG_TOO_LARGE"
        }
        Kind::MalformedJson | Kind::DuplicateJsonKey | Kind::InvalidUtf8 => {
            "OPRA_CATALOG_INVALID_JSONL"
        }
        Kind::UnknownRecordType => "OPRA_CATALOG_SCHEMA_UNSUPPORTED",
        Kind::InvalidRecord => "OPRA_CATALOG_SCHEMA_INCOMPATIBLE",
        Kind::DuplicateRecordId
        | Kind::InvalidIdentifier
        | Kind::InvalidField
        | Kind::InvalidNumber
        | Kind::BrokenReference => "OPRA_CATALOG_RECORD_INVALID",
    }
}

async fn refresh_snapshot<F, Fut, W>(
    cached: &mut Option<Snapshot>,
    fetch: F,
    persist: W,
) -> Result<Snapshot, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
    W: FnOnce(&[u8]) -> Result<(), String>,
{
    let bytes = fetch().await?;
    let value = parse_snapshot(&bytes)?;
    persist(&bytes)?;
    *cached = Some(value.clone());
    Ok(value)
}

async fn fetch_catalog() -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("Sonante/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "OPRA_NETWORK_UNAVAILABLE")?;
    let mut response = client
        .get(CATALOG_URL)
        .send()
        .await
        .map_err(|error| network_error(error.is_timeout()))?;
    if let Some(error) = catalog_status_error(response.status()) {
        return Err(error.into());
    }
    if response
        .content_length()
        .is_some_and(|size| catalog_size_error(size).is_some())
    {
        return Err("OPRA_CATALOG_TOO_LARGE".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| network_error(error.is_timeout()))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_CATALOG_BYTES {
            return Err("OPRA_CATALOG_TOO_LARGE".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if let Some(error) = catalog_body_error(&bytes) {
        return Err(error.into());
    }
    Ok(bytes)
}

fn cache_file(app: &AppHandle) -> Result<PathBuf, String> {
    let root = app
        .path()
        .app_cache_dir()
        .map_err(|_| "OPRA_CACHE_UNAVAILABLE")?;
    let directory = root.join(CACHE_DIR);
    match std::fs::symlink_metadata(&directory) {
        Ok(metadata) => validate_cache_directory(&metadata)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&root).map_err(|_| "OPRA_CACHE_UNAVAILABLE")?;
            std::fs::create_dir(&directory).map_err(|_| "OPRA_CACHE_UNAVAILABLE")?;
            set_private_directory(&directory)?;
            let metadata =
                std::fs::symlink_metadata(&directory).map_err(|_| "OPRA_CACHE_UNAVAILABLE")?;
            validate_cache_directory(&metadata)?;
        }
        Err(_) => return Err("OPRA_CACHE_UNAVAILABLE".into()),
    }
    Ok(directory.join("database_v1.jsonl"))
}

fn validate_cache_directory(metadata: &std::fs::Metadata) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err("OPRA_CACHE_UNSAFE".into());
    }
    Ok(())
}

fn set_private_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| "OPRA_CACHE_UNAVAILABLE".into())
}

fn read_cache(path: &Path) -> Result<Option<Vec<u8>>, String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("OPRA_CACHE_UNAVAILABLE".into()),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > MAX_CATALOG_BYTES as u64
    {
        return Err("OPRA_CACHE_UNSAFE".into());
    }
    std::fs::read(path)
        .map(Some)
        .map_err(|_| "OPRA_CACHE_UNAVAILABLE".into())
}

fn write_cache(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    static TEMP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    if bytes.is_empty() || bytes.len() > MAX_CATALOG_BYTES {
        return Err("OPRA_CATALOG_TOO_LARGE".into());
    }
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err("OPRA_CACHE_UNSAFE".into());
        }
    }
    let parent = path.parent().ok_or("OPRA_CACHE_UNAVAILABLE")?;
    let name = format!(
        ".database-{}-{}.tmp",
        std::process::id(),
        TEMP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let temporary = parent.join(name);
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|_| "OPRA_CACHE_UNAVAILABLE")?;
        file.write_all(bytes)
            .map_err(|_| "OPRA_CACHE_UNAVAILABLE")?;
        file.sync_all().map_err(|_| "OPRA_CACHE_UNAVAILABLE")?;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| "OPRA_CACHE_UNAVAILABLE")?;
        std::fs::rename(&temporary, path).map_err(|_| "OPRA_CACHE_UNAVAILABLE")?;
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "OPRA_CACHE_UNAVAILABLE")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn search(snapshot: &Snapshot, query: &str) -> CatalogSearch {
    let normalized_query = normalize_search_text(query);
    let query_tokens = normalized_query.split_whitespace().collect::<Vec<_>>();
    let mut profiles_by_product: HashMap<&str, Vec<&opra_catalog::EqRecord>> = HashMap::new();
    for profile in snapshot.catalog.equalizers.values() {
        let Some(product_id) = profile.data.product_id.as_deref() else {
            continue;
        };
        profiles_by_product
            .entry(product_id)
            .or_default()
            .push(profile);
    }

    struct Candidate<'a> {
        score: i32,
        profile_count: usize,
        normalized_name: String,
        product: &'a opra_catalog::ProductRecord,
        vendor: Option<&'a opra_catalog::VendorRecord>,
        profiles: Vec<&'a opra_catalog::EqRecord>,
    }

    let mut candidates = Vec::new();
    for product in snapshot.catalog.products.values() {
        let vendor = product
            .data
            .vendor_id
            .as_ref()
            .and_then(|id| snapshot.catalog.vendors.get(id));
        let vendor_name = vendor
            .map(|record| record.data.name.as_str())
            .unwrap_or_default();
        let profiles = profiles_by_product
            .get(product.id.as_str())
            .cloned()
            .unwrap_or_default();
        let name_score = product_match_score(
            &query_tokens,
            &normalized_query,
            &product.data.name,
            vendor_name,
        );
        let profile_score = profiles
            .iter()
            .filter_map(|profile| {
                let metadata = format!(
                    "{} {}",
                    profile.data.author,
                    profile.data.details.as_deref().unwrap_or_default()
                );
                let normalized = normalize_search_text(&metadata);
                let tokens = normalized.split_whitespace().collect::<Vec<_>>();
                all_tokens_score(&query_tokens, &tokens).map(|score| 100 + score)
            })
            .max();
        let Some(score) = name_score.or(profile_score) else {
            continue;
        };
        candidates.push(Candidate {
            score,
            profile_count: profiles.len(),
            normalized_name: normalize_search_text(&product.data.name),
            product,
            vendor,
            profiles,
        });
    }
    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.profile_count.cmp(&a.profile_count))
            .then_with(|| a.normalized_name.cmp(&b.normalized_name))
            .then_with(|| a.product.id.cmp(&b.product.id))
    });

    let mut truncated = candidates.len() > MAX_PRODUCTS;
    let mut products = Vec::new();
    let mut profile_count = 0usize;
    for candidate in candidates.into_iter().take(MAX_PRODUCTS) {
        if profile_count >= MAX_PROFILES {
            truncated = true;
            break;
        }
        let mut profiles = Vec::new();
        for profile in &candidate.profiles {
            if profile_count >= MAX_PROFILES {
                truncated = true;
                break;
            }
            profile_count += 1;
            let assessment = opra_compat::assess_profile(&snapshot.catalog, &profile.id)
                .expect("catalog profile exists");
            let compatibility = match assessment.verdict {
                Verdict::Compatible => "compatible",
                Verdict::Incompatible => "incompatible",
                Verdict::Unverified => "unverified",
            };
            let reasons = assessment.reasons.iter().map(reason_code).collect();
            profiles.push(ProfileHit {
                profile_id: profile.id.clone(),
                author: profile.data.author.clone(),
                details: profile.data.details.clone(),
                source_link: profile.data.link.as_deref().and_then(safe_source_link),
                overall_gain_db: profile.data.parameters.gain_db,
                bands: profile
                    .data
                    .parameters
                    .bands
                    .iter()
                    .map(|band| BandHit {
                        filter_type: filter_code(band.filter_type),
                        frequency: band.frequency,
                        gain_db: band.gain_db,
                        q: band.q,
                        slope: band.slope,
                    })
                    .collect(),
                compatibility,
                compatibility_reasons: reasons,
            });
        }
        products.push(ProductHit {
            product_id: candidate.product.id.clone(),
            vendor_id: candidate.product.data.vendor_id.clone(),
            product_name: candidate.product.data.name.clone(),
            vendor_name: candidate.vendor.map(|record| record.data.name.clone()),
            product_type: "headphones",
            subtype: match candidate.product.data.subtype {
                opra_catalog::ProductSubtype::OverTheEar => "over_the_ear",
                opra_catalog::ProductSubtype::OnEar => "on_ear",
                opra_catalog::ProductSubtype::InEar => "in_ear",
                opra_catalog::ProductSubtype::Earbuds => "earbuds",
            },
            profile_count: candidate.profile_count,
            profiles,
        });
        if profile_count >= MAX_PROFILES {
            truncated = true;
            break;
        }
    }
    CatalogSearch {
        snapshot_sha256: snapshot.sha256.clone(),
        dataset_license: opra_compat::OPRA_DECLARED_DATASET_LICENSE,
        products,
        truncated,
    }
}

fn product_match_score(
    query_tokens: &[&str],
    normalized_query: &str,
    product: &str,
    vendor: &str,
) -> Option<i32> {
    if query_tokens.is_empty() {
        return None;
    }
    let normalized_product = normalize_search_text(product);
    let normalized_vendor = normalize_search_text(vendor);
    let product_tokens = normalized_product.split_whitespace().collect::<Vec<_>>();
    let vendor_tokens = normalized_vendor.split_whitespace().collect::<Vec<_>>();
    let product_only = all_tokens_score(query_tokens, &product_tokens);
    let mut combined = product_tokens.clone();
    combined.extend(vendor_tokens.iter().copied());
    let all_fields = all_tokens_score(query_tokens, &combined)?;

    if normalized_product == normalized_query {
        Some(1_200 + all_fields)
    } else if normalized_product.contains(normalized_query) {
        Some(1_100 + all_fields)
    } else if let Some(score) = product_only {
        Some(900 + score)
    } else {
        Some(700 + all_fields)
    }
}

fn all_tokens_score(query_tokens: &[&str], field_tokens: &[&str]) -> Option<i32> {
    if query_tokens.is_empty() {
        return None;
    }
    query_tokens
        .iter()
        .map(|query| {
            field_tokens
                .iter()
                .map(|candidate| {
                    if candidate == query {
                        100
                    } else if candidate.starts_with(query) || query.starts_with(candidate) {
                        70
                    } else if candidate.contains(query) || query.contains(candidate) {
                        45
                    } else {
                        0
                    }
                })
                .max()
                .filter(|score| *score > 0)
        })
        .try_fold(0, |sum, score| score.map(|value| sum + value))
}

fn normalize_search_text(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut separator = true;
    for character in value.chars().flat_map(char::to_lowercase) {
        if is_combining_mark(character) {
            continue;
        }
        if let Some(folded) = fold_search_character(character) {
            normalized.push_str(folded);
            separator = false;
        } else if character.is_alphanumeric() {
            normalized.push(character);
            separator = false;
        } else if !separator {
            normalized.push(' ');
            separator = true;
        }
    }
    normalized.trim().to_owned()
}

fn is_combining_mark(character: char) -> bool {
    matches!(character as u32, 0x0300..=0x036f | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff | 0x20d0..=0x20ff | 0xfe20..=0xfe2f)
}

fn fold_search_character(character: char) -> Option<&'static str> {
    Some(match character {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' | 'ǎ' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' | 'ǐ' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' | 'ŉ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' | 'ǒ' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' | 'ǔ' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        'þ' => "th",
        _ => return None,
    })
}

fn safe_source_link(value: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "https" | "http")
        || parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
    {
        return None;
    }
    Some(value.to_string())
}

fn filter_code(filter: OpraFilterType) -> &'static str {
    match filter {
        OpraFilterType::PeakDip => "peak_dip",
        OpraFilterType::HighShelf => "high_shelf",
        OpraFilterType::LowShelf => "low_shelf",
        OpraFilterType::LowPass => "low_pass",
        OpraFilterType::HighPass => "high_pass",
        OpraFilterType::BandPass => "band_pass",
        OpraFilterType::BandStop => "band_stop",
    }
}

fn reason_code(reason: &opra_compat::Reason) -> &'static str {
    use opra_compat::Reason::*;
    match reason {
        UnsupportedFilter { .. } => "unsupported_filter",
        TooManyBands { .. } => "too_many_bands",
        ParameterOutOfRange { .. } => "parameter_out_of_range",
        MissingRequiredParameter { .. } => "missing_required_parameter",
        MissingRequiredSlope { .. } => "missing_required_slope",
        UnmappedSlope { .. } => "slope_unmapped",
        QConventionNotSpecified { .. } => "q_convention_unverified",
        ShelfGainReferenceNotSpecified { .. } => "shelf_gain_reference_unverified",
        OverallGainRoleNotSpecified => "overall_gain_unverified",
        CandidateValidationFailed => "candidate_validation_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opra_catalog::{
        EqInfo, EqParameters, EqRecord, OpraBand, ProductInfo, ProductRecord, ProductSubtype,
        ProductType, VendorInfo, VendorRecord,
    };

    #[test]
    fn catalog_download_requires_complete_http_200_and_bounded_nonempty_body() {
        use reqwest::StatusCode;
        assert_eq!(catalog_status_error(StatusCode::OK), None);
        assert_eq!(
            catalog_status_error(StatusCode::PARTIAL_CONTENT),
            Some("OPRA_CATALOG_INCOMPLETE")
        );
        assert_eq!(
            catalog_status_error(StatusCode::FOUND),
            Some("OPRA_REDIRECT_REJECTED")
        );
        assert_eq!(
            catalog_status_error(StatusCode::NOT_FOUND),
            Some("OPRA_CATALOG_UNAVAILABLE")
        );
        assert_eq!(
            catalog_status_error(StatusCode::BAD_GATEWAY),
            Some("OPRA_CATALOG_UNAVAILABLE")
        );
        assert_eq!(catalog_size_error(MAX_CATALOG_BYTES as u64), None);
        assert_eq!(
            catalog_size_error(MAX_CATALOG_BYTES as u64 + 1),
            Some("OPRA_CATALOG_TOO_LARGE")
        );
        assert_eq!(catalog_body_error(b""), Some("OPRA_CATALOG_EMPTY"));
        assert_eq!(catalog_body_error(b"not-jsonl"), None);
        assert!(parse_snapshot(b"not-jsonl").is_err());
        assert_eq!(network_error(true), "OPRA_NETWORK_TIMEOUT");
        assert_eq!(network_error(false), "OPRA_NETWORK_UNAVAILABLE");
    }

    #[test]
    fn parser_failures_keep_sanitized_categories() {
        use opra_catalog::ParseErrorKind as Kind;
        assert_eq!(
            parse_error_code(Kind::MalformedJson),
            "OPRA_CATALOG_INVALID_JSONL"
        );
        assert_eq!(
            parse_error_code(Kind::UnknownRecordType),
            "OPRA_CATALOG_SCHEMA_UNSUPPORTED"
        );
        assert_eq!(
            parse_error_code(Kind::InvalidRecord),
            "OPRA_CATALOG_SCHEMA_INCOMPATIBLE"
        );
        assert_eq!(
            parse_error_code(Kind::InvalidIdentifier),
            "OPRA_CATALOG_RECORD_INVALID"
        );
        assert_eq!(
            parse_error_code(Kind::BrokenReference),
            "OPRA_CATALOG_RECORD_INVALID"
        );
        assert_eq!(
            parse_error_code(Kind::LimitExceeded),
            "OPRA_CATALOG_TOO_LARGE"
        );
    }

    #[tokio::test]
    async fn failed_catalog_refresh_keeps_the_previous_valid_snapshot() {
        let previous = fixture_snapshot();
        let previous_hash = previous.sha256.clone();
        let mut cached = Some(previous);
        let result = refresh_snapshot(
            &mut cached,
            || async { Err::<Vec<u8>, String>("OPRA_NETWORK_UNAVAILABLE".into()) },
            |_| panic!("cache must not be written after a failed fetch"),
        )
        .await;
        assert!(matches!(result, Err(error) if error == "OPRA_NETWORK_UNAVAILABLE"));
        assert_eq!(
            cached.as_ref().map(|snapshot| snapshot.sha256.as_str()),
            Some(previous_hash.as_str())
        );
    }

    #[tokio::test]
    async fn invalid_disk_cache_is_replaced_only_after_a_valid_complete_fetch() {
        let invalid_disk_copy = b"{broken jsonl".to_vec();
        let mut memory_cache = parse_cached_snapshot(Some(&invalid_disk_copy));
        assert!(memory_cache.is_none());
        let response = br#"{"type":"vendor","id":"maker","data":{"name":"Synthetic"}}"#.to_vec();
        let mut persisted = None;
        let loaded = refresh_snapshot(
            &mut memory_cache,
            || async { Ok(response.clone()) },
            |bytes| {
                persisted = Some(bytes.to_vec());
                Ok(())
            },
        )
        .await
        .expect("the validated replacement should load");
        assert_eq!(persisted.as_deref(), Some(response.as_slice()));
        assert_eq!(
            memory_cache.as_ref().map(|value| &value.sha256),
            Some(&loaded.sha256)
        );
        assert_eq!(invalid_disk_copy, b"{broken jsonl");

        // A failed request after detecting corruption must not delete or write
        // over the previous on-disk bytes.
        let mut persisted_after_failure = false;
        let result = refresh_snapshot(
            &mut None,
            || async { Err::<Vec<u8>, String>("OPRA_NETWORK_TIMEOUT".into()) },
            |_| {
                persisted_after_failure = true;
                Ok(())
            },
        )
        .await;
        assert!(matches!(result, Err(error) if error == "OPRA_NETWORK_TIMEOUT"));
        assert!(!persisted_after_failure);
        assert_eq!(invalid_disk_copy, b"{broken jsonl");
    }

    #[tokio::test]
    async fn failed_cache_write_does_not_replace_a_valid_memory_snapshot() {
        let previous = fixture_snapshot();
        let previous_hash = previous.sha256.clone();
        let mut cached = Some(previous);
        let valid_response =
            br#"{"type":"vendor","id":"maker","data":{"name":"Replacement"}}"#.to_vec();
        let result = refresh_snapshot(
            &mut cached,
            || async { Ok(valid_response) },
            |_| Err("OPRA_CACHE_UNAVAILABLE".into()),
        )
        .await;
        assert!(matches!(result, Err(error) if error == "OPRA_CACHE_UNAVAILABLE"));
        assert_eq!(
            cached.as_ref().map(|snapshot| snapshot.sha256.as_str()),
            Some(previous_hash.as_str())
        );
    }

    #[test]
    fn absent_or_invalid_jsonl_cache_is_treated_as_a_miss_without_mutating_input() {
        assert!(parse_cached_snapshot(None).is_none());
        assert!(parse_cached_snapshot(Some(b"{broken jsonl")).is_none());
        let raw = b"{broken jsonl";
        let _ = parse_cached_snapshot(Some(raw));
        assert_eq!(raw, b"{broken jsonl");
    }
    use std::collections::BTreeMap;

    fn fixture_snapshot() -> Snapshot {
        let vendor = VendorRecord {
            id: "maker".into(),
            line: 1,
            data: VendorInfo {
                name: "Synthetic Audio".into(),
                official_name: None,
                blurb: None,
                logo: None,
            },
        };
        let product = ProductRecord {
            id: "maker_model".into(),
            line: 2,
            data: ProductInfo {
                vendor_id: Some("maker".into()),
                name: "Model One".into(),
                blurb: None,
                line_art_svg: None,
                line_art_96x64_png: None,
                product_type: ProductType::Headphones,
                subtype: ProductSubtype::OverTheEar,
            },
        };
        let profile = EqRecord {
            id: "maker_model_author".into(),
            line: 3,
            data: EqInfo {
                product_id: Some("maker_model".into()),
                author: "Synthetic Author".into(),
                details: Some("Synthetic profile".into()),
                link: Some("https://example.invalid/profile".into()),
                eq_type: crate::opra_catalog::EqType::ParametricEq,
                parameters: EqParameters {
                    gain_db: 0.0,
                    bands: vec![],
                },
            },
        };
        let nonneutral = EqRecord {
            id: "maker_model_curve".into(),
            line: 4,
            data: EqInfo {
                product_id: Some("maker_model".into()),
                author: "Curve Author".into(),
                details: Some("Synthetic non-neutral curve".into()),
                link: None,
                eq_type: crate::opra_catalog::EqType::ParametricEq,
                parameters: EqParameters {
                    gain_db: 0.0,
                    bands: vec![OpraBand {
                        filter_type: OpraFilterType::PeakDip,
                        frequency: 1000.0,
                        gain_db: Some(2.0),
                        q: Some(1.0),
                        slope: None,
                    }],
                },
            },
        };
        let unsupported = EqRecord {
            id: "maker_model_unhandled".into(),
            line: 5,
            data: EqInfo {
                product_id: Some("maker_model".into()),
                author: "Other Author".into(),
                details: None,
                link: None,
                eq_type: crate::opra_catalog::EqType::ParametricEq,
                parameters: EqParameters {
                    gain_db: 0.0,
                    bands: vec![OpraBand {
                        filter_type: OpraFilterType::BandPass,
                        frequency: 1000.0,
                        gain_db: None,
                        q: Some(1.0),
                        slope: None,
                    }],
                },
            },
        };
        let catalog = Catalog {
            vendors: BTreeMap::from([(vendor.id.clone(), vendor)]),
            products: BTreeMap::from([(product.id.clone(), product)]),
            equalizers: BTreeMap::from([
                (profile.id.clone(), profile),
                (nonneutral.id.clone(), nonneutral),
                (unsupported.id.clone(), unsupported),
            ]),
        };
        Snapshot {
            catalog: Arc::new(catalog),
            sha256: "synthetic-sha256".into(),
        }
    }

    #[test]
    fn search_keeps_product_profile_provenance_and_declared_license() {
        let result = search(&fixture_snapshot(), "synthetic");
        assert_eq!(result.snapshot_sha256, "synthetic-sha256");
        assert_eq!(result.dataset_license, "CC-BY-SA-4.0");
        assert_eq!(result.products.len(), 1);
        assert_eq!(
            result.products[0].vendor_name.as_deref(),
            Some("Synthetic Audio")
        );
        assert_eq!(result.products[0].profiles[0].author, "Synthetic Author");
        assert_eq!(
            result.products[0].profiles[0].source_link.as_deref(),
            Some("https://example.invalid/profile")
        );
        assert_eq!(result.products[0].profiles[0].compatibility, "compatible");
        assert_eq!(result.products[0].profiles[1].compatibility, "unverified");
        assert_eq!(result.products[0].profiles[1].bands[0].gain_db, Some(2.0));
        assert_eq!(result.products[0].profiles[2].compatibility, "incompatible");
        assert_eq!(
            result.products[0].profiles[2].compatibility_reasons,
            vec!["unsupported_filter"]
        );
        assert_eq!(result.products[0].profile_count, 3);
    }

    fn headphone_search_fixture() -> Snapshot {
        let mut snapshot = fixture_snapshot();
        let mut catalog = (*snapshot.catalog).clone();
        catalog.vendors.insert(
            "hifiman".into(),
            VendorRecord {
                id: "hifiman".into(),
                line: 10,
                data: VendorInfo {
                    name: "HIFIMAN".into(),
                    official_name: None,
                    blurb: None,
                    logo: None,
                },
            },
        );
        for (id, name) in [
            ("hifiman_edition_xs", "Edition XS"),
            ("hifiman_he6", "HE-6"),
        ] {
            catalog.products.insert(
                id.into(),
                ProductRecord {
                    id: id.into(),
                    line: 11,
                    data: ProductInfo {
                        vendor_id: Some("hifiman".into()),
                        name: name.into(),
                        blurb: None,
                        line_art_svg: None,
                        line_art_96x64_png: None,
                        product_type: ProductType::Headphones,
                        subtype: ProductSubtype::OverTheEar,
                    },
                },
            );
        }
        for index in 0..35 {
            let product_id = format!("hifiman_model_{index:02}");
            catalog.products.insert(
                product_id.clone(),
                ProductRecord {
                    id: product_id.clone(),
                    line: 30 + index,
                    data: ProductInfo {
                        vendor_id: Some("hifiman".into()),
                        name: format!("Model {index:02}"),
                        blurb: None,
                        line_art_svg: None,
                        line_art_96x64_png: None,
                        product_type: ProductType::Headphones,
                        subtype: ProductSubtype::OverTheEar,
                    },
                },
            );
            let profile_id = format!("hifiman_model_profile_{index:02}");
            catalog.equalizers.insert(
                profile_id.clone(),
                EqRecord {
                    id: profile_id,
                    line: 70 + index,
                    data: EqInfo {
                        product_id: Some(product_id),
                        author: "Fixture Author".into(),
                        details: None,
                        link: None,
                        eq_type: crate::opra_catalog::EqType::ParametricEq,
                        parameters: EqParameters {
                            gain_db: 0.0,
                            bands: vec![],
                        },
                    },
                },
            );
        }
        for index in 0..3 {
            let id = format!("edition_xs_profile_{index}");
            catalog.equalizers.insert(
                id.clone(),
                EqRecord {
                    id,
                    line: 12 + index,
                    data: EqInfo {
                        product_id: Some("hifiman_edition_xs".into()),
                        author: format!("Contributor {index}"),
                        details: None,
                        link: None,
                        eq_type: crate::opra_catalog::EqType::ParametricEq,
                        parameters: EqParameters {
                            gain_db: 0.0,
                            bands: vec![],
                        },
                    },
                },
            );
        }
        catalog.equalizers.insert(
            "he6_unrelated_details".into(),
            EqRecord {
                id: "he6_unrelated_details".into(),
                line: 20,
                data: EqInfo {
                    product_id: Some("hifiman_he6".into()),
                    author: "Another Contributor".into(),
                    details: Some("HIFIMAN Edition XS mention only".into()),
                    link: None,
                    eq_type: crate::opra_catalog::EqType::ParametricEq,
                    parameters: EqParameters {
                        gain_db: 0.0,
                        bands: vec![],
                    },
                },
            },
        );
        snapshot.catalog = Arc::new(catalog);
        snapshot
    }

    #[test]
    fn headphone_search_matches_tokens_across_vendor_and_model_and_ranks_model_first() {
        let snapshot = headphone_search_fixture();
        for query in ["HIFIMAN", "Edition XS", "HIFIMAN Edition XS", "edítion-xs"] {
            let result = search(&snapshot, query);
            assert!(!result.products.is_empty(), "query {query:?} should match");
            assert_eq!(
                result.products[0].product_id,
                "hifiman_edition_xs",
                "the actual product name match must outrank a profile whose notes merely mention the query"
            );
        }
        let manufacturer = search(&snapshot, "Hifiman");
        assert!(
            manufacturer.truncated,
            "the manufacturer fixture exceeds the bounded result page"
        );
        assert_eq!(manufacturer.products[0].product_id, "hifiman_edition_xs");
        let edition = manufacturer
            .products
            .iter()
            .find(|product| product.product_id == "hifiman_edition_xs")
            .expect("Edition XS is visible in manufacturer results");
        assert_eq!(edition.profile_count, 3);
        assert_eq!(normalize_search_text("Édition-XS"), "edition xs");
    }

    #[test]
    fn profile_urls_are_only_exposed_as_inert_http_or_https_metadata() {
        assert_eq!(
            safe_source_link("http://example.invalid/path").as_deref(),
            Some("http://example.invalid/path")
        );
        assert!(safe_source_link("javascript:alert(1)").is_none());
        assert!(safe_source_link("https://user:password@example.invalid/path").is_none());
        assert_eq!(
            safe_source_link("https://example.invalid/path").as_deref(),
            Some("https://example.invalid/path")
        );
    }
}
