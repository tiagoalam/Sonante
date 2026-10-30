//! Initial publication has no prior MPD catalogue to restore. Never use the
//! folders-only Save's existing-catalogue assumptions, or a legacy mirror.
use crate::{
    config::AppConfig,
    local_media::mirror,
    profile_lease::Lease,
    recovery_fs::{self, Dir, Identity},
};
use std::{
    fs::{self, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
};

const LIMIT: u64 = 16 * 1024 * 1024;
const ERROR: &str = "FIRST_RUN_MANAGED_SAVE_UNCONFIRMED";
const DATABASE_ERROR: &str = "FIRST_RUN_DATABASE_UNCONFIRMED";
pub(crate) const PENDING: &str = ".sonante-initial-library.json";

/// A separate, private admission for resuming only the published first library.
/// Old stop markers alone never acquire this authority. No credentials or media
/// bytes are stored here. Preferences and catalogue completion are separate.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingRecord {
    version: u8,
    profile: Identity,
    library: Identity,
    previous_library: Option<Identity>,
    config_hash: String,
    manifest_hash: String,
    topology_hash: String,
    ledger_hash: String,
    ledger_lock_hash: String,
    intent_hash: String,
    #[serde(default)]
    policy_excluded: std::collections::BTreeMap<String, usize>,
}

pub(crate) struct PendingPreparation {
    profile: Dir,
    record: PendingRecord,
    bytes: Vec<u8>,
    id: Identity,
}

impl PendingPreparation {
    pub(crate) fn restore_policy_counts(&self, inventory: &mut crate::index_coverage::Inventory) -> Result<(), String> {
        let ids = inventory.source_ids();
        for (id, count) in &self.record.policy_excluded {
            if !ids.contains(id) { return Err(ERROR.into()); }
            inventory.exclude(id, *count);
        }
        Ok(())
    }

    fn digest(profile: &Dir, name: &str) -> Result<String, String> {
        Ok(recovery_fs::hash(
            &profile.read(name, LIMIT, &|| false)?.ok_or(ERROR)?,
        ))
    }

    /// Read-only configuration admission. It does not admit catalogue readers,
    /// playback, mutation, an old journal, or an unverified physical source.
    pub(crate) fn config(path: &std::path::Path) -> Result<Option<AppConfig>, String> {
        if !path.try_exists().map_err(|_| ERROR)? {
            return Ok(None);
        }
        let profile = Dir::open(path)?;
        let Some(bytes) = profile.read(PENDING, LIMIT, &|| false)? else {
            return Ok(None);
        };
        let record: PendingRecord =
            crate::source_identity::readonly_unique_json(&bytes).map_err(|_| ERROR)?;
        let pending = Self {
            id: profile.file_identity(PENDING)?.ok_or(ERROR)?,
            profile,
            record,
            bytes,
        };
        pending.validate_config().map(Some)
    }

    pub(crate) fn open(owner: &Lease) -> Result<Self, String> {
        owner.validate()?;
        let profile = Dir::open(&owner.root().path)?;
        let bytes = profile.read(PENDING, LIMIT, &|| false)?.ok_or(ERROR)?;
        let record = crate::source_identity::readonly_unique_json(&bytes).map_err(|_| ERROR)?;
        let pending = Self {
            id: profile.file_identity(PENDING)?.ok_or(ERROR)?,
            profile,
            record,
            bytes,
        };
        if pending.profile.id() != owner.root().id() {
            return Err(ERROR.into());
        }
        pending.validate_config()?;
        pending.snapshot()?;
        owner.validate()?;
        Ok(pending)
    }

