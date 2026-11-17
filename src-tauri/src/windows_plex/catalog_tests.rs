use super::*;
use std::sync::{Arc, Mutex};
struct FixtureClient {
    connection_manager: Arc<PlexConnectionManager>,
}
fn manager_config(url: &str, id: Option<&str>) -> super::super::super::Settings {
    super::super::super::Settings {
        enabled: true,
        plex_url: url.into(),
        plex_token: "TEST_FIXTURE_TOKEN".into(),
        plex_server_id: id.map(str::to_string),
        plex_server_name: None,
    }
}
fn media_client_for_test(settings: &super::super::super::Settings) -> FixtureClient {
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    FixtureClient {
        connection_manager: Arc::new(PlexConnectionManager::with_clients(
            settings,
            http.clone(),
            http,
            "http://127.0.0.1:9/unused".into(),
        )),
    }
}
fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}
struct MockResponse {
    status_line: &'static str,
    content_type: &'static str,
    body: String,
    delay: Duration,
}
impl MockResponse {
    fn json(status_line: &'static str, body: serde_json::Value) -> Self {
        Self {
            status_line,
            content_type: "application/json",
            body: body.to_string(),
            delay: Duration::ZERO,
        }
    }
}
fn album_catalog_page(offset: usize, total: Option<usize>, ids: &[&str]) -> serde_json::Value {
    let mut body = serde_json::json!({"MediaContainer": {"offset": offset, "size": ids.len(),
            "Metadata": ids.iter().map(|id| serde_json::json!({"ratingKey": id, "title": id})).collect::<Vec<_>>()}});
    if let Some(total) = total {
        body["MediaContainer"]["totalSize"] = serde_json::json!(total);
    }
    body
}

fn fixture_album_catalog(
    responses: Vec<MockResponse>,
    change_after: Option<usize>,
) -> (Result<PlexAlbumCatalog, PlexAlbumCatalogError>, Vec<String>) {
    let client = media_client_for_test(&manager_config(
        "http://127.0.0.1:9",
        Some("fixture-machine-id"),
    ));
    let initial = client.connection_manager.snapshot().unwrap();
    let mut responses = responses.into_iter();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let recorded = paths.clone();
    let manager = client.connection_manager.clone();
    let result = block_on(read_album_catalog(
        "1",
        "added",
        initial,
        move |path| {
            recorded.lock().unwrap().push(path);
            let response = responses
                .next()
                .expect("unexpected additional catalog request");
            let index = recorded.lock().unwrap().len();
            let snapshot = manager.snapshot().unwrap();
            let connection = ResolvedPlexConnection {
                server_id: "fixture-machine-id".to_string(),
                base_url: String::new(),
                token: String::new(),
                generation: snapshot.generation,
            };
            if change_after == Some(index) {
                let mut changed = manager_config("http://127.0.0.1:9", Some("fixture-machine-id"));
                changed.plex_token = "NEW_FIXTURE_TOKEN".into();
                manager.update_config(&changed);
            }
            std::future::ready(if response.status_line.starts_with("200 ") {
                serde_json::from_str(&response.body)
                    .map(|json| (json, connection))
                    .map_err(|_| PlexError::invalid_response("consultar os álbuns"))
            } else {
                Err(PlexError::from_status(
                    "consultar os álbuns",
                    StatusCode::from_u16(
                        response
                            .status_line
                            .split(' ')
                            .next()
                            .unwrap()
                            .parse()
                            .unwrap(),
                    )
                    .unwrap(),
                ))
            })
        },
        || client.connection_manager.snapshot(),
    ));
    let paths = paths.lock().unwrap().clone();
    (result, paths)
}

