// Scoped, small mirror-only fixtures. No source traversal, profile, MPD or environment override.
use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scene { root: PathBuf, library: PathBuf }
impl Scene {
    fn new() -> Self {
        let root=std::env::temp_dir().join(format!("sonante-startup-proof-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let library=root.join("library");
        fs::create_dir(&library).unwrap();
        fs::create_dir(library.join("Music")).unwrap();
        fs::create_dir(library.join("Music/Album")).unwrap();
        // Deliberately absent physical origin: this object proves virtual topology only.
        let origin=root.join("absent-origin");
        symlink(origin.join("Album/track.flac"),library.join("Music/Album/track.flac")).unwrap();
        let manifest=MirrorManifest {schema_version:2,generation_id:"test-startup-proof".into(),registry_id:Some("a".repeat(32)),sources:BTreeMap::from([("Music".into(),origin)])};
        fs::write(library.join(MANIFEST_NAME),serde_json::to_vec(&manifest).unwrap()).unwrap();
        fs::set_permissions(library.join(MANIFEST_NAME),fs::Permissions::from_mode(0o600)).unwrap();
        Self {root,library}
    }
    fn mutate(&self, kind:&str) {
        let track=self.library.join("Music/Album/track.flac");
        match kind {
            "link"=>{fs::remove_file(&track).unwrap();symlink(self.root.join("outside.flac"),track).unwrap();},
            "addition"=>{symlink(self.root.join("absent-origin/Album/new.flac"),self.library.join("Music/Album/new.flac")).unwrap();},
            "removal"=>{fs::remove_file(track).unwrap();},
            "directory"=>{fs::rename(self.library.join("Music/Album"),self.root.join("old-album")).unwrap();fs::create_dir(self.library.join("Music/Album")).unwrap();},
            "marker"=>{let p=self.library.join(MANIFEST_NAME);let bytes=fs::read(&p).unwrap();fs::remove_file(&p).unwrap();fs::write(&p,bytes).unwrap();},
            _=>panic!("unknown mutation"),
        }
    }
}
impl Drop for Scene {fn drop(&mut self){fs::remove_dir_all(&self.root).unwrap();}}
#[test]
fn cached_snapshot_matches_strict_topology_without_reading_absent_physical_origin() {
    let scene=Scene::new();
    let strict=ReadonlyMirrorSnapshot::open(&scene.library).unwrap();
    let cached=ReadonlyMirrorSnapshot::open_cached(&scene.library).unwrap();
    assert_eq!(strict.topology,cached.topology);
    assert_eq!(strict.topology_hash().unwrap(),cached.topology_hash().unwrap());
    assert!(!scene.root.join("absent-origin").exists());
    strict.revalidate().unwrap();cached.revalidate().unwrap();
}
#[test]
fn retained_cached_snapshot_refuses_every_original_generation_mutation() {
    for kind in ["link","addition","removal","directory","marker"] {
        let scene=Scene::new();let snapshot=ReadonlyMirrorSnapshot::open_cached(&scene.library).unwrap();
        scene.mutate(kind);assert!(snapshot.revalidate().is_err(),"mutation: {kind}");
        assert!(snapshot.revalidate().is_err(),"dirty proof must stay invalid: {kind}");
    }
}
#[test]
fn lost_watch_never_reuses_a_cached_proof() {
    let scene=Scene::new();let snapshot=ReadonlyMirrorSnapshot::open_cached(&scene.library).unwrap();
    if snapshot.has_change_watch() {snapshot.remove_change_watch();assert!(snapshot.revalidate().is_err());assert!(snapshot.revalidate().is_err());}
    else {assert!(snapshot.revalidate_full().is_ok());scene.mutate("link");assert!(snapshot.revalidate().is_err());}
}

#[test]
fn startup_snapshot_matches_legacy_strict_and_cached_snapshots() {
    let scene=Scene::new();
    let strict=ReadonlyMirrorSnapshot::open(&scene.library).unwrap();
    let legacy=ReadonlyMirrorSnapshot::open_cached(&scene.library).unwrap();
    let startup=ReadonlyMirrorSnapshot::open_for_startup(&scene.library).unwrap();
    assert_eq!(strict.topology,startup.topology);assert_eq!(legacy.topology,startup.topology);
    assert_eq!(strict.topology_hash().unwrap(),startup.topology_hash().unwrap());
    assert_eq!(legacy.has_change_watch(),startup.has_change_watch());
    startup.revalidate().unwrap();assert!(!scene.root.join("absent-origin").exists());
}
#[test]
fn startup_refuses_mutation_before_or_during_watch_registration_even_without_notifications() {
    for phase in ["before","during","unsupported"] {
        for kind in ["link","addition","removal","directory","marker"] {
            let scene=Scene::new();let captured=ReadonlyMirrorSnapshot::capture(&scene.library).unwrap();
            let result=ReadonlyMirrorSnapshot::finish_startup(captured,|snapshot| {
                if phase=="before" {scene.mutate(kind);}
                let registered=if phase=="unsupported" {Err(())} else {MirrorChanges::open(snapshot)};
                if phase!="before" {scene.mutate(kind);}
                registered
            });
            assert!(result.is_err(),"phase {phase}, mutation {kind}");
        }
    }
}
#[test]
fn startup_without_watch_keeps_complete_revalidation() {
    let scene=Scene::new();let captured=ReadonlyMirrorSnapshot::capture(&scene.library).unwrap();
    let snapshot=ReadonlyMirrorSnapshot::finish_startup(captured,|_|Err(())).unwrap();
    assert!(!snapshot.has_change_watch());snapshot.revalidate().unwrap();
    scene.mutate("link");assert!(snapshot.revalidate().is_err());
}
#[test]
fn startup_inventory_refuses_additions_independently_of_directory_timestamps() {
    for watched in [false,true] {
        let scene=Scene::new();
        let mut snapshot=ReadonlyMirrorSnapshot::finish_startup(
            ReadonlyMirrorSnapshot::capture(&scene.library).unwrap(),
            |value|if watched {MirrorChanges::open(value)} else {Err(())},
        ).unwrap();
        scene.mutate("addition");
        // Model a filesystem reporting the same directory stamp: make stamps
        // agree without changing the captured entry names. No clock/sleep race.
        for (relative,(stamp,target)) in &mut snapshot.topology {
            if target.is_none() {*stamp=crate::source_identity::ReadonlyStamp::of(
                &fs::symlink_metadata(scene.library.join(relative)).unwrap());}
        }
        let names=snapshot.startup_names.take();
        assert!(snapshot.revalidate_full().is_ok(),"the stamp-only check misses this addition");
        snapshot.startup_names=names;
        assert!(snapshot.revalidate_startup_names().is_err());
        assert!(snapshot.revalidate_full().is_err());
    }
}
#[test]
fn startup_proof_refuses_later_mutation_and_watch_loss_at_every_admission() {
    for kind in ["link","addition","removal","directory","marker"] {
        let scene=Scene::new();let snapshot=ReadonlyMirrorSnapshot::open_for_startup(&scene.library).unwrap();
        for _ in 0..10 {snapshot.revalidate().unwrap();}
        scene.mutate(kind);for _ in 0..10 {assert!(snapshot.revalidate().is_err());}
    }
    let scene=Scene::new();let snapshot=ReadonlyMirrorSnapshot::open_for_startup(&scene.library).unwrap();
    if snapshot.has_change_watch() {snapshot.remove_change_watch();assert!(snapshot.revalidate().is_err());}
}
