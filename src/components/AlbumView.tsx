import { usePlaybackAttempt } from "../utils/usePlaybackAttempt";
import { PlaybackRequestNotice } from "./PlaybackRequestNotice";
import { logError } from "../utils/frontendDiagnostics";
import React, { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  Play,
  Clock,
  Disc3,
  User,
  Sparkles,
  Heart,
  ListPlus,
  Info,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { PlexAlbum, PlexTrack, SelectedArtist } from "../types/plex";
import { FavoriteAlbum } from "../types/favorite";
import { plexService } from "../services/plex";
import { audioService } from "../services/audio";
import { PlexImage } from "./PlexImage";
import { favoritesService } from "../services/favorites";
import { PlexFileInfoModal } from "./PlexFileInfoModal";
import { PlaylistPickerModal } from "./PlaylistPickerModal";
import type { NewPlaylistItem } from "../types/playlist";

interface Props {
  album: PlexAlbum;
  active?: boolean;
  onBack: () => void;
  onSelectArtist?: (artist: SelectedArtist) => void;
  onToggleFavorite?: () => void;
  isPlaybackAvailable: boolean;
}

export const AlbumView: React.FC<Props> = ({
  album,
  active = true,
  onBack,
  onSelectArtist,
  onToggleFavorite,
  isPlaybackAvailable,
}) => {
  const { t } = useTranslation();
  const [tracks, setTracks] = useState<PlexTrack[]>([]);
  const [loading, setLoading] = useState(true);
  const [isFavorite, setIsFavorite] = useState(false);
  const [playlistItems, setPlaylistItems] = useState<NewPlaylistItem[] | null>(null);
  const infoAlbumKey = JSON.stringify([album.server_id ?? null, album.library_id ?? null, album.rating_key]);
  const [fileInfoTrack, setFileInfoTrack] = useState<{ albumKey: string; track: PlexTrack } | null>(null);

  const viewport = useRef<HTMLDivElement>(null), scroll = useRef(0), artistLink = useRef<HTMLButtonElement>(null);
  const returning = useRef(false), returnFocus = useRef<HTMLElement | null>(null);
  useLayoutEffect(() => {
    if (!active) { setFileInfoTrack(null); returning.current = true; return; }
    if (viewport.current) {
      viewport.current.scrollTop = scroll.current;
      if (returning.current) (returnFocus.current ?? artistLink.current)?.focus({ preventScroll: true });
      else if (onSelectArtist) viewport.current.querySelector<HTMLElement>("h1")?.focus({ preventScroll: true });
      returning.current = false;
    }
  }, [active]);

  const albumKey = String(album.rating_key || "");

  useEffect(() => {
    let current = true;
    setFileInfoTrack(null);
    setLoading(true);
    plexService
      .getAlbumTracks(albumKey)
      .then(value => { if (current) setTracks(value); })
      .catch(error => { if (current) logError("plex.album-tracks", error); })
      .finally(() => { if (current) setLoading(false); });

    favoritesService
      .getFavorites()
      .then((favs) => {
        if (current) setIsFavorite(favs.some((f) => f.source === "plex" && String(f.id) === albumKey));
      })
      .catch(error => { if (current) logError("favorites.load", error); });
    return () => { current = false; };
  }, [albumKey, album.server_id, album.library_id]);

  const handleToggleFav = async () => {
    if (!albumKey) return;
    const favItem: FavoriteAlbum = {
      id: albumKey,
      source: "plex",
      title: album.title,
      artist: album.artist,
      year: album.year != null ? String(album.year) : undefined,
      thumb: null,
      plex_image: album.thumb || null,
      path_or_key: albumKey,
      exists: true,
    };

    try {
      const added = await favoritesService.toggleFavorite(favItem);
      setIsFavorite(added);
      if (onToggleFavorite) {
        onToggleFavorite();
      }
    } catch (err) {
      logError("plex.album-favorite", err);
    }
  };

  const request = usePlaybackAttempt(infoAlbumKey + String(active), "plex.album-play");
  const handlePlayTracks = (startIndex: number = 0) => {
    if (!active || !isPlaybackAvailable) return;
    const metaTracks = tracks.map((t) => ({
      title: t.title,
      artist: album.artist,
      album: album.title,
      plex_image: t.thumb || album.thumb || null,
      media_locator: t.media_locator,
      duration: t.duration_ms ? t.duration_ms / 1000 : (t.duration ? (t.duration > 1000 ? t.duration / 1000 : t.duration) : undefined),
    }));

    if (metaTracks.length > 0) {
      void request.run(intent => audioService.playTracks(metaTracks, startIndex, intent));
    }
  };

  const toPlaylistItem = (track: PlexTrack): NewPlaylistItem => ({
    media_locator: track.media_locator,
    metadata: {
      title: track.title,
      artist: album.artist,
      album: track.album_title || album.title,
      duration: track.duration_ms
        ? track.duration_ms / 1000
        : track.duration && track.duration > 1000
          ? track.duration / 1000
          : track.duration,
    },
  });

  const formatDuration = (ms: number) => {
    const totalSeconds = Math.floor(ms / 1000);
    const minutes = Math.floor(totalSeconds / 60);
    const seconds = totalSeconds % 60;
    return `${minutes}:${seconds.toString().padStart(2, "0")}`;
  };

  return (
    <div className="flex-1 flex flex-col h-full bg-[var(--theme-surface-base,#121212)] overflow-hidden select-none">
      <PlaybackRequestNotice failure={request.failure} />
      <div className="p-8 pb-4 border-b border-[color:var(--theme-border-soft,#222222)] flex items-center">
        <button
          onClick={onBack}
          className="flex items-center space-x-2 text-xs font-semibold text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer"
        >
          <ArrowLeft size={16} />
          <span>{t("common.backToLibrary")}</span>
        </button>
      </div>

      <div ref={viewport} onFocusCapture={event => { if (active) returnFocus.current = event.target as HTMLElement; }} onScroll={event => { if (active) scroll.current = event.currentTarget.scrollTop; }} className="flex-1 overflow-y-auto p-8 space-y-8">
        <div className="flex items-end space-x-6">
          <div className="w-56 h-56 rounded-xl bg-[var(--theme-surface-raised,#202020)] border border-[color:var(--theme-border-soft,#2B2B2B)] overflow-hidden shrink-0 shadow-2xl flex items-center justify-center">
            {album.thumb ? (
              <PlexImage
                image={album.thumb}
                alt={album.title}
                className="w-full h-full object-cover"
              />
            ) : (
              <Disc3 size={64} className="text-[color:var(--theme-fg-faint,#444444)]" />
            )}
          </div>

          <div className="space-y-3">
            <span className="text-xs font-bold text-[color:var(--theme-fg-accent,#E5A00D)] uppercase tracking-wider flex items-center space-x-1">
              <Sparkles size={13} />
              <span>{t("albumView.plexAlbum")}</span>
            </span>

            <h1 tabIndex={-1} className="text-3xl font-black text-[color:var(--theme-fg-primary,#ffffff)]">{album.title}</h1>

            <div className="flex items-center space-x-2 text-sm text-[color:var(--theme-fg-secondary,#CCCCCC)]">
              {album.artist_rating_key && onSelectArtist ? (
                <button type="button" ref={artistLink}
                  onClick={() =>
                    onSelectArtist({
                      rating_key: album.artist_rating_key!,
                      name: album.artist,
                      server_id: album.server_id, library_id: album.library_id,
                    })
                  }
                  className="rounded font-medium focus-visible:outline focus-visible:outline-[color:var(--theme-border-accent,#E5A00D)] hover:text-[color:var(--theme-fg-accent,#E5A00D)] transition-colors cursor-pointer flex items-center space-x-1.5"
                >
                  <User size={15} />
                  <span>{album.artist}</span>
                </button>
              ) : (
                <span className="font-medium">{album.artist}</span>
              )}

              {album.year && (
                <>
                  <span className="text-[color:var(--theme-fg-muted,#666666)]">•</span>
                  <span className="text-[color:var(--theme-fg-muted,#888888)]">{album.year}</span>
                </>
              )}

              <span className="text-[color:var(--theme-fg-muted,#666666)]">•</span>
              <span className="text-[color:var(--theme-fg-muted,#888888)]">{t("common.trackCount", { count: tracks.length })}</span>
            </div>

            <div className="pt-2 flex items-center space-x-3">
              <button
                onClick={() => handlePlayTracks(0)}
                disabled={!isPlaybackAvailable || loading || tracks.length === 0}
                className="flex items-center space-x-2 px-6 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-lg transition-transform active:scale-95 cursor-pointer disabled:opacity-50"
              >
                <Play size={16} fill="black" />
                <span>{t("common.playAlbum")}</span>
              </button>

              <button
                type="button"
                onClick={() => setPlaylistItems(tracks.map(toPlaylistItem))}
                disabled={loading || tracks.length === 0}
                className="flex items-center space-x-2 rounded-xl border border-[color:var(--theme-border-soft,#2B2B2B)] bg-[var(--theme-surface-raised,#1E1E1E)] px-4 py-2.5 text-xs font-bold text-[color:var(--theme-fg-primary,#ffffff)] hover:bg-[var(--theme-surface-hover,#282828)] disabled:opacity-50"
                title={t("playlists.addAlbum")}
              >
                <ListPlus size={16} />
                <span>{t("playlists.addAlbum")}</span>
              </button>

              <button
                onClick={handleToggleFav}
                className="p-2.5 rounded-xl bg-[var(--theme-surface-raised,#1E1E1E)] border border-[color:var(--theme-border-soft,#2B2B2B)] hover:bg-[var(--theme-surface-hover,#282828)] text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer"
                title={isFavorite ? t("favorites.removeFavorite") : t("common.addFavorite")}
              >
                <Heart
                  size={16}
                  className={isFavorite ? "text-[color:var(--theme-fg-accent,#E5A00D)]" : "text-[color:var(--theme-fg-muted,#888888)]"}
                  fill={isFavorite ? "#E5A00D" : "none"}
                />
              </button>
            </div>
          </div>
        </div>

        <div className="bg-[var(--theme-surface-base,#141414)] border border-[color:var(--theme-border-soft,#222222)] rounded-xl overflow-hidden divide-y divide-[color:var(--theme-border-soft,#1D1D1D)]">
          <div className="grid grid-cols-12 px-4 py-2.5 text-[11px] font-bold text-[color:var(--theme-fg-muted,#666666)] uppercase tracking-wider bg-[var(--theme-surface-panel,#181818)]">
            <span className="col-span-1 text-center">#</span>
            <span className="col-span-7">{t("common.trackTitle")}</span>
            <span className="col-span-4 text-right flex items-center justify-end space-x-1">
              <Clock size={12} />
              <span>{t("common.duration")}</span>
            </span>
          </div>

          {loading ? (
            <div className="p-8 text-center text-xs text-[color:var(--theme-fg-muted,#666666)]">
              {t("albumView.loadingTracks")}
            </div>
          ) : tracks.length === 0 ? (
            <div className="p-8 text-center text-xs text-[color:var(--theme-fg-muted,#666666)]">
              {t("common.noTracks")}
            </div>
          ) : (
            tracks.map((track, idx) => (
              <div
                key={track.rating_key}
                onClick={() => handlePlayTracks(idx)}
                aria-disabled={!isPlaybackAvailable}
                className={`grid grid-cols-12 px-4 py-3 text-xs items-center transition-colors group ${
                  isPlaybackAvailable
                    ? "hover:bg-[var(--theme-surface-raised,#1E1E1E)] cursor-pointer"
                    : "opacity-60 cursor-not-allowed"
                }`}
              >
                <span className="col-span-1 text-center font-mono text-[color:var(--theme-fg-muted,#666666)] group-hover:text-[color:var(--theme-fg-accent,#E5A00D)]">
                  {track.index || idx + 1}
                </span>

                <div className="col-span-7 flex flex-col pr-2">
                  <span className="font-semibold text-[color:var(--theme-fg-primary,#ffffff)] group-hover:text-[color:var(--theme-fg-accent,#E5A00D)] transition-colors truncate">
                    {track.title}
                  </span>
                  <span className="text-[11px] text-[color:var(--theme-fg-muted,#777777)] truncate">
                    {album.artist}
                  </span>
                </div>
                <div className="col-span-4 flex items-center justify-end gap-3">
                  <span className="font-mono text-[color:var(--theme-fg-muted,#888888)]">
                    {formatDuration(track.duration_ms ?? track.duration ?? 0)}
                  </span>
                  <button
                    type="button"
                    onClick={(event) => {
                      event.stopPropagation();
                      setPlaylistItems([toPlaylistItem(track)]);
                    }}
                    className="rounded-md p-1.5 text-[color:var(--theme-fg-muted,#777777)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] hover:text-[color:var(--theme-fg-accent,#E5A00D)]"
                    title={t("playlists.addTrack")}
                  >
                    <ListPlus size={15} />
                  </button>
                  <button type="button" data-plex-file-info onClick={event => { event.stopPropagation(); setFileInfoTrack({ albumKey: infoAlbumKey, track }); }}
                    onKeyDown={event => event.stopPropagation()} title={t("fileInfo.title")} aria-label={t("fileInfo.forTrack", { title: track.title })}
                    className="rounded-md p-1.5 text-[color:var(--theme-fg-muted,#777777)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] hover:text-[color:var(--theme-fg-accent,#E5A00D)]">
                    <Info size={14} aria-hidden="true" />
                  </button>
                </div>
              </div>
            ))
          )}
        </div>
      </div>
      {active && !loading && fileInfoTrack?.albumKey === infoAlbumKey && tracks.includes(fileInfoTrack.track) && <PlexFileInfoModal track={fileInfoTrack.track} onClose={() => setFileInfoTrack(null)} />}
      {playlistItems && (
        <PlaylistPickerModal
          items={playlistItems}
          title={
            playlistItems.length === 1
              ? t("playlists.addTrack")
              : t("playlists.addAlbum")
          }
          onClose={() => setPlaylistItems(null)}
        />
      )}
    </div>
  );
};

export default AlbumView;