    fn validate_config(&self) -> Result<AppConfig, String> {
        self.profile.validate()?;
        if self.record.version != 1
            || self.profile.id() != self.record.profile
            || self.profile.file_identity(PENDING)?.as_ref() != Some(&self.id)
            || self.profile.read(PENDING, LIMIT, &|| false)?.as_ref() != Some(&self.bytes)
        {
            return Err(ERROR.into());
        }
        for name in [crate::recovery_set::PENDING, crate::recovery_set::SEAL] {
            absent(&self.profile, name)?;
        }
        let config = AppConfig::load_from_path(&self.profile.path.join("config.json"))?;
        if config.first_run
            || recovery_fs::hash(&serde_json::to_vec(&config).map_err(|_| ERROR)?)
                != self.record.config_hash
            || self.profile.child("library")?.id() != self.record.library
            || Self::digest(&self.profile.child("library")?, mirror::MANIFEST_NAME)?
                != self.record.manifest_hash
            || Self::digest(&self.profile, "local-source-identities.json")?
                != self.record.ledger_hash
            || Self::digest(&self.profile, "local-source-identities.lock")?
                != self.record.ledger_lock_hash
        {
            return Err(ERROR.into());
        }
        if self.profile.exists(crate::managed_save_intent::NAME)?
            && Self::digest(&self.profile, crate::managed_save_intent::NAME)?
                != self.record.intent_hash
        {
            return Err(ERROR.into());
        }
        if self.profile.exists("library.next")? {
            let previous = self.profile.child("library.next")?;
            if self.record.previous_library.as_ref() != Some(&previous.id())
                || fs::read_dir(&previous.path)
                    .map_err(|_| ERROR)?
                    .next()
                    .is_some()
            {
                return Err(ERROR.into());
            }
        }
        self.profile.validate()?;
        Ok(config)
    }

    pub(crate) fn snapshot(&self) -> Result<mirror::ReadonlyMirrorSnapshot, String> {
        let config = self.validate_config()?;
        let snapshot = mirror::ReadonlyMirrorSnapshot::open(&self.profile.path.join("library"))
            .map_err(|_| ERROR)?;
        let configured = config
            .local_folders
            .iter()
            .map(PathBuf::from)
            .collect::<std::collections::BTreeSet<_>>();
        let bound = snapshot
            .manifest
            .sources
            .values()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if snapshot.manifest.schema_version != 2
            || snapshot.manifest.registry_id.is_none()
            || configured.len() != config.local_folders.len()
            || configured != bound
            || snapshot.topology_hash()? != self.record.topology_hash
        {
            return Err(ERROR.into());
        }
        crate::source_identity::verify_origins(
            &self.profile.path.join("library"),
            &snapshot.manifest.sources,
        )?;
        snapshot.revalidate().map_err(|_| ERROR)?;
        Ok(snapshot)
    }

    /// Idempotent across crashes during cleanup. Only an originally admitted
    /// empty predecessor can be removed; all other contents are retained.
    pub(crate) fn finish(&self, owner: &Lease) -> Result<(), String> {
        owner.validate()?;
        self.snapshot()?;
        sync_database(&self.profile)?;
        if self.profile.exists("library.next")? {
            self.validate_config()?;
            fs::remove_dir(self.profile.entry("library.next")?).map_err(|_| ERROR)?;
            self.profile.sync()?;
        }
        self.validate_config()?;
        if self.profile.exists(crate::managed_save_intent::NAME)? {
            fs::remove_file(self.profile.entry(crate::managed_save_intent::NAME)?)
                .map_err(|_| ERROR)?;
            self.profile.sync()?;
        }
        self.validate_config()?;
        owner.validate()?;
        fs::remove_file(self.profile.entry(PENDING)?).map_err(|_| ERROR)?;
        self.profile.sync()
    }
}

/// A live-only owner. Drop intentionally does not undo anything: a panic/crash
/// leaves the durable stop marker for manual review, like a managed Save.
pub(crate) struct InitialPublication<'a> {
    owner: &'a Lease,
    profile: Dir,
    before_config: Option<Vec<u8>>,
    before_config_id: Option<Identity>,
    before_library: Option<Identity>,
    next: AppConfig,
    marker: Vec<u8>,
    marker_id: Identity,
    rejected_database: String,
    prepared: mirror::MirrorBuildResult,
    publication: Option<mirror::ManagedMirrorPublication>,
}

fn absent(profile: &Dir, name: &str) -> Result<(), String> {
    if profile.exists(name)? {
        Err(ERROR.into())
    } else {
        Ok(())
    }
}