#[test]
fn album_catalog_retains_optional_added_timestamp_for_smart_rules_without_an_extra_request() {
    let mut page = album_catalog_page(0, Some(2), &["1", "2"]);
    page["MediaContainer"]["Metadata"][0]["addedAt"] = serde_json::json!(1793404800u64);
    page["MediaContainer"]["Metadata"][1]["addedAt"] = serde_json::json!(-1);
    let (result, paths) = fixture_album_catalog(vec![MockResponse::json("200 OK", page)], None);
    let result = result.unwrap();
    assert_eq!(paths.len(), 1);
    assert_eq!(result.albums[0].added_at, Some(1793404800));
    assert_eq!(result.albums[1].added_at, None);
}
#[test]
fn album_catalog_empty_and_multiple_pages_are_complete_only_after_verified_termination() {
    for pages in [
        vec![album_catalog_page(0, Some(0), &[])],
        vec![
            album_catalog_page(0, Some(3), &["1", "2"]),
            album_catalog_page(2, Some(3), &["3"]),
        ],
        vec![
            album_catalog_page(0, None, &["1"]),
            album_catalog_page(1, None, &[]),
        ],
    ] {
        let expected = pages
            .iter()
            .map(|p| p["MediaContainer"]["size"].as_u64().unwrap() as usize)
            .sum::<usize>();
        let page_count = pages.len();
        let (result, paths) = fixture_album_catalog(
            pages
                .into_iter()
                .map(|p| MockResponse::json("200 OK", p))
                .collect(),
            None,
        );
        let catalog = result.unwrap();
        assert_eq!(catalog.completeness, AlbumCatalogCompleteness::Complete);
        assert_eq!(catalog.total, expected);
        assert_eq!(catalog.albums.len(), expected);
        assert_eq!(catalog.server_id, "fixture-machine-id");
        assert_eq!(catalog.library_id, "1");
        assert_eq!(paths.len(), page_count);
        assert!(paths[0].contains("X-Plex-Container-Start=0&X-Plex-Container-Size=200"));
        if expected == 3 {
            assert!(paths[1].contains("X-Plex-Container-Start=2&"));
        }
    }
}

#[test]
fn album_catalog_rejects_repetition_no_progress_duplicate_ids_and_inconsistent_totals() {
    for pages in [
        vec![
            album_catalog_page(0, Some(2), &["1"]),
            album_catalog_page(0, Some(2), &["1"]),
        ],
        vec![
            album_catalog_page(0, Some(2), &["1"]),
            album_catalog_page(1, Some(2), &[]),
        ],
        vec![
            album_catalog_page(0, Some(2), &["1"]),
            album_catalog_page(1, Some(2), &["1"]),
        ],
        vec![
            album_catalog_page(0, Some(2), &["1"]),
            album_catalog_page(1, Some(3), &["2"]),
        ],
        vec![album_catalog_page(0, Some(2), &["1", "1"])],
        vec![album_catalog_page(0, Some(1), &["1", "2"])],
        vec![album_catalog_page(
            0,
            Some(MAX_ALBUM_PAGES * ALBUM_PAGE_SIZE + 1),
            &["1"],
        )],
        vec![album_catalog_page(0, Some(1), &[""])],
    ] {
        let (result, _) = fixture_album_catalog(
            pages
                .into_iter()
                .map(|p| MockResponse::json("200 OK", p))
                .collect(),
            None,
        );
        let error = result.unwrap_err();
        assert_ne!(error.completeness, AlbumCatalogCompleteness::Complete);
        assert!(error.message.contains("inválida"));
    }
}

#[test]
fn album_catalog_missing_or_malformed_pagination_is_not_an_empty_success() {
    for body in [
        serde_json::json!({"MediaContainer": {"Metadata": []}}),
        serde_json::json!({"MediaContainer": {"offset": 0, "size": "0"}}),
        serde_json::json!({"MediaContainer": {"offset": 0, "size": 1, "totalSize": 1}}),
        serde_json::json!({"MediaContainer": {"offset": 0, "size": 0, "totalSize": -1}}),
        serde_json::json!({"MediaContainer": {"offset": 0, "size": 0, "Metadata": null}}),
        serde_json::json!({"unexpected": true}),
    ] {
        let (result, _) = fixture_album_catalog(vec![MockResponse::json("200 OK", body)], None);
        let error = result.unwrap_err();
        assert_eq!(error.completeness, AlbumCatalogCompleteness::Unknown);
        assert_eq!(error.received, 0);
    }
}

