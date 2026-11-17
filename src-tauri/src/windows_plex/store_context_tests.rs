use super::*;
fn account() -> Account {
    Account {
        server_url: "https://fixture.invalid/".into(),
        account_token: "PRIVATE-fixture-token".into(),
        server_id: Some("fixture-server".into()),
        server_name: Some("Fixture".into()),
    }
}
#[test]
fn source_enablement_is_never_restored_from_the_account_and_schema_stays_strict() {
    let settings = account().settings(true);
    let account = Account::from_settings(&settings).unwrap();
    let bytes = serde_json::to_vec(&account).unwrap();
    assert!(!String::from_utf8(bytes.clone())
        .unwrap()
        .contains("enabled"));
    let restored: Account = serde_json::from_slice(&bytes).unwrap();
    assert!(!restored.settings(false).enabled);
    assert_eq!(
        restored.settings(false).plex_server_id.as_deref(),
        Some("fixture-server")
    );
    for raw in [br#"{"server_url":"https://fixture.invalid/","account_token":"PRIVATE","server_id":null,"server_name":null,"enabled":true}"#.to_vec(),br#"{"server_url":"https://fixture.invalid/","account_token":"PRIVATE","account_token":"OTHER","server_id":null,"server_name":null}"#.to_vec()] {assert!(serde_json::from_slice::<Account>(&raw).is_err());}
    let mut empty = account;
    empty.account_token.clear();
    assert!(empty.validate().is_err());
}
#[test]
fn a_valid_navidrome_cipher_frame_is_not_a_valid_plex_record_and_is_preserved() {
    let mut random = [0; 12];
    getrandom::fill(&mut random).unwrap();
    let root = std::env::temp_dir().join(format!("sonante-plex-purpose-{random:x?}"));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("record.json");
    let payload = serde_json::to_vec(&account()).unwrap();
    let cipher = secret::fixture_seal(Purpose::Navidrome, &payload).unwrap();
    let bytes = serde_json::to_vec(&Record {
        version: 1,
        content: Content::DpapiUser {
            data: base64::engine::general_purpose::STANDARD.encode(cipher),
        },
    })
    .unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(read(&path, secret::fixture_open).is_err());
    assert!(write(&path, None, secret::fixture_seal, secret::fixture_open).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_dir_all(root).unwrap();
}
