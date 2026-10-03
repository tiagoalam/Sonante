import React, { useEffect, useMemo, useState } from "react";
import {
  ArrowLeft,
  Check,
  ListMusic,
  LoaderCircle,
  Music2,
  Pencil,
  Plus,
  Trash2,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import { playlistsService } from "../services/playlists";
import type { Playlist } from "../types/playlist";

export const PlaylistsView: React.FC = () => {
  const { t, i18n } = useTranslation();
  const [playlists, setPlaylists] = useState<Playlist[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selected = useMemo(
    () => playlists.find((playlist) => playlist.id === selectedId) ?? null,
    [playlists, selectedId],
  );

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
      setError(t("playlists.createError"));
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
      setError(t("playlists.renameError"));
    } finally {
      setBusy(false);
    }
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

  return (
    <main className="flex flex-1 flex-col overflow-hidden bg-[#121212] select-none">
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

      <div className="flex-1 overflow-y-auto p-8">
        {loading ? (
          <div className="flex h-full items-center justify-center text-[#777777]">
            <LoaderCircle size={24} className="animate-spin" />
          </div>
        ) : selected ? (
          <section className="mx-auto flex h-full max-w-4xl flex-col">
            <div className="mb-6 flex items-center justify-between gap-4 rounded-xl border border-[#2B2B2B] bg-[#181818] p-4">
              <div>
                <p className="text-xs font-semibold uppercase tracking-wider text-[#666666]">
                  {t("playlists.details")}
                </p>
                <p className="mt-1 text-xs text-[#888888]">
                  {t("playlists.updated", { date: formatDate(selected.updated_at) })}
                </p>
              </div>
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={() => beginRename(selected)}
                  className="flex cursor-pointer items-center gap-2 rounded-lg border border-[#333333] bg-[#202020] px-3 py-2 text-xs font-semibold text-[#CCCCCC] hover:bg-[#2A2A2A] hover:text-white"
                >
                  <Pencil size={14} />
                  <span>{t("playlists.rename")}</span>
                </button>
                <button
                  type="button"
                  onClick={() => void deletePlaylist(selected)}
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

            <div className="flex flex-1 flex-col items-center justify-center rounded-2xl border border-dashed border-[#333333] bg-[#161616] px-8 py-16 text-center">
              <div className="mb-4 flex h-16 w-16 items-center justify-center rounded-2xl border border-[#E5A00D]/20 bg-[#E5A00D]/10 text-[#E5A00D]">
                <Music2 size={30} />
              </div>
              <h3 className="text-lg font-bold text-white">{t("playlists.emptyTitle")}</h3>
              <p className="mt-2 max-w-md text-sm leading-relaxed text-[#777777]">
                {t("playlists.emptyHint")}
              </p>
            </div>
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
