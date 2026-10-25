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

    pub(crate) fn coverage(&self) -> crate::index_coverage::Inventory {
        self.prepared.coverage.clone()
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

    pub(crate) fn validate_before_commit(&self) -> Result<(), String> {
        self.validate_previous_config()?;
        self.validate_published()
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
    pub(crate) fn finish_committed(&self) -> Result<(), String> {
        self.validate_published()?;
        if AppConfig::load_from_path(&self.profile.path.join("config.json"))? != self.next {
            return Err(ERROR.into());
        }
        let database = self.profile.file_identity("mpd-v2.db")?.ok_or(ERROR)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.profile.entry("mpd-v2.db")?)
            .map_err(|_| ERROR)?;
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata().map_err(|_| ERROR)?;
        if metadata.mode() & 0o022 != 0
            || metadata.dev() != database.dev
            || metadata.ino() != database.ino
        {
            return Err(ERROR.into());
        }
        file.sync_all().map_err(|_| ERROR)?;
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
