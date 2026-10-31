import { useQueueInsertion } from "./QueueInsertionMenu";
import { plexQueueTrack } from "../utils/queueActions";
import {plexTrackDurationSeconds} from '../utils/plexTrackMetadata';
import { usePlaybackAttempt } from "../utils/usePlaybackAttempt";
import { PlaybackRequestNotice } from "./PlaybackRequestNotice";
import { logError } from "../utils/frontendDiagnostics";
import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ArrowLeft, Play, Pause, Disc3, ChevronDown, ChevronUp, User, ListPlus } from "lucide-react";
import { useTranslation } from "react-i18next";
import { PlexAlbum, PlexTrack, SelectedArtist } from "../types/plex";
import { PlaybackStatus } from "../types/audio";
import { plexService } from "../services/plex";
import { audioService } from "../services/audio";
import { PlexImage } from "./PlexImage";
import { PlaylistPickerModal } from "./PlaylistPickerModal";
import type { NewPlaylistItem } from "../types/playlist";

interface Props {
  artist: SelectedArtist;
  onBack: () => void;
  onSelectAlbum: (album: PlexAlbum) => void;
  status: PlaybackStatus;
  isPlaybackAvailable: boolean;
  requestIdentity?: object;
  active?: boolean;
  loadContent?: (artist: SelectedArtist) => Promise<{ albums: PlexAlbum[]; topTracks: PlexTrack[] }>;
}

