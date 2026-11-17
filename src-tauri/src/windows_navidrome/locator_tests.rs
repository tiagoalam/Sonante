use super::super::proxy::windows_contract_tests::{credentials, upstream};
use super::*;

#[tokio::test]
async fn references_keep_the_original_wire_scope_and_rebind_only_the_same_account() {
    let before = Connection::fixture(credentials(
        "http://127.0.0.1:9/prefix".into(),
        "OLD",
        "one",
    ))
    .await;
    let after = Connection::fixture(credentials(
        "http://127.0.0.1:9/prefix/".into(),
        "NEW",
        "one",
    ))
    .await;
    let other = Connection::fixture(credentials(
        "http://127.0.0.1:9/prefix/".into(),
        "OTHER",
        "two",
    ))
    .await;
    assert_eq!(before.account(), after.account());
    assert_ne!(before.account(), other.account());
    let reference = MediaLocator::Navidrome {
        connection_id: "OLD".into(),
        account_id: Some(before.account()),
        song_id: "song-a".into(),
    };
    let saved = crate::navidrome::storage_locator(&reference).unwrap();
    let bytes = serde_json::to_string(&saved).unwrap();
    for secret in ["OLD", "NEW", "127.0.0.1", "PRIVATE", "prefix"] {
        assert!(!bytes.contains(secret));
    }
    let rebound = bound(&after, &saved).unwrap();
    assert!(matches!(rebound,MediaLocator::Navidrome{connection_id,..}if connection_id=="NEW"));
    assert!(bound(&other, &saved).is_err());
    assert!(bound(
        &after,
        &MediaLocator::Navidrome {
            connection_id: "OLD".into(),
            account_id: None,
            song_id: "song-a".into()
        }
    )
    .is_err());
    for id in ["../escape", "http://private/?token=PRIVATE", ""] {
        assert!(crate::navidrome::storage_locator(&MediaLocator::Navidrome {
            connection_id: "OLD".into(),
            account_id: Some(before.account()),
            song_id: id.into()
        })
        .is_none());
    }
    after.retire();
    assert!(bound(&after, &saved).is_err());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_resolution_uses_get_song_and_a_fresh_proxy_then_fences_the_actual_effect() {
    let root = r#"{"subsonic-response":{"status":"ok","song":{"id":"song-a"}}}"#;
    let (server, upstream) = upstream(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{root}",
            root.len()
        )
        .into_bytes(),
    )
    .await;
    let c = Connection::fixture(credentials(server.clone(), "ONE", "one")).await;
    let registry = Registry::default();
    registry.replace(Some(c.clone())).unwrap();
    let saved = MediaLocator::Navidrome {
        connection_id: String::new(),
        account_id: Some(c.account()),
        song_id: "song-a".into(),
    };
    let resolved = registry.resolve_saved(&saved).await.unwrap();
    assert_eq!(resolved.uri, c.stream("song-a").unwrap());
    let request = upstream.await.unwrap();
    assert!(request.starts_with("GET /prefix/rest/getSong.view?"));
    let mut count = 0;
    registry
        .with_resolved(std::slice::from_ref(&resolved), || {
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 1);
    let replacement = Connection::fixture(credentials(server, "TWO", "one")).await;
    registry.replace(Some(replacement.clone())).unwrap();
    assert!(!c.active());
    assert!(registry
        .with_resolved(&[resolved], || {
            count += 1;
            Ok(())
        })
        .is_err());
    assert_eq!(count, 1);
    registry.replace(None).unwrap();
    assert!(!replacement.active());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_accounts_and_mismatched_server_song_ids_never_get_resolved() {
    let root = r#"{"subsonic-response":{"status":"ok","song":{"id":"different"}}}"#;
    let (server, upstream) = upstream(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{root}",
            root.len()
        )
        .into_bytes(),
    )
    .await;
    let c = Connection::fixture(credentials(server, "ONE", "one")).await;
    let registry = Registry::default();
    registry.replace(Some(c.clone())).unwrap();
    let wrong = MediaLocator::Navidrome {
        connection_id: String::new(),
        account_id: Some("0".repeat(64)),
        song_id: "song-a".into(),
    };
    assert!(matches!(registry.resolve_saved(&wrong).await,Err(e)if e==CHANGED));
    let correct = MediaLocator::Navidrome {
        connection_id: String::new(),
        account_id: Some(c.account()),
        song_id: "song-a".into(),
    };
    assert!(matches!(registry.resolve_saved(&correct).await,Err(e)if e==PROTOCOL));
    upstream.await.unwrap();
}
