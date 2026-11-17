import { invoke } from "@tauri-apps/api/core";

export type FolderSelection = { token: string; displayPath: string };

export type Source = {
  id: number;
  displayName: string;
  generation: number;
  trackCount: number;
};

export type RemoveSourceResult = { sourceId: number; displayName: string; removedTrackCount: number; queueStopped: boolean };
export type LegacySourceMigrationStatus = { eligible: boolean; pendingSources: number };
export type LegacySourceImportResult = { importedSources: number; alreadyPresentSources: number; failedSourceNames: string[] };

export type IndexMetrics = {
  sourcesExamined: number;
  entriesVisited: number;
  filesExamined: number;
  filesNew: number;
  filesModified: number;
  filesUnchanged: number;
  filesRemoved: number;
  recordsWritten: number;
  filesOpenedForExtraction: number;
  filesProcessedByParser: number;
  successfulExtractions: number;
  filesWithoutTags: number;
  unsupportedFormats: number;
  extractionFailures: number;
};

export type Album = {
  sourceId: number;
  identity: string;
  title: string;
  albumArtists: string[];
  trackCount: number;
  staleTrackCount: number;
  dateValues: string[];
  ambiguous: boolean;
  directoryDisplay: string;
};

export type Artist = {
  name: string;
  appearsAsTrackArtist: boolean;
  appearsAsAlbumArtist: boolean;
  trackCount: number;
  albumCount: number;
  staleTrackCount: number;
};

export type Track = {
  relativePath: string;
  metadataStale: boolean;
  title: string | null;
  trackArtists: string[];
  albumArtists: string[];
  album: string | null;
  genre: string | null;
  dateText: string | null;
  trackNumber: number | null;
  discNumber: number | null;
  durationMs: number | null;
  sampleRate: number | null;
  channels: number | null;
  fileFormat: string | null;
  metadataStatus: string | null;
};

export type LibraryView = {
  sourceId: number;
  generation: number;
  albums: Album[];
  artists: Artist[];
};

export type IndexResult = {
  source: Source;
  metrics: IndexMetrics;
  view: LibraryView;
};

export type SqliteIndexProgress = {
  operationId: string;
  operation: "add" | "verify" | "reindex" | "remove" | "legacyImport";
  sourceName: string;
  phase: "scanning" | "metadata" | "grouping" | "persisting" | "completed" | "failed";
  entriesVisited: number;
  filesExamined: number;
  audioFilesFound: number;
  metadataProcessed: number;
  metadataTotal: number;
  identifiedAlbums: number;
  identifiedArtists: number;
  countsProvisional: boolean;
  resultSummary?: "verifyChanged" | "verifyUnchanged" | "sourceRemoved" | "sourceRemovedQueueStopped";
  removedTrackCount?: number;
  errorMessage?: string;
};

export const sqliteLibrary = {
  chooseFolder: () => invoke<FolderSelection | null>("sqlite_library_choose_folder"),
  cancelFolderSelection: (selectedFolderToken: string) =>
    invoke<void>("sqlite_library_cancel_folder_selection", { selectedFolderToken }),
  sources: () => invoke<Source[]>("sqlite_library_sources"),
  legacySourceMigrationStatus: () => invoke<LegacySourceMigrationStatus>("sqlite_library_legacy_source_migration_status"),
  importLegacySources: (operationId: string) =>
    invoke<LegacySourceImportResult>("sqlite_library_import_legacy_sources", { operationId }),
  addAndIndex: (selectedFolderToken: string, operationId: string) =>
    invoke<IndexResult>("sqlite_library_add_and_index", { selectedFolderToken, operationId }),
  verify: (sourceId: number, operationId: string) =>
    invoke<IndexResult>("sqlite_library_verify_source", { sourceId, operationId }),
  reindex: (sourceId: number, operationId: string) =>
    invoke<IndexResult>("sqlite_library_reindex_source", { sourceId, operationId }),
  remove: (sourceId: number) =>
    invoke<RemoveSourceResult>("sqlite_library_remove_source", { sourceId }),
  openView: (sourceId: number) =>
    invoke<LibraryView>("sqlite_library_open_view", { sourceId }),
  albums: (sourceId: number) =>
    invoke<Album[]>("sqlite_library_albums", { sourceId }),
  artists: (sourceId: number) =>
    invoke<Artist[]>("sqlite_library_artists", { sourceId }),
  albumsForArtist: (sourceId: number, artistName: string) =>
    invoke<Album[]>("sqlite_library_albums_for_artist", { sourceId, artistName }),
  tracksForAlbum: (sourceId: number, albumIdentity: string) =>
    invoke<Track[]>("sqlite_library_tracks_for_album", { sourceId, albumIdentity }),
  albumCover: (virtualAlbumPath: string) => {
    const prefix = "sqlite-album://";
    if (!virtualAlbumPath.startsWith(prefix)) return Promise.resolve(null);
    const albumId = virtualAlbumPath.slice(prefix.length).replace(/\/disc\/\d+$/, "");
    if (!/^(?:sqlite|sqlite-cue-album):\d+:[A-Za-z0-9_-]+$/.test(albumId)) return Promise.resolve(null);
    return invoke<string | null>("sqlite_library_album_cover", { albumId });
  },
  albumForTrack: (trackLocator: string) => {
    if ((!trackLocator.startsWith("sqlite-track://") && !trackLocator.startsWith("sqlite-cue://")) || trackLocator.length > 16_384) return Promise.resolve(null);
    return invoke<string | null>("sqlite_library_album_for_track", { trackLocator });
  },
};