export const ArtistView: React.FC<Props> = ({
  artist,
  onBack,
  onSelectAlbum,
  status,
  isPlaybackAvailable,
  requestIdentity,
  active = true,
  loadContent,
}) => {
  const { t } = useTranslation();
  const [loadedAlbums, setAlbums] = useState<PlexAlbum[]>([]);
  const [loadedTracks, setTopTracks] = useState<PlexTrack[]>([]);
  const [visibleTracksCount, setVisibleTracksCount] = useState<number>(5);
  const [loading, setLoading] = useState(true);
  const [playlistItems, setPlaylistItems] = useState<NewPlaylistItem[] | null>(null);
  const [loadedFor, setLoadedFor] = useState<{ key: string; identity: unknown } | null>(null);
  const [loadError, setLoadError] = useState<{ key: string; identity: unknown } | null>(null);
  const key = JSON.stringify([artist.server_id, artist.library_id, artist.rating_key]);
  const request = usePlaybackAttempt(key + String(active), "player.request", requestIdentity);
  const current = loadedFor?.key === key && loadedFor.identity === requestIdentity;
  const failed = loadError?.key === key && loadError.identity === requestIdentity;
  const queueContext = useMemo(() => ({}), [key, requestIdentity, loadedTracks, active, current, isPlaybackAvailable]);
  const queueActions = useQueueInsertion(queueContext, active && current && isPlaybackAvailable);
  const queueRequest = (track: PlexTrack) => ({ track: plexQueueTrack(track),
    album: track.album_rating_key ? async () => (await plexService.getAlbumTracks(track.album_rating_key!)).map(item => plexQueueTrack(item)) : undefined });
  const albums = current ? loadedAlbums : [], topTracks = current ? loadedTracks : [];
  const viewport = useRef<HTMLElement>(null), scroll = useRef(0), returnAlbum = useRef<HTMLButtonElement | null>(null);
  const lastContext = useRef({ key, identity: requestIdentity });
  useLayoutEffect(() => {
    if (lastContext.current.key !== key || lastContext.current.identity !== requestIdentity) { scroll.current = 0; returnAlbum.current = null; lastContext.current = { key, identity: requestIdentity }; }
    if (active && viewport.current) {
      viewport.current.scrollTop = scroll.current;
      if (returnAlbum.current) returnAlbum.current.focus({ preventScroll: true });
      else if (loadContent) viewport.current.querySelector<HTMLElement>("[data-artist-heading]")?.focus({ preventScroll: true });
      returnAlbum.current = null;
    }
  }, [active, key, requestIdentity, loadContent]);

  useEffect(() => {
    let cancelled = false;
    scroll.current = 0; returnAlbum.current = null;
    setLoading(true);
    setAlbums([]); setTopTracks([]); setLoadedFor(null); setLoadError(null); setVisibleTracksCount(5); setPlaylistItems(null);
    Promise.resolve().then(() => { if (cancelled) throw "PLEX_DISCOVER_CONTEXT"; return loadContent ? loadContent(artist) : Promise.all([
        plexService.getArtistAlbums(artist.rating_key), plexService.getArtistTopTracks(artist.rating_key),
      ]).then(([albums, topTracks]) => ({ albums, topTracks })); })
      .then(({ albums: fetchedAlbums, topTracks: fetchedTracks }) => {
        if (cancelled) return;
        setAlbums(fetchedAlbums);
        setTopTracks(fetchedTracks);
        setLoadedFor({ key, identity: requestIdentity });
      })
      .catch(error => { if (!cancelled) { setLoadError({ key, identity: requestIdentity }); logError("artist.load", error); } })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [key, requestIdentity, loadContent]);

  const formatTime = (ms: number) => {
    const totalSeconds = Math.floor(ms / 1000);
    const minutes = Math.floor(totalSeconds / 60);
    const seconds = totalSeconds % 60;
    return `${minutes}:${seconds.toString().padStart(2, "0")}`;
  };

  const isTrackActive = (track: PlexTrack) => {
    if (!isPlaybackAvailable || status.current_media?.kind !== "plex") return false;
    return (
      track.media_locator.kind === "plex" &&
      status.current_media.server_id === track.media_locator.server_id &&
      status.current_media.part_key === track.media_locator.part_key
    );
  };

  const artistThumb = loadContent ? artist.thumb : artist.thumb ?? albums.find((a) => a.thumb)?.thumb;

  const handlePlayTrack = async (index: number) => {
    if (!active || !current || !isPlaybackAvailable) return;
    const track = topTracks[index];
    if (!track) return;

    if (isTrackActive(track)) {
      audioService.togglePlay().catch(error => logError("player.toggle", error));
      return;
    }

    const metaTracks = topTracks.map((t) => ({
      title: t.title,
      artist: artist.name,
      album: t.album_title || "Single",
      plex_image: t.thumb || artistThumb || null,
      media_locator: t.media_locator,
      duration: plexTrackDurationSeconds(t),
    }));

    await request.run(intent => audioService.playTracks(metaTracks, index, intent));
  };

  const toPlaylistItem = (track: PlexTrack): NewPlaylistItem => ({
    media_locator: track.media_locator,
    metadata: {
      title: track.title,
      artist: track.artist || artist.name,
      album: track.album_title || "",
      duration: plexTrackDurationSeconds(track),
    },
  });

  return (
    <main ref={viewport} onScroll={event => { if (active) scroll.current = event.currentTarget.scrollTop; }}
      onKeyDown={event => { if ((event.target as HTMLElement).closest?.("[data-artist-album]")) event.stopPropagation(); }}
      className="flex-1 flex min-h-0 flex-col overflow-y-auto bg-gradient-to-b from-[var(--theme-surface-panel,#1C1812)] via-[var(--theme-surface-base,#121212)] to-[var(--theme-surface-base,#121212)] select-none p-8">
      <PlaybackRequestNotice failure={request.failure} />
      {queueActions.notice}{queueActions.menu}
      {/* Botão de Retorno */}
      <button
        onClick={onBack}
        className="flex items-center space-x-2 text-xs font-bold text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-accent,#E5A00D)] transition-colors mb-6 cursor-pointer w-fit"
      >
        <ArrowLeft size={16} />
        <span>{t("artistView.back")}</span>
      </button>

      {/* Cabeçalho do Artista */}
      <div className="flex items-end space-x-6 pb-8 border-b border-[color:var(--theme-border-soft,#252525)]">
        <div className="w-44 h-44 rounded-full bg-[var(--theme-surface-panel,#1C1C1C)] overflow-hidden shadow-2xl shrink-0 border-2 border-[color:var(--theme-border-soft,#2B2B2B)]">
          {artistThumb ? (
            <PlexImage image={artistThumb} cacheScope={requestIdentity} alt={artist.name} className="w-full h-full object-cover" />
          ) : (
            <div className="w-full h-full flex items-center justify-center text-[color:var(--theme-fg-faint,#444444)]">
              <User size={64} />
            </div>
          )}
        </div>

        <div className="flex flex-col justify-end space-y-2">
          <span className="text-[11px] font-bold text-[color:var(--theme-fg-accent,#E5A00D)] tracking-widest uppercase">
            {t("artistView.artist")}
          </span>
          <h2 data-artist-heading tabIndex={-1} className="text-4xl font-black text-[color:var(--theme-fg-primary,#ffffff)] tracking-tight leading-tight outline-none">
            {artist.name}
          </h2>
          <div className="flex items-center space-x-3 text-xs text-[color:var(--theme-fg-muted,#777777)] pt-1">
            <span>{t("artistView.albumsCount", { count: albums.length })}</span>
            <span>•</span>
            <span>{t("artistView.catalogedTracks", { count: topTracks.length })}</span>
          </div>
        </div>
      </div>

      {failed ? <p role="alert" className="mt-8 text-sm text-[color:var(--theme-fg-accent,#D8B980)]">{t("artistView.loadError")}</p> : loading || !current ? (
        <div className="h-40 flex items-center justify-center text-xs text-[color:var(--theme-fg-muted,#666666)]">
          {t("artistView.loading")}
        </div>
      ) : (
        <div className="mt-8 space-y-10">
          {/* Músicas Populares */}
          {topTracks.length > 0 && (
            <div>
              <div className="flex items-center justify-between mb-4">
                <h3 className="text-lg font-bold text-[color:var(--theme-fg-primary,#ffffff)] tracking-tight">
                  {t("artistView.popular")}
                </h3>
              </div>

              <div className="divide-y divide-[color:var(--theme-border-soft,#1A1A1A)]">
                {topTracks.slice(0, visibleTracksCount).map((track, idx) => {
                  const active = isTrackActive(track);
                  const isPlaying = active && status.state === "play";

                  return (
                    <div
                      key={track.rating_key}
                      onClick={() => handlePlayTrack(idx)}
                      onContextMenu={event => queueActions.open(event, queueRequest(track))}
                      aria-disabled={!isPlaybackAvailable}
                      className={`group grid grid-cols-[40px_48px_minmax(0,1fr)_140px] items-center px-4 py-2.5 rounded-lg text-xs transition-colors ${
                        isPlaybackAvailable ? "cursor-pointer" : "cursor-not-allowed opacity-60"
                      } ${
                        active ? "bg-[var(--theme-surface-accent,#251E10)] text-[color:var(--theme-fg-accent,#E5A00D)]" : "hover:bg-[var(--theme-surface-panel,#1A1A1A)] text-[color:var(--theme-fg-secondary,#CCCCCC)]"
                      }`}
                    >
                      <div className="flex items-center justify-center">
                        {isPlaying ? (
                          <div className="flex items-end space-x-0.5 h-3.5 w-3.5">
                            <span className="w-0.5 h-3 bg-[#E5A00D] animate-pulse rounded-full" />
                            <span className="w-0.5 h-1.5 bg-[#E5A00D] animate-pulse delay-75 rounded-full" />
                            <span className="w-0.5 h-3.5 bg-[#E5A00D] animate-pulse delay-150 rounded-full" />
                          </div>
                        ) : active ? (
                          <Pause size={14} className="text-[color:var(--theme-fg-accent,#E5A00D)]" />
                        ) : (
                          <>
                            <span className="group-hover:hidden text-[color:var(--theme-fg-muted,#666666)] font-mono">
                              {idx + 1}
                            </span>
                            <Play size={14} className="hidden group-hover:block text-[color:var(--theme-fg-primary,#ffffff)]" fill="white" />
                          </>
                        )}
                      </div>

                      <div className="w-8 h-8 rounded bg-[var(--theme-surface-raised,#202020)] overflow-hidden mr-3">
                        {track.thumb ? (
                          <PlexImage image={track.thumb} cacheScope={requestIdentity} alt="" className="w-full h-full object-cover" />
                        ) : (
                          <div className="w-full h-full flex items-center justify-center text-[color:var(--theme-fg-faint,#444444)]">
                            <Disc3 size={14} />
                          </div>
                        )}
                      </div>

                      <div className="flex flex-col truncate pr-4">
                        <span className={`truncate font-medium ${active ? "text-[color:var(--theme-fg-accent,#E5A00D)] font-bold" : "text-[color:var(--theme-fg-primary,#ffffff)]"}`}>
                          {track.title}
                        </span>
                        {track.album_title && (
                          <span className="text-[11px] text-[color:var(--theme-fg-muted,#666666)] truncate mt-0.5">
                            {track.album_title}
                          </span>
                        )}
                      </div>

                      <div className="flex items-center justify-end gap-3">
                        <span className={`font-mono text-[11px] ${active ? "text-[color:var(--theme-fg-accent,#E5A00D)]" : "text-[color:var(--theme-fg-muted,#777777)]"}`}>
                          {formatTime(track.duration_ms || 0)}
                        </span>
                        {queueActions.trigger(queueRequest(track))}
                        <button
                          type="button"
                          disabled={!active || !current}
                          onClick={(event) => {
                            event.stopPropagation();
                            setPlaylistItems([toPlaylistItem(track)]);
                          }}
                          className="rounded-md p-1.5 text-[color:var(--theme-fg-muted,#777777)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] hover:text-[color:var(--theme-fg-accent,#E5A00D)]"
                          title={t("playlists.addTrack")}
                        >
                          <ListPlus size={15} />
                        </button>
                      </div>
                    </div>
                  );
                })}
              </div>

              {topTracks.length > 5 && (
                <div className="mt-3 px-4">
                  {visibleTracksCount < topTracks.length ? (
                    <button
                      onClick={() => setVisibleTracksCount((prev) => Math.min(prev + 5, topTracks.length))}
                      className="flex items-center space-x-1.5 text-xs font-bold text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer"
                    >
                      <span>{t("artistView.showMore")}</span>
                      <ChevronDown size={14} />
                    </button>
                  ) : (
                    <button
                      onClick={() => setVisibleTracksCount(5)}
                      className="flex items-center space-x-1.5 text-xs font-bold text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer"
                    >
                      <span>{t("artistView.showLess")}</span>
                      <ChevronUp size={14} />
                    </button>
                  )}
                </div>
              )}
            </div>
          )}

          {/* Discografia com Cards Clicáveis e Links Reativos */}
          <div>
            <h3 className="text-lg font-bold text-[color:var(--theme-fg-primary,#ffffff)] tracking-tight mb-4">
              {t("artistView.discography")}
            </h3>
            <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">
              {albums.map((alb) => (
                <button type="button" data-artist-album={alb.rating_key}
                  key={alb.rating_key}
                  disabled={!active || !current}
                  onClick={event => { if (!active || !current) return; returnAlbum.current = event.currentTarget; onSelectAlbum(alb); }}
                  className="group flex flex-col cursor-pointer text-left rounded-lg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]"
                >
                  <div className="relative aspect-square w-full rounded-lg bg-[var(--theme-surface-raised,#202020)] overflow-hidden mb-2.5 shadow-md border border-[color:var(--theme-border-soft,#262626)] group-hover:border-[color:var(--theme-border-accent,#E5A00D)]/50 transition-all">
                    {alb.thumb ? (
                      <PlexImage
                        image={alb.thumb}
                        cacheScope={requestIdentity}
                        alt={alb.title}
                        className="w-full h-full object-cover transition-transform duration-300 group-hover:scale-105"
                        loading="lazy"
                      />
                    ) : (
                      <div className="w-full h-full flex items-center justify-center text-[color:var(--theme-fg-faint,#444444)]">
                        <Disc3 size={40} />
                      </div>
                    )}
                  </div>
                  <span
                    className="text-sm font-semibold text-[color:var(--theme-fg-primary,#ffffff)] truncate group-hover:text-[color:var(--theme-fg-accent,#E5A00D)] transition-colors"
                    title={alb.title}
                  >
                    {alb.title}
                  </span>
                  {alb.year && (
                    <span className="text-[11px] text-[color:var(--theme-fg-muted,#666666)] mt-0.5">{alb.year}</span>
                  )}
                </button>
              ))}
            </div>
          </div>
        </div>
      )}
      {playlistItems && active && current && (
        <PlaylistPickerModal
          items={playlistItems}
          title={t("playlists.addTrack")}
          onClose={() => setPlaylistItems(null)}
        />
      )}
    </main>
  );
};