fn empty_library(profile: &Dir) -> Result<Option<Identity>, String> {
    if !profile.exists("library")? {
        return Ok(None);
    }
    let library = profile.child("library")?;
    if fs::read_dir(&library.path)
        .map_err(|_| ERROR)?
        .next()
        .is_some()
    {
        // first_run is not authority to repair or replace a retained catalogue.
        return Err("FIRST_RUN_EXISTING_LIBRARY_REQUIRES_REVIEW".into());
    }
    library.validate()?;
    Ok(Some(library.id()))
}

impl<'a> InitialPublication<'a> {
    pub(crate) fn validate_preferences_before_save(&self) -> Result<(), String> {
        self.validate_previous_config()?;
        self.validate_published()
    }

    /// Called only AFTER durable preference persistence. Until this record is
    /// durable, IPC cannot report success. A crash or an uncertain config rename
    /// before then leaves the old stop marker blocked for explicit review.
    pub(crate) fn prepare_background(&self) -> Result<(), String> {
        self.validate_published()?;
        if AppConfig::load_from_path(&self.profile.path.join("config.json"))? != self.next {
            return Err(ERROR.into());
        }
        absent(&self.profile, PENDING)?;
        let record = PendingRecord {
            version: 1,
            profile: self.profile.id(),
            library: self.profile.child("library")?.id(),
            previous_library: self.before_library.clone(),
            config_hash: recovery_fs::hash(&serde_json::to_vec(&self.next).map_err(|_| ERROR)?),
            manifest_hash: PendingPreparation::digest(
                &self.profile.child("library")?,
                mirror::MANIFEST_NAME,
            )?,
            topology_hash: mirror::ReadonlyMirrorSnapshot::open(&self.profile.path.join("library"))
                .map_err(|_| ERROR)?
                .topology_hash()?,
            ledger_hash: PendingPreparation::digest(&self.profile, "local-source-identities.json")?,
            ledger_lock_hash: PendingPreparation::digest(
                &self.profile,
                "local-source-identities.lock",
            )?,
            intent_hash: recovery_fs::hash(&self.marker),
            policy_excluded: self.prepared.coverage.policy_exclusions().clone(),
        };
        self.profile
            .write_new(PENDING, &serde_json::to_vec(&record).map_err(|_| ERROR)?)
    }
    pub(crate) fn prepare(
        owner: &'a Lease,
        previous: &AppConfig,
        next: &AppConfig,
        generation: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, String> {
        owner.validate()?;
        recovery_fs::check(cancelled)?;
        if !previous.first_run
            || next.first_run
            || generation.is_empty()
            || !generation
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err(ERROR.into());
        }
        let profile = Dir::open(&owner.root().path)?;
        if profile.id() != owner.root().id() {
            return Err(ERROR.into());
        }
        for name in [
            crate::managed_save_intent::NAME,
            crate::recovery_set::PENDING,
            crate::recovery_set::SEAL,
            "library.next",
            "mpd-v2.db",
            "mpd-v2.db.writing",
            PENDING,
        ] {
            absent(&profile, name)?;
        }
        let before_library = empty_library(&profile)?;
        let before_config_id = profile.file_identity("config.json")?;
        let before_config = profile.read("config.json", LIMIT, cancelled)?;
        if AppConfig::load_from_path(&profile.path.join("config.json"))? != *previous
            || profile.file_identity("config.json")? != before_config_id
        {
            return Err(ERROR.into());
        }
        let rejected_database = format!(".first-run-rejected-{generation}-mpd-v2.db");
        absent(&profile, &rejected_database)?;
        absent(&profile, &format!("{rejected_database}.writing"))?;
        // Reuse the existing stop-marker NAME, so all existing startup/read/Save
        // guards fail closed. This distinct initial format has no automatic
        // recovery interpretation; it contains hashes, never Plex credentials.
        let marker = serde_json::to_vec(&serde_json::json!({
            "initial_managed_save": 1, "profile": profile.id(),
            "previous_config": before_config.as_ref().map(|b| recovery_fs::hash(b)),
            "previous_library": before_library,
            "next_config": recovery_fs::hash(&serde_json::to_vec(next).map_err(|_| ERROR)?),
            "generation": generation, "rejected_database": rejected_database,
        }))
        .map_err(|_| ERROR)?;
        profile.write_new(crate::managed_save_intent::NAME, &marker)?;
        let marker_id = profile
            .file_identity(crate::managed_save_intent::NAME)?
            .ok_or(ERROR)?;
        owner.validate()?;
        recovery_fs::check(cancelled)?;
        let live = profile.path.join("library");
        let roots = next
            .local_folders
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let preparation = mirror::prepare_managed_generation(
            &live,
            &profile.path.join("library.next"),
            &roots,
            generation,
        )?;
        let mirror::ManagedGenerationPreparation::Prepared(prepared) = preparation else {
            return Err(ERROR.into());
        };
        if prepared.manifest.schema_version != 2 || prepared.manifest.registry_id.is_none() {
            return Err(ERROR.into());
        }
        Ok(Self {
            owner,
            profile,
            before_config,
            before_config_id,
            before_library,
            next: next.clone(),
            marker,
            marker_id,
            rejected_database,
            prepared,
            publication: None,
        })
    }