#[test]
fn album_catalog_http_auth_and_parse_failures_preserve_partial_classification() {
    for status in [
        "500 Internal Server Error",
        "401 Unauthorized",
        "403 Forbidden",
        "200 OK",
    ] {
        for intermediate in [false, true] {
            let mut responses = Vec::new();
            if intermediate {
                responses.push(MockResponse::json(
                    "200 OK",
                    album_catalog_page(0, Some(2), &["1"]),
                ));
            }
            responses.push(MockResponse {
                status_line: status,
                content_type: "application/json",
                body: "not json".to_string(),
                delay: Duration::ZERO,
            });
            let (result, _) = fixture_album_catalog(responses, None);
            let error = result.unwrap_err();
            assert_eq!(
                error.completeness,
                if intermediate {
                    AlbumCatalogCompleteness::Partial
                } else {
                    AlbumCatalogCompleteness::Unknown
                }
            );
            assert_eq!(error.received, usize::from(intermediate));
            if status != "200 OK" {
                assert!(error.message.contains(status.split(' ').next().unwrap()));
            }
            assert!(!error.message.contains("TEST_SECRET"));
        }
    }
}

#[test]
fn album_catalog_discards_a_final_page_after_connection_context_changes() {
    let (result, paths) = fixture_album_catalog(
        vec![
            MockResponse::json("200 OK", album_catalog_page(0, Some(2), &["1"])),
            MockResponse::json("200 OK", album_catalog_page(1, Some(2), &["2"])),
        ],
        Some(2),
    );
    let error = result.unwrap_err();
    assert_eq!(error.completeness, AlbumCatalogCompleteness::Partial);
    assert_eq!(error.received, 1);
    assert!(error.message.contains("configuração Plex mudou"));
    assert_eq!(paths.len(), 2);
}

#[test]
fn album_catalog_without_a_terminal_page_stops_at_the_request_budget() {
    let responses = (0..MAX_ALBUM_PAGES)
        .map(|offset| {
            MockResponse::json(
                "200 OK",
                album_catalog_page(offset, None, &[&(offset + 1).to_string()]),
            )
        })
        .collect();
    let (result, paths) = fixture_album_catalog(responses, None);
    let error = result.unwrap_err();
    assert_eq!(paths.len(), MAX_ALBUM_PAGES);
    assert_eq!(error.received, MAX_ALBUM_PAGES);
    assert_eq!(error.completeness, AlbumCatalogCompleteness::Partial);
    assert!(error.message.contains("limite de páginas"));
    assert!(serde_json::to_value(error).unwrap().get("albums").is_none());
}

#[test]
fn windows_aggregate_catalog_budget_is_an_error_without_accepting_the_overflowing_page() {
    let mut pages = AlbumCatalogPages::default();
    pages.bytes = 32 * 1024 * 1024;
    let connection = ResolvedPlexConnection {
        server_id: "fixture-machine-id".into(),
        base_url: String::new(),
        token: String::new(),
        generation: 0,
    };
    assert!(pages
        .accept(&album_catalog_page(0, Some(1), &["1"]), &connection)
        .is_err());
    assert!(pages.albums.is_empty());
}
#[test]
fn original_genre_merge_and_safe_source_library_projection_are_reused() {
    let connection = ResolvedPlexConnection {
        server_id: "fixture-machine-id".into(),
        base_url: String::new(),
        token: "PRIVATE_TOKEN".into(),
        generation: 0,
    };
    let value = serde_json::json!({"Genre":[{"tag":"Jazz"},{"tag":"jazz"},{"tag":"Pop / Rock"},{"tag":"PRIVATE_TOKEN"},{"tag":"http://PRIVATE.invalid/"}],"librarySectionID":7});
    assert_eq!(
        plex_album_genres(&value, &connection),
        ["Jazz", "Pop / Rock"]
    );
    assert_eq!(source_info::source_library(&value).as_deref(), Some("7"));
}

#[test]
fn windows_excessive_genre_inventory_is_rejected_without_partial_success() {
    let mut page = album_catalog_page(0, Some(1), &["1"]);
    page["MediaContainer"]["Metadata"][0]["Genre"] =
        serde_json::json!(vec![serde_json::json!({"tag":"Fixture"}); 129]);
    let (result, _) = fixture_album_catalog(vec![MockResponse::json("200 OK", page)], None);
    let error = result.unwrap_err();
    assert_eq!(error.received, 0);
    assert_eq!(error.completeness, AlbumCatalogCompleteness::Unknown);
}
