//! Original opt-in Local artwork flow, with a Windows cache/cancellation boundary.
mod policy;
mod storage;
pub(crate) use policy::OnlineAlbumCoverRequest;
#[cfg(windows)]
#[path = "../windows_scrobbling/private_file.rs"]
mod private_file;
use crate::windows_mpd::library::Library;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
const ERROR: &str = "WINDOWS_ARTWORK_UNAVAILABLE";
const STALE: &str = "ARTWORK_CONTEXT_CHANGED";
const MAX_IMAGE: usize = 8 * 1024 * 1024;
const MAX_JSON: usize = 256 * 1024;
const NEGATIVE_TTL: u64 = 7 * 24 * 60 * 60;
const MUSICBRAINZ_URL: &str = "https://musicbrainz.org/ws/2/release-group/";
const COVER_ART_URL: &str = "https://coverartarchive.org/release-group";
static REQUESTS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
static RATE_WORKERS: AtomicUsize = AtomicUsize::new(0);
struct RateWorker;
impl RateWorker {
    fn acquire() -> Result<Self, String> {
        RATE_WORKERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 2).then_some(n + 1)
            })
            .map_err(|_| ERROR)?;
        Ok(Self)
    }
}
impl Drop for RateWorker {
    fn drop(&mut self) {
        RATE_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Scope {
    library: Library,
    token: String,
    revision: u64,
    enabled: bool,
    request: OnlineAlbumCoverRequest,
    live: AtomicBool,
    root: PathBuf,
}
impl Scope {
    fn current(&self) -> bool {
        self.live.load(Ordering::Acquire)
            && self
                .library
                .check_online_artwork_context(&self.token, self.revision)
                .is_ok()
    }
    fn check(&self) -> Result<(), String> {
        if self.current() {
            Ok(())
        } else {
            Err(STALE.into())
        }
    }
    fn admit(
        library: Library,
        request: OnlineAlbumCoverRequest,
        root: PathBuf,
    ) -> Result<Arc<Self>, String> {
        let (token, revision, enabled, album) = library
            .online_artwork_context(&request.album_id)
            .map_err(|_| STALE)?;
        if request.title != album.title
            || request.artist != album.artist
            || request.year != album.year.map(|n| n.to_string())
        {
            return Err(STALE.into());
        }
        Ok(Arc::new(Self {
            library,
            token,
            revision,
            enabled,
            request,
            live: AtomicBool::new(true),
            root,
        }))
    }
}
struct Owner(Arc<Scope>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.live.store(false, Ordering::Release);
    }
}
fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|n| n.as_secs())
        .map_err(|_| ERROR.into())
}
async fn cache<T: Send + 'static>(
    scope: &Arc<Scope>,
    work: impl FnOnce(&std::path::Path, &str, u64, &dyn Fn() -> bool) -> Result<T, String>
        + Send
        + 'static,
) -> Result<T, String> {
    scope.check()?;
    let s = scope.clone();
    let value = tauri::async_runtime::spawn_blocking(move || {
        crate::windows_io::read(Duration::from_secs(3), move |job| {
            let current = || job.current() && s.current();
            if !current() {
                return Err(STALE.into());
            }
            let value = work(&s.root, &s.request.album_id, now()?, &current)?;
            if !current() {
                return Err(STALE.into());
            }
            Ok(value)
        })
        .map_err(|_| ERROR.to_owned())?
    })
    .await
    .map_err(|_| ERROR)?;
    scope.check()?;
    value
}
async fn observed<T>(
    scope: &Arc<Scope>,
    work: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let cancelled = async {
        let mut ticks = tokio::time::interval(Duration::from_millis(50));
        loop {
            ticks.tick().await;
            if !scope.current() {
                break;
            }
        }
    };
    let result = tokio::select! {biased;_=cancelled=>Err(STALE.into()),result=tokio::time::timeout(Duration::from_secs(30),work)=>result.map_err(|_|ERROR.to_owned())?};
    scope.check()?;
    result
}
async fn rate(scope: &Arc<Scope>) -> Result<(), String> {
    scope.check()?;
    let permit = RateWorker::acquire()?;
    let s = scope.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _physical = permit;
        s.check()?;
        crate::online_artwork::wait_for_musicbrainz()?;
        s.check()
    })
    .await
    .map_err(|_| ERROR)??;
    scope.check()
}
async fn bounded_body(
    mut response: reqwest::Response,
    limit: usize,
    scope: &Scope,
) -> Result<Vec<u8>, String> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(ERROR.into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ERROR)? {
        scope.check()?;
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(ERROR.into());
        }
        bytes.extend_from_slice(&chunk);
    }
    scope.check()?;
    Ok(bytes)
}
async fn lookup(
    scope: &Arc<Scope>,
    request: OnlineAlbumCoverRequest,
    mb_url: &str,
    art_url: &str,
    limited: bool,
) -> Result<Option<String>, String> {
    scope.check()?;
    let client = reqwest::Client::builder()
        .user_agent(crate::online_artwork::USER_AGENT)
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| ERROR)?;
    if limited {
        rate(scope).await?;
    }
    scope.check()?;
    let query = format!(
        "releasegroup:\"{}\" AND artist:\"{}\"",
        request.title.replace(['\\', '"'], " "),
        request.artist.replace(['\\', '"'], " ")
    );
    let response = client
        .get(mb_url)
        .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "5")])
        .send()
        .await
        .map_err(|_| ERROR)?;
    if !response.status().is_success() {
        return Err(ERROR.into());
    }
    let bytes = bounded_body(response, MAX_JSON, scope).await?;
    let results: policy::SearchResponse = serde_json::from_slice(&bytes).map_err(|_| ERROR)?;
    let Some(mbid) = policy::select_release_group(
        &results,
        &request.title,
        &request.artist,
        request.year.as_deref(),
    ) else {
        cache(scope, storage::negative).await?;
        return Ok(None);
    };
    scope.check()?;
    let response = client
        .get(format!("{art_url}/{mbid}/front-500"))
        .send()
        .await
        .map_err(|_| ERROR)?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        scope.check()?;
        cache(scope, storage::negative).await?;
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(ERROR.into());
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::trim)
        .filter(|v| matches!(*v, "image/jpeg" | "image/png" | "image/webp"))
        .ok_or(ERROR)?
        .to_owned();
    let image = bounded_body(response, MAX_IMAGE, scope).await?;
    if policy::image_mime(&image) != Some(mime.as_str()) {
        return Err(ERROR.into());
    }
    scope.check()?;
    cache(scope, move |root, id, time, current| {
        storage::positive(root, id, &mbid, &mime, &image, time, current)
    })
    .await
    .map(Some)
}
async fn cover_at(
    library: Library,
    request: OnlineAlbumCoverRequest,
    only: bool,
    root: PathBuf,
    mb: &str,
    art: &str,
    limited: bool,
) -> Result<Option<String>, String> {
    let _slot = REQUESTS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| ERROR)?;
    let scope = Scope::admit(library, request, root)?;
    let _owner = Owner(scope.clone());
    if !scope.enabled {
        return Ok(None);
    }
    let Some(request) = scope.request.clone().validated()? else {
        return Ok(None);
    };
    observed(&scope, async {
        match cache(&scope, storage::read).await? {
            storage::Hit::Image(image) => return Ok(Some(image)),
            storage::Hit::Negative => return Ok(None),
            storage::Hit::Miss => {}
        }
        if only {
            return Ok(None);
        }
        lookup(&scope, request, mb, art, limited).await
    })
    .await
}
pub(crate) async fn cover(
    library: Library,
    request: OnlineAlbumCoverRequest,
    only: bool,
) -> Result<Option<String>, String> {
    cover_at(
        library,
        request,
        only,
        crate::persistence::sonante_config_dir(),
        MUSICBRAINZ_URL,
        COVER_ART_URL,
        true,
    )
    .await
}
pub(crate) async fn status(
    library: Library,
    request: OnlineAlbumCoverRequest,
) -> Result<String, String> {
    let _slot = REQUESTS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| ERROR)?;
    let scope = Scope::admit(library, request, crate::persistence::sonante_config_dir())?;
    let _owner = Owner(scope.clone());
    if scope.request.clone().validated()?.is_none() {
        return Ok("ineligible".into());
    }
    observed(&scope, async {
        Ok(match cache(&scope, storage::read).await? {
            storage::Hit::Image(_) => "positive",
            storage::Hit::Negative => "negative",
            storage::Hit::Miss => "miss",
        }
        .into())
    })
    .await
}

#[cfg(all(test,not(windows)))]mod tests;