    fn validate(&self) -> Result<(), String> {
        self.owner.validate()?;
        self.profile.validate()?;
        if self.profile.id() != self.owner.root().id()
            || self
                .profile
                .file_identity(crate::managed_save_intent::NAME)?
                .as_ref()
                != Some(&self.marker_id)
            || self
                .profile
                .read(crate::managed_save_intent::NAME, LIMIT, &|| false)?
                .as_deref()
                != Some(&self.marker)
        {
            return Err(ERROR.into());
        }
        Ok(())
    }

    fn validate_previous_config(&self) -> Result<(), String> {
        self.validate()?;
        if self.profile.file_identity("config.json")? != self.before_config_id
            || self.profile.read("config.json", LIMIT, &|| false)? != self.before_config
        {
            return Err(ERROR.into());
        }
        Ok(())
    }

    pub(crate) fn publish(&mut self, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
        self.validate_previous_config()?;
        if self.publication.is_some() || empty_library(&self.profile)? != self.before_library {
            return Err(ERROR.into());
        }
        absent(&self.profile, "mpd-v2.db")?;
        recovery_fs::check(cancelled)?;
        crate::managed_save_intent::sync_mirror(&self.prepared.staging_path, 0)?;
        self.publication = Some(mirror::publish_prepared_managed_generation_guarded(
            &self.profile.path.join("library"),
            &self.prepared,
            || {
                self.validate_previous_config()?;
                recovery_fs::check(cancelled)
            },
        )?);
        // Make publication durable BEFORE Supervisor can start/index MPD.
        crate::managed_save_intent::sync_mirror(&self.profile.path.join("library"), 0)?;
        self.profile.sync()?;
        self.validate_published()
    }

    pub(crate) fn validate_published(&self) -> Result<(), String> {
        self.validate()?;
        let publication = self.publication.as_ref().ok_or(ERROR)?;
        let live = self.profile.child("library")?;
        let (dev, ino) = publication.preview_generation_identity();
        if live.id() != (Identity { dev, ino }) {
            return Err(ERROR.into());
        }
        let snapshot = mirror::ReadonlyMirrorSnapshot::open(&self.profile.path.join("library"))
            .map_err(|_| ERROR)?;
        if snapshot.manifest != self.prepared.manifest {
            return Err(ERROR.into());
        }
        crate::source_identity::verify_origins(
            &self.profile.path.join("library"),
            &snapshot.manifest.sources,
        )?;
        snapshot.revalidate().map_err(|_| ERROR.to_string())
    }

    #[cfg(test)]
    pub(crate) fn validate_before_commit(&self) -> Result<(), String> {
        self.validate_previous_config()?;
        self.validate_published()?;
        // MPD can finish an update even when saving its database failed. Do not
        // commit first_run=false on the strength of the in-memory catalogue.
        self.sync_database().map(|_| ())
    }

