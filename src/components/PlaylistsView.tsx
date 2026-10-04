import React, { useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowLeft,
  ArrowDown,
  ArrowUp,
  CircleAlert,
  Check,
  ListMusic,
  LoaderCircle,
  Music2,
  Pencil,
  Play,
  Plus,
  Shuffle,
  Trash2,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import {
  isDuplicatePlaylistNameError,
  playlistsService,
} from "../services/playlists";
import type { Playlist, PlaylistItemAvailability } from "../types/playlist";

type AvailabilityStatus = PlaylistItemAvailability["status"];

export const PlaylistsView: React.FC<{ isPlaybackAvailable: boolean }> = ({ isPlaybackAvailable }) => {
  const { t, i18n } = useTranslation();
  const [playlists, setPlaylists] = useState<Playlist[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [availability, setAvailability] = useState<Record<string, AvailabilityStatus>>({});
  const [resolving, setResolving] = useState(false);
  const [preparingPlayback, setPreparingPlayback] = useState(false);
  const [playbackFeedback, setPlaybackFeedback] = useState<string | null>(null);
  const playRequestInFlight = useRef(false);
  const selectedIdRef = useRef(selectedId);
  selectedIdRef.current = selectedId;

  const selected = useMemo(
    () => playlists.find((playlist) => playlist.id === selectedId) ?? null,
    [playlists, selectedId],
  );

  useEffect(() => {
    setPlaybackFeedback(null);
  }, [selectedId]);

  useEffect(() => {
    let disposed = false;
    playlistsService
      .list()
      .then((items) => {
        if (!disposed) setPlaylists(items);
      })
      .catch((cause) => {
        console.error("Falha ao carregar playlists:", cause);
        if (!disposed) setError(t("playlists.loadError"));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [t]);

  useEffect(() => {
    if (!selectedId) {
      setAvailability({});
      return;
    }
    let disposed = false;
    setResolving(true);
    playlistsService
      .resolveItems(selectedId)
      .then((statuses) => {
        if (disposed) return;
        setAvailability(
          Object.fromEntries(statuses.map((status) => [status.item_id, status.status])),
        );
        statuses
          .filter((status) => status.reason)
          .forEach((status) => console.warn("Item de playlist indisponível:", status.reason));
      })
      .catch((cause) => {
        console.error("Falha ao resolver itens da playlist:", cause);
        if (!disposed) setError(t("playlists.resolveError"));
      })
      .finally(() => {
        if (!disposed) setResolving(false);
      });
    return () => {
      disposed = true;
    };
  }, [selectedId, t]);

  const createPlaylist = async (event: React.FormEvent) => {
    event.preventDefault();
    const name = newName.trim();
    if (!name) {
      setError(t("playlists.nameRequired"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const created = await playlistsService.create(name);
      setPlaylists((current) => [...current, created]);
      setNewName("");
      setSelectedId(created.id);
    } catch (cause) {
      console.error("Falha ao criar playlist:", cause);
      setError(
        isDuplicatePlaylistNameError(cause)
          ? t("playlists.duplicateName")
          : t("playlists.createError"),
      );
    } finally {
      setBusy(false);
    }
  };

  const beginRename = (playlist: Playlist) => {
    setEditingId(playlist.id);
    setEditingName(playlist.name);
    setError(null);
  };

  const renamePlaylist = async (event: React.FormEvent, id: string) => {
    event.preventDefault();
    const name = editingName.trim();
    if (!name) {
      setError(t("playlists.nameRequired"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const updated = await playlistsService.rename(id, name);
      setPlaylists((current) =>
        current.map((playlist) => (playlist.id === updated.id ? updated : playlist)),
      );
      setEditingId(null);
    } catch (cause) {
      console.error("Falha ao renomear playlist:", cause);
      setError(
        isDuplicatePlaylistNameError(cause)
          ? t("playlists.duplicateName")
          : t("playlists.renameError"),
      );
    } finally {
      setBusy(false);
    }
  };

  const replacePlaylist = (updated: Playlist) => {
    setPlaylists((current) =>
      current.map((playlist) => (playlist.id === updated.id ? updated : playlist)),
    );
  };

  const removeItem = async (playlistId: string, itemId: string) => {
    setBusy(true);
    setError(null);
    try {
      replacePlaylist(await playlistsService.removeItem(playlistId, itemId));
      setAvailability((current) => {
        const next = { ...current };
        delete next[itemId];
        return next;
      });
    } catch (cause) {
      console.error("Falha ao remover item da playlist:", cause);
      setError(t("playlists.removeError"));
    } finally {
      setBusy(false);
    }
  };

  const moveItem = async (playlist: Playlist, index: number, offset: -1 | 1) => {
    const target = index + offset;
    if (target < 0 || target >= playlist.items.length) return;
    const orderedIds = playlist.items.map((item) => item.id);
    [orderedIds[index], orderedIds[target]] = [orderedIds[target], orderedIds[index]];
    setBusy(true);
    setError(null);
    try {
      replacePlaylist(await playlistsService.reorderItems(playlist.id, orderedIds));
    } catch (cause) {
      console.error("Falha ao reordenar playlist:", cause);
      setError(t("playlists.reorderError"));
    } finally {
      setBusy(false);
    }
  };

  const formatDuration = (duration?: number | null) => {
    if (duration == null || !Number.isFinite(duration)) return "--:--";
    const minutes = Math.floor(duration / 60);
    const seconds = Math.floor(duration % 60);
    return `${minutes}:${seconds.toString().padStart(2, "0")}`;
  };

  const deletePlaylist = async (playlist: Playlist) => {
    if (!window.confirm(t("playlists.deleteConfirm", { name: playlist.name }))) return;
    setBusy(true);
    setError(null);
    try {
      await playlistsService.delete(playlist.id);
      setPlaylists((current) => current.filter((item) => item.id !== playlist.id));
      if (selectedId === playlist.id) setSelectedId(null);
      if (editingId === playlist.id) setEditingId(null);
    } catch (cause) {
      console.error("Falha ao excluir playlist:", cause);
      setError(t("playlists.deleteError"));
    } finally {
      setBusy(false);
    }
  };

  const formatDate = (timestamp: number) =>
    new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium" }).format(timestamp);

  const playPlaylist = async (playlistId: string, startItemId?: string, shuffle = false) => {
    if (!isPlaybackAvailable || playRequestInFlight.current) return;
    playRequestInFlight.current = true;
    setPreparingPlayback(true);
    setError(null);
    setPlaybackFeedback(null);
    try {
      const result = await playlistsService.play(playlistId, startItemId, shuffle);
      if (selectedIdRef.current === playlistId && result.skipped_count > 0) {
        setPlaybackFeedback(t("playlists.skippedCount", { count: result.skipped_count }));
      }
    } catch (cause) {
      console.error("Falha ao tocar playlist:", cause);
      const message = String(cause);
      if (selectedIdRef.current === playlistId) setError(t(message.includes("playlist_no_playable_items")
        ? "playlists.noPlayableItems"
        : message.includes("playlist_selected_item_unavailable")
          ? "playlists.selectedItemUnavailable"
          : "playlists.playError"));
    } finally {
      playRequestInFlight.current = false;
      setPreparingPlayback(false);
    }
  };

  return (
    <main className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-[#121212] select-none">
      <header className="flex items-center justify-between gap-6 border-b border-[#222222] p-8 pb-4">
        <div className="flex min-w-0 items-center space-x-3">
          {selected && (
            <button
              type="button"
              onClick={() => {
                setSelectedId(null);
                setEditingId(null);
              }}
              className="mr-1 cursor-pointer rounded-lg border border-[#333333] bg-[#1E1E1E] p-1.5 text-white transition-colors hover:bg-[#2A2A2A]"
              title={t("playlists.back")}
            >
              <ArrowLeft size={16} />
            </button>
          )}
          <div className="min-w-0">
            <h2 className="flex items-center space-x-2 truncate text-2xl font-bold tracking-tight text-white">
              <ListMusic size={22} className="shrink-0 text-[#E5A00D]" />
              <span className="truncate">{selected?.name ?? t("playlists.title")}</span>
            </h2>
            <p className="mt-0.5 text-xs text-[#888888]">
              {selected
                ? t("playlists.itemCount", { count: selected.items.length })
                : t("playlists.count", { count: playlists.length })}
            </p>
          </div>
        </div>

        {!selected && (
          <form onSubmit={createPlaylist} className="flex w-full max-w-sm items-center gap-2">
            <input
              value={newName}
              onChange={(event) => setNewName(event.target.value)}
              placeholder={t("playlists.namePlaceholder")}
              disabled={busy}
              className="min-w-0 flex-1 rounded-lg border border-[#333333] bg-[#1A1A1A] px-3 py-2 text-sm text-white outline-none transition-colors placeholder:text-[#666666] focus:border-[#E5A00D] disabled:opacity-50"
              aria-label={t("playlists.namePlaceholder")}
            />
            <button
              type="submit"
              disabled={busy}
              className="flex cursor-pointer items-center gap-2 rounded-lg bg-[#E5A00D] px-4 py-2 text-sm font-bold text-black transition-colors hover:bg-[#F5B01D] disabled:cursor-not-allowed disabled:opacity-50"
            >
              {busy ? <LoaderCircle size={16} className="animate-spin" /> : <Plus size={16} />}
              <span>{t("playlists.create")}</span>
            </button>
          </form>
        )}
      </header>

      {error && (
        <div className="mx-8 mt-4 rounded-lg border border-red-400/20 bg-red-400/10 px-4 py-3 text-sm text-red-200">
          {error}
        </div>
      )}

      {playbackFeedback && (
        <div role="status" className="mx-8 mt-4 rounded-lg border border-[#E5A00D]/20 bg-[#E5A00D]/10 px-4 py-3 text-sm text-[#E5A00D]">
          {playbackFeedback}
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-y-auto p-8">
        {loading ? (
          <div className="flex h-full items-center justify-center text-[#777777]">
            <LoaderCircle size={24} className="animate-spin" />
          </div>
        ) : selected ? (
          <section className="mx-auto flex max-w-4xl flex-col">
            <div className="mb-6 flex items-center justify-between gap-4 rounded-xl border border-[#2B2B2B] bg-[#181818] p-4">
              <div>
                <p className="text-xs font-semibold uppercase tracking-wider text-[#666666]">
                  {t("playlists.details")}
                </p>
                <p className="mt-1 text-xs text-[#888888]">
                  {t("playlists.updated", { date: formatDate(selected.updated_at) })}
                </p>
              </div>
              <div className="flex flex-wrap items-center justify-end gap-2">
                <button
                  type="button"
                  onClick={() => void playPlaylist(selected.id)}
                  disabled={!isPlaybackAvailable || preparingPlayback || busy || selected.items.length === 0}
                  className="flex cursor-pointer items-center gap-2 rounded-lg bg-[#E5A00D] px-4 py-2 text-xs font-bold text-black hover:bg-[#F5B01D] disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {preparingPlayback ? <LoaderCircle size={15} className="animate-spin" /> : <Play size={15} fill="currentColor" />}
                  <span>{preparingPlayback ? t("playlists.preparingPlayback") : t("playlists.play")}</span>
                </button>
                <button
                  type="button"
                  onClick={() => void playPlaylist(selected.id, undefined, true)}
                  disabled={!isPlaybackAvailable || preparingPlayback || busy}
                  className="flex cursor-pointer items-center gap-2 rounded-lg border border-[#E5A00D] bg-[#202020] px-4 py-2 text-xs font-bold text-[#E5A00D] hover:bg-[#2A2A2A] disabled:cursor-not-allowed disabled:opacity-50"
                >
                  <Shuffle size={15} />
                  <span>{t("playlists.shuffle")}</span>
                </button>
                <button
                  type="button"
                  onClick={() => beginRename(selected)}
                  disabled={preparingPlayback}
                  className="flex cursor-pointer items-center gap-2 rounded-lg border border-[#333333] bg-[#202020] px-3 py-2 text-xs font-semibold text-[#CCCCCC] hover:bg-[#2A2A2A] hover:text-white"
                >
                  <Pencil size={14} />
                  <span>{t("playlists.rename")}</span>
                </button>
                <button
                  type="button"
                  onClick={() => void deletePlaylist(selected)}
                  disabled={preparingPlayback}
                  className="flex cursor-pointer items-center gap-2 rounded-lg border border-red-400/20 bg-red-400/5 px-3 py-2 text-xs font-semibold text-red-300 hover:bg-red-400/10"
                >
                  <Trash2 size={14} />
                  <span>{t("playlists.delete")}</span>
                </button>
              </div>
            </div>

            {editingId === selected.id && (
              <form
                onSubmit={(event) => void renamePlaylist(event, selected.id)}
                className="mb-6 flex max-w-md items-center gap-2"
              >
                <input
                  value={editingName}
                  onChange={(event) => setEditingName(event.target.value)}
                  autoFocus
                  disabled={busy}
                  className="min-w-0 flex-1 rounded-lg border border-[#E5A00D] bg-[#1A1A1A] px-3 py-2 text-sm text-white outline-none"
                  aria-label={t("playlists.rename")}
                />
                <button type="submit" disabled={busy} className="rounded-lg bg-[#E5A00D] p-2 text-black">
                  <Check size={17} />
                </button>
                <button
                  type="button"
                  onClick={() => setEditingId(null)}
                  className="rounded-lg border border-[#333333] bg-[#202020] p-2 text-[#AAAAAA]"
                >
                  <X size={17} />
                </button>
              </form>
            )}

            {selected.items.length === 0 ? (
              <div className="flex flex-1 flex-col items-center justify-center rounded-2xl border border-dashed border-[#333333] bg-[#161616] px-8 py-16 text-center">
                <div className="mb-4 flex h-16 w-16 items-center justify-center rounded-2xl border border-[#E5A00D]/20 bg-[#E5A00D]/10 text-[#E5A00D]">
                  <Music2 size={30} />
                </div>
                <h3 className="text-lg font-bold text-white">{t("playlists.emptyTitle")}</h3>
                <p className="mt-2 max-w-md text-sm leading-relaxed text-[#777777]">
                  {t("playlists.emptyHint")}
                </p>
              </div>
            ) : (
              <div className="rounded-xl border border-[#292929] bg-[#151515]">
                {resolving && (
                  <div className="flex items-center gap-2 border-b border-[#292929] px-4 py-2 text-xs text-[#888888]">
                    <LoaderCircle size={13} className="animate-spin" />
                    <span>{t("playlists.resolving")}</span>
                  </div>
                )}
                <div className="divide-y divide-[#242424]">
                  {selected.items.map((item, index) => {
                    const missing = availability[item.id] === "missing";
                    const unavailable = availability[item.id] === "unavailable";
                    return (
                      <article
                        key={item.id}
                        className={`flex items-center gap-4 px-4 py-3 ${missing ? "bg-red-400/5" : unavailable ? "bg-amber-400/5" : ""}`}
                      >
                        <span className="w-7 shrink-0 text-center font-mono text-xs text-[#666666]">
                          {index + 1}
                        </span>
                        <div className="min-w-0 flex-1">
                          <div className="flex items-center gap-2">
                            <h3 className={`truncate text-sm font-semibold ${missing || unavailable ? "text-[#AAAAAA]" : "text-white"}`}>
                              {item.metadata.title}
                            </h3>
                            {missing && (
                              <span className="flex shrink-0 items-center gap-1 rounded-full bg-red-400/10 px-2 py-0.5 text-[10px] font-bold text-red-300">
                                <CircleAlert size={11} />
                                {t("playlists.missing")}
                              </span>
                            )}
                            {unavailable && (
                              <span className="flex shrink-0 items-center gap-1 rounded-full bg-amber-400/10 px-2 py-0.5 text-[10px] font-bold text-amber-300">
                                <CircleAlert size={11} />
                                {t("playlists.unavailable")}
                              </span>
                            )}
                          </div>
                          <p className="truncate text-xs text-[#777777]">
                            {[item.metadata.artist, item.metadata.album].filter(Boolean).join(" • ")}
                          </p>
                        </div>
                        <span className="shrink-0 font-mono text-xs text-[#777777]">
                          {formatDuration(item.metadata.duration)}
                        </span>
                        <div className="flex shrink-0 items-center gap-1">
                          <button
                            type="button"
                            disabled={!isPlaybackAvailable || preparingPlayback || busy}
                            onClick={() => void playPlaylist(selected.id, item.id)}
                            className="rounded-md p-1.5 text-[#E5A00D] hover:bg-[#292929] disabled:cursor-not-allowed disabled:opacity-30"
                            title={t("playlists.playFromTrack")}
                            aria-label={t("playlists.playFromTrack")}
                          >
                            <Play size={15} fill="currentColor" />
                          </button>
                          <button
                            type="button"
                            disabled={busy || preparingPlayback || index === 0}
                            onClick={() => void moveItem(selected, index, -1)}
                            className="rounded-md p-1.5 text-[#888888] hover:bg-[#292929] hover:text-white disabled:opacity-25"
                            title={t("playlists.moveUp")}
                            aria-label={t("playlists.moveUp")}
                          >
                            <ArrowUp size={15} />
                          </button>
                          <button
                            type="button"
                            disabled={busy || preparingPlayback || index === selected.items.length - 1}
                            onClick={() => void moveItem(selected, index, 1)}
                            className="rounded-md p-1.5 text-[#888888] hover:bg-[#292929] hover:text-white disabled:opacity-25"
                            title={t("playlists.moveDown")}
                            aria-label={t("playlists.moveDown")}
                          >
                            <ArrowDown size={15} />
                          </button>
                          <button
                            type="button"
                            disabled={busy || preparingPlayback}
                            onClick={() => void removeItem(selected.id, item.id)}
                            className="rounded-md p-1.5 text-[#888888] hover:bg-red-400/10 hover:text-red-300 disabled:opacity-50"
                            title={t("playlists.removeTrack")}
                            aria-label={t("playlists.removeTrack")}
                          >
                            <Trash2 size={15} />
                          </button>
                        </div>
                      </article>
                    );
                  })}
                </div>
              </div>
            )}
          </section>
        ) : playlists.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center text-center">
            <ListMusic size={48} className="mb-4 text-[#444444]" />
            <h3 className="text-lg font-bold text-white">{t("playlists.noneTitle")}</h3>
            <p className="mt-2 max-w-sm text-sm text-[#777777]">{t("playlists.noneHint")}</p>
          </div>
        ) : (
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
            {playlists.map((playlist) => (
              <article
                key={playlist.id}
                onClick={() => setSelectedId(playlist.id)}
                className="group cursor-pointer rounded-xl border border-[#2A2A2A] bg-[#181818] p-5 transition-colors hover:border-[#4A3D1B] hover:bg-[#1D1D1D]"
              >
                {editingId === playlist.id ? (
                  <form
                    onSubmit={(event) => void renamePlaylist(event, playlist.id)}
                    onClick={(event) => event.stopPropagation()}
                    className="flex items-center gap-2"
                  >
                    <input
                      value={editingName}
                      onChange={(event) => setEditingName(event.target.value)}
                      autoFocus
                      disabled={busy}
                      className="min-w-0 flex-1 rounded-md border border-[#E5A00D] bg-[#121212] px-2.5 py-1.5 text-sm text-white outline-none"
                      aria-label={t("playlists.rename")}
                    />
                    <button type="submit" className="text-[#E5A00D]" title={t("playlists.save")}>
                      <Check size={17} />
                    </button>
                    <button type="button" onClick={() => setEditingId(null)} className="text-[#888888]" title={t("playlists.cancel")}>
                      <X size={17} />
                    </button>
                  </form>
                ) : (
                  <>
                    <div className="flex items-start justify-between gap-4">
                      <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-[#E5A00D]/10 text-[#E5A00D]">
                        <ListMusic size={20} />
                      </div>
                      <div className="flex items-center gap-1 opacity-60 transition-opacity group-hover:opacity-100">
                        <button
                          type="button"
                          onClick={(event) => {
                            event.stopPropagation();
                            beginRename(playlist);
                          }}
                          className="rounded-md p-1.5 text-[#999999] hover:bg-[#2B2B2B] hover:text-white"
                          title={t("playlists.rename")}
                        >
                          <Pencil size={14} />
                        </button>
                        <button
                          type="button"
                          onClick={(event) => {
                            event.stopPropagation();
                            void deletePlaylist(playlist);
                          }}
                          className="rounded-md p-1.5 text-[#999999] hover:bg-red-400/10 hover:text-red-300"
                          title={t("playlists.delete")}
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>
                    </div>
                    <h3 className="mt-4 truncate font-bold text-white">{playlist.name}</h3>
                    <p className="mt-1 text-xs text-[#777777]">
                      {t("playlists.itemCount", { count: playlist.items.length })}
                    </p>
                    <p className="mt-3 text-[10px] uppercase tracking-wide text-[#555555]">
                      {t("playlists.updated", { date: formatDate(playlist.updated_at) })}
                    </p>
                  </>
                )}
              </article>
            ))}
          </div>
        )}
      </div>
    </main>
  );
};
