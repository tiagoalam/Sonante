//! Bounded filesystem evidence, bound to a confirmed source map and generation.
//! Check without this evidence remains a topology check, not a content scan.
use crate::local_media::{
    self,
    mirror::{ManagedRefreshPlan, MirrorManifest},
    LocalMediaClass,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

pub(crate) const MAX_DIRTY_PATHS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Source {
    pub root: PathBuf,
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Context {
    pub sources: BTreeMap<String, Source>,
    manifest: Option<MirrorManifest>,
    generation: (u64, u64),
}

impl Context {
    pub(crate) fn capture(library: &Path, configured: &[String]) -> Result<Self, String> {
        let resolved =
            local_media::resolve_local_sources(library, configured).map_err(|e| e.to_string())?;
        let metadata =
            fs::symlink_metadata(library).map_err(|_| "LOCAL_WATCH_CONTEXT_UNAVAILABLE")?;
        let manifest = if resolved.topology() == local_media::LocalLibraryTopology::Managed {
            Some(local_media::mirror::read_manifest(library)?)
        } else {
            None
        };
        let sources = resolved
            .sources()
            .iter()
            .map(|(id, root)| {
                let meta = fs::metadata(root).map_err(|_| "LOCAL_WATCH_SOURCE_UNAVAILABLE")?;
                Ok((
                    id.clone(),
                    Source {
                        root: root.clone(),
                        device: meta.dev(),
                        inode: meta.ino(),
                    },
                ))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            sources,
            manifest,
            generation: (metadata.dev(), metadata.ino()),
        })
    }

    pub(crate) fn roots(&self) -> Vec<PathBuf> {
        self.sources
            .values()
            .map(|s| s.root.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn available(&self) -> bool {
        self.sources.values().all(|source| {
            fs::canonicalize(&source.root).is_ok_and(|p| p == source.root)
                && fs::symlink_metadata(&source.root).is_ok_and(|m| {
                    m.is_dir()
                        && !m.file_type().is_symlink()
                        && m.dev() == source.device
                        && m.ino() == source.inode
                })
        })
    }

    /// Cheap revalidation at admission; no walk/hash of media or mirror tree.
    pub(crate) fn is_current(&self, library: &Path, configured: &[String]) -> bool {
        let Ok(metadata) = fs::symlink_metadata(library) else {
            return false;
        };
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || (metadata.dev(), metadata.ino()) != self.generation
            || !self.available()
        {
            return false;
        }
        let Ok(roots) = local_media::mirror::canonical_configured_roots(
            &configured.iter().map(PathBuf::from).collect::<Vec<_>>(),
        ) else {
            return false;
        };
        if roots.into_iter().collect::<BTreeSet<_>>() != self.roots().into_iter().collect() {
            return false;
        }
        let bindings = self
            .sources
            .iter()
            .map(|(id, source)| (id.clone(), source.root.clone()))
            .collect();
        if crate::source_identity::verify_if_initialized(library, &bindings).is_err() {
            return false;
        }
        match &self.manifest {
            Some(expected) => {
                local_media::mirror::read_manifest(library).is_ok_and(|actual| actual == *expected)
            }
            None => fs::symlink_metadata(library.join(local_media::mirror::MANIFEST_NAME))
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        }
    }

    /// Pending paths can cross a confirmed generation change only for the
    /// same reserved ID and the same physical source. Removed/rebound IDs drop.
    pub(crate) fn retains(&self, previous: &Self, path: &Path) -> bool {
        let namespace = |context: &Context| {
            context
                .manifest
                .as_ref()
                .and_then(|m| m.registry_id.clone())
        };
        if namespace(previous).is_some() && namespace(previous) != namespace(self) {
            return false;
        }
        self.sources.iter().any(|(id, source)| {
            previous.sources.get(id) == Some(source) && path.starts_with(&source.root)
        })
    }

    #[cfg(test)]
    pub(crate) fn for_roots(roots: &[PathBuf]) -> Self {
        let sources = roots
            .iter()
            .enumerate()
            .map(|(i, root)| {
                let root = fs::canonicalize(root).unwrap();
                let m = fs::metadata(&root).unwrap();
                (
                    i.to_string(),
                    Source {
                        root,
                        device: m.dev(),
                        inode: m.ino(),
                    },
                )
            })
            .collect();
        Self {
            sources,
            manifest: None,
            generation: (0, 0),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Batch {
    pub context: Context,
    pub paths: BTreeSet<PathBuf>,
}

pub(crate) struct Plan {
    pub refresh: ManagedRefreshPlan,
    pub needs_mirror: bool,
    pub coverage: crate::index_coverage::Inventory,
}

impl Batch {
    /// Files already represented by the same link need only an MPD directory
    /// update. CUE/new/deleted/directory entries retain the structural builder.
    pub(crate) fn plan(&self, library: &Path) -> Result<Option<Plan>, String> {
        if self.paths.len() > MAX_DIRTY_PATHS {
            return Err("LOCAL_WATCH_OVERFLOW".into());
        }
        let sources = self.context.sources.iter().map(|(id,s)|(id.clone(),s.root.clone())).collect();
        let generation = self.context.manifest.as_ref().map(|m|m.generation_id.as_str()).unwrap_or("legacy-dirty");
        let mut coverage = crate::index_coverage::Inventory::new(generation, &sources, false)?;
        let mut directories = BTreeSet::new();
        let mut needs_mirror = false;
        let mut changed = 0;
        for path in &self.paths {
            let metadata = match fs::symlink_metadata(path) {
                Ok(m) => Some(m),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => return Err("LOCAL_WATCH_PATH_UNAVAILABLE".into()),
            };
            if metadata
                .as_ref()
                .is_some_and(|m| m.file_type().is_symlink())
            {
                continue;
            }
            let class = local_media::classify_path(path);
            let is_directory = metadata.as_ref().is_some_and(|m| m.is_dir());
            if !is_directory && class == LocalMediaClass::Ignored {
                // Deleted directories have no extension/type; they still may
                // be represented by a real directory in the mirror below.
                let published_directory = self.context.sources.iter().any(|(id, source)| {
                    path.strip_prefix(&source.root)
                        .ok()
                        .is_some_and(|relative| library.join(id).join(relative).is_dir())
                });
                if !published_directory {
                    continue;
                }
            }
            let mut bound = false;
            for (id, source) in &self.context.sources {
                let Ok(relative) = path.strip_prefix(&source.root) else {
                    continue;
                };
                if relative
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                {
                    return Err("LOCAL_WATCH_UNSAFE_PATH".into());
                }
                let uri = PathBuf::from(id).join(relative);
                let uri_string = uri.to_str().ok_or("LOCAL_WATCH_UNSAFE_PATH")?;
                if !local_media::is_safe_relative_mpd_uri(uri_string) {
                    return Err("LOCAL_WATCH_UNSAFE_PATH".into());
                }
                bound = true;
                if class == LocalMediaClass::PlayableAudio {
                    if let Some(meta) = metadata.as_ref().filter(|m| m.is_file()) { coverage.observe(id, relative, path, crate::index_coverage::Kind::Audio, meta)?; }
                }
                let published = library.join(&uri);
                let existing = class != LocalMediaClass::ControlSidecar
                    && metadata.as_ref().is_some_and(|m| m.is_file())
                    && fs::read_link(&published).is_ok_and(|p| p == *path)
                    && fs::canonicalize(path).is_ok_and(|p| p == *path);
                needs_mirror |= !existing;
                // Update the parent to cover CUE virtual tracks and a removed
                // entry as well as the currently existing physical file.
                let directory = if is_directory {
                    uri.as_path()
                } else {
                    uri.parent().unwrap_or(Path::new(id))
                };
                let directory = directory.to_str().ok_or("LOCAL_WATCH_UNSAFE_PATH")?;
                if !local_media::is_safe_relative_mpd_uri(directory) {
                    return Err("LOCAL_WATCH_UNSAFE_PATH".into());
                }
                directories.insert(directory.to_owned());
            }
            if !bound {
                return Err("LOCAL_WATCH_UNBOUND_PATH".into());
            }
            changed += 1;
        }
        if directories.is_empty() {
            return Ok(None);
        }
        Ok(Some(Plan {
            refresh: targeted(directories, changed),
            needs_mirror,
            coverage,
        }))
    }
}

fn targeted(directories: BTreeSet<String>, changed_files: usize) -> ManagedRefreshPlan {
    let directories = directories
        .iter()
        .filter(|path| {
            !Path::new(path)
                .ancestors()
                .skip(1)
                .any(|parent| parent.to_str().is_some_and(|p| directories.contains(p)))
        })
        .cloned()
        .collect();
    ManagedRefreshPlan::Targeted {
        directories,
        changed_files,
    }
}

pub(crate) fn merge(
    structure: &ManagedRefreshPlan,
    content: &ManagedRefreshPlan,
) -> ManagedRefreshPlan {
    match (structure, content) {
        (ManagedRefreshPlan::Full, _) | (_, ManagedRefreshPlan::Full) => ManagedRefreshPlan::Full,
        (ManagedRefreshPlan::MetadataOnly, other) | (other, ManagedRefreshPlan::MetadataOnly) => {
            other.clone()
        }
        (
            ManagedRefreshPlan::Targeted {
                directories: a,
                changed_files: na,
            },
            ManagedRefreshPlan::Targeted {
                directories: b,
                changed_files: nb,
            },
        ) => targeted(a.iter().chain(b).cloned().collect(), na + nb),
    }
}
