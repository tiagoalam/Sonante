//! Original Local/Plex A/B contract. Direct HTTP on explicit prepare only; no
//! MPD Part/proxy listener and no account, URL or filename exposed to WebView.
use super::{core, host::Binding, Client, Controller};
use crate::windows_shared::{ab_comparison as ab, ab_spool::Spool};
use std::{path::Path, sync::Arc, time::Duration};
struct Inventory {
    rows: Vec<(ab::Track, crate::windows_mpd::queue_v3::Source)>,
    connection: core::ResolvedPlexConnection,
}
fn parse(
    json: &serde_json::Value,
    server: &str,
    library: &str,
    album: &str,
    connection: &core::ResolvedPlexConnection,
) -> Result<Vec<(ab::Track, crate::windows_mpd::queue_v3::Source)>, String> {
    if connection.server_id != server
        || crate::windows_editions::policy::plex_profile(json, library, album).is_none()
    {
        return Err("AB_TRACKS".into());
    }
    json["MediaContainer"]["Metadata"]
        .as_array()
        .ok_or("AB_TRACKS")?
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let rating = row["ratingKey"].as_str().ok_or("AB_TRACKS")?;
            let title = row["title"]
                .as_str()
                .filter(|t| connection.ab_text_safe(t))
                .ok_or("AB_TRACKS")?;
            let part = row["Media"][0]["Part"][0]["key"]
                .as_str()
                .filter(|p| ab::valid_part_path(p))
                .ok_or("AB_TRACKS")?;
            core::content::validate_reference(server, rating, part).map_err(|_| "AB_TRACKS")?;
            Ok((
                ab::track(
                    rating,
                    title,
                    row["duration"].as_f64().map(|n| n / 1000.),
                    i + 1,
                )?,
                crate::windows_mpd::queue_v3::Source::Plex {
                    server_id: server.into(),
                    rating_key: rating.into(),
                    part_key: part.into(),
                },
            ))
        })
        .collect()
}
impl Client {
    async fn ab_inventory(
        &self,
        server: &str,
        library: &str,
        album: &str,
    ) -> Result<Inventory, String> {
        if core::validate_plex_server_id(server).is_err()
            || !core::content::valid_id(library)
            || !core::content::valid_id(album)
            || self.manager.selected_server_id().as_deref() != Some(server)
        {
            return Err("AB_CHANGED".into());
        }
        let path = format!(
            "/library/metadata/{album}/children?X-Plex-Container-Start=0&X-Plex-Container-Size=500"
        );
        let (json, connection): (serde_json::Value, _) = self
            .manager
            .request_json(&path, "preparar comparação")
            .await
            .map_err(|_| "AB_NETWORK")?;
        let rows = parse(&json, server, library, album, &connection)?;
        self.manager
            .validate_source_connection(&connection)
            .map_err(|_| "AB_CHANGED")?;
        Ok(Inventory { rows, connection })
    }
}
async fn guarded<T>(
    current: &ab::Check,
    future: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    if !current() {
        return Err("AB_CHANGED".into());
    }
    tokio::pin!(future);
    loop {
        tokio::select! {biased;
            _=tokio::time::sleep(Duration::from_millis(100))=>{if !current(){return Err("AB_CHANGED".into());}},
            result=&mut future=>{if !current(){return Err("AB_CHANGED".into());}return result;}
        }
    }
}
impl Controller {
    fn ab_binding(&self, server: &str, current: &ab::Check) -> Result<Arc<Binding>, String> {
        let binding = self.registry().selected().map_err(|_| "AB_CHANGED")?;
        if !current()
            || !binding.settings.enabled
            || binding.client.manager.selected_server_id().as_deref() != Some(server)
        {
            return Err("AB_CHANGED".into());
        }
        Ok(binding)
    }
    pub(crate) async fn ab_tracks(
        &self,
        server: &str,
        library: &str,
        album: &str,
        current: ab::Check,
    ) -> Result<Vec<ab::Track>, String> {
        let binding = self.ab_binding(server, &current)?;
        let inventory = guarded(
            &current,
            self.ab_read(binding.client.ab_inventory(server, library, album)),
        )
        .await?;
        self.registry()
            .with_current(&binding, || {
                binding
                    .client
                    .manager
                    .validate_source_connection(&inventory.connection)
                    .map_err(|_| "AB_CHANGED".into())
            })
            .map_err(|_| "AB_CHANGED")?;
        Ok(inventory.rows.into_iter().map(|(track, _)| track).collect())
    }
    /// Run this entire async function using Handle::block_on ON the owned loader
    /// worker. Private spool writes stay off the UI/Tokio workers; physical
    /// admission and DeleteOnClose survive caller cancellation until return.
    pub(crate) async fn ab_input(
        &self,
        server: &str,
        library: &str,
        album: &str,
        key: &str,
        parent: &Path,
        current: ab::Check,
    ) -> Result<ab::Input, String> {
        let binding = self.ab_binding(server, &current)?;
        let registry = self.registry();
        let work = async {
            let inventory = guarded(
                &current,
                binding.client.ab_inventory(server, library, album),
            )
            .await?;
            let source = inventory
                .rows
                .into_iter()
                .find_map(|(track, source)| (track.key == key).then_some(source))
                .ok_or("AB_CHANGED")?;
            let manager = binding.client.manager.clone();
            let connection = inventory.connection;
            let held = binding.clone();
            let policy = current.clone();
            let alive: ab::Check = Arc::new(move || {
                policy()
                    && held.active()
                    && registry
                        .with_current(&held, || {
                            manager
                                .validate_source_connection(&connection)
                                .map_err(|_| "AB_CHANGED".into())
                        })
                        .is_ok()
            });
            if !alive() {
                return Err("AB_CHANGED".into());
            }
            let http = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(3))
                .read_timeout(Duration::from_secs(20))
                .timeout(Duration::from_secs(45))
                .build()
                .map_err(|_| "AB_NETWORK")?;
            let mut response = guarded(&alive, async {
                binding
                    .client
                    .media(&http, &source, None)
                    .await
                    .map_err(|_| "AB_NETWORK".into())
            })
            .await?;
            if response.status() != reqwest::StatusCode::OK {
                return Err("AB_NETWORK".into());
            }
            if let Some(kind) = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
            {
                let kind = kind.to_ascii_lowercase();
                if kind.contains("json") || kind.contains("xml") || kind.contains("html") {
                    return Err("AB_NETWORK".into());
                }
            }
            let expected = response.content_length();
            if expected.is_some_and(|n| n > ab::MAX_SOURCE_BYTES as u64) {
                return Err("AB_LIMIT".into());
            }
            let mut spool = Spool::new(parent, &*alive)?;
            while let Some(bytes) = guarded(&alive, async {
                response.chunk().await.map_err(|_| "AB_NETWORK".into())
            })
            .await?
            {
                spool.write(&bytes, &*alive)?;
            }
            let spool = spool.seal(expected, &*alive)?;
            if !alive() {
                return Err("AB_CHANGED".into());
            }
            Ok(spool.input(alive, format!("plex:{server}:{key}")))
        };
        guarded(&current, self.ab_read(work)).await
    }
}