    #[cfg(test)]
    fn sync_database(&self) -> Result<Identity, String> {
        sync_database(&self.profile)
    }

    /// Stop must be confirmed before any rename/cleanup. No old DB was admitted:
    /// any new DB is retained under an exclusive name, never deleted.
    pub(crate) fn rollback(&self, stop: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        stop()?;
        self.validate_previous_config()?;
        if let Some(publication) = &self.publication {
            self.validate_published()?;
            publication.rollback()?;
            self.profile.sync()?;
            publication.cleanup_rejected_after_rollback()?;
        } else {
            mirror::cleanup_prepared_managed_generation(&self.prepared)?;
        }
        if empty_library(&self.profile)? != self.before_library {
            return Err(ERROR.into());
        }
        for (name, retained) in [
            ("mpd-v2.db", self.rejected_database.clone()),
            (
                "mpd-v2.db.writing",
                format!("{}.writing", self.rejected_database),
            ),
        ] {
            if let Some(id) = self.profile.file_identity(name)? {
                // Retain only a private, exclusive regular file, not a symlink.
                let file = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(self.profile.entry(name)?)
                    .map_err(|_| ERROR)?;
                use std::os::unix::fs::MetadataExt;
                let metadata = file.metadata().map_err(|_| ERROR)?;
                if metadata.mode() & 0o022 != 0
                    || metadata.dev() != id.dev
                    || metadata.ino() != id.ino
                {
                    return Err(ERROR.into());
                }
                file.sync_all().map_err(|_| ERROR)?;
                self.profile.rename(name, &self.profile, &retained, true)?;
                if self.profile.file_identity(&retained)?.as_ref() != Some(&id) {
                    return Err(ERROR.into());
                }
            }
        }
        self.validate_previous_config()?;
        absent(&self.profile, "library.next")?;
        absent(&self.profile, "mpd-v2.db")?;
        absent(&self.profile, "mpd-v2.db.writing")?;
        self.remove_marker()
    }

    /// Config persistence is already confirmed. Failure here must preserve the
    /// published generation and stop startup, never roll back a committed config.
    #[cfg(test)]
    pub(crate) fn finish_committed(&self) -> Result<(), String> {
        self.validate_published()?;
        if AppConfig::load_from_path(&self.profile.path.join("config.json"))? != self.next {
            return Err(ERROR.into());
        }
        let database = self.sync_database()?;
        self.publication
            .as_ref()
            .ok_or(ERROR)?
            .cleanup_previous_durable()?;
        absent(&self.profile, "library.next")?;
        self.validate_published()?;
        if self.profile.file_identity("mpd-v2.db")?.as_ref() != Some(&database) {
            return Err(ERROR.into());
        }
        self.remove_marker()
    }

    fn remove_marker(&self) -> Result<(), String> {
        self.validate()?;
        self.profile.sync()?;
        fs::remove_file(self.profile.entry(crate::managed_save_intent::NAME)?)
            .map_err(|_| ERROR)?;
        // Caller tracks this error explicitly; visible absence is not durability.
        self.profile.sync()
    }
}

#[cfg(test)]
#[path = "first_run_managed_tests.rs"]
mod tests;

fn sync_database(profile: &Dir) -> Result<Identity, String> {
    let database = profile.file_identity("mpd-v2.db")?.ok_or(DATABASE_ERROR)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(profile.entry("mpd-v2.db")?)
        .map_err(|_| DATABASE_ERROR)?;
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().map_err(|_| DATABASE_ERROR)?;
    if metadata.mode() & 0o022 != 0
        || metadata.dev() != database.dev
        || metadata.ino() != database.ino
    {
        return Err(DATABASE_ERROR.into());
    }
    file.sync_all().map_err(|_| DATABASE_ERROR)?;
    profile.sync()?;
    if profile.file_identity("mpd-v2.db")?.as_ref() != Some(&database) {
        return Err(DATABASE_ERROR.into());
    }
    Ok(database)
}
