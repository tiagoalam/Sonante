import React, { useEffect, useState } from "react";
import { ListMusic, LoaderCircle, Plus, X } from "lucide-react";
import { useTranslation } from "react-i18next";

import {
  isDuplicatePlaylistNameError,
  playlistsService,
} from "../services/playlists";
import type { NewPlaylistItem, Playlist } from "../types/playlist";

interface Props {
  items: NewPlaylistItem[];
  title: string;
  onClose: () => void;
}

export const PlaylistPickerModal: React.FC<Props> = ({ items, title, onClose }) => {
  const { t } = useTranslation();
  const [playlists, setPlaylists] = useState<Playlist[]>([]);
  const [newName, setNewName] = useState("");
  const [creating, setCreating] = useState(false);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    playlistsService
      .list()
      .then((result) => {
        if (!disposed) setPlaylists(result);
      })
      .catch((cause) => {
        console.error("Falha ao listar playlists:", cause);
        if (!disposed) setError(t("playlists.loadError"));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [t]);

  const addToPlaylist = async (playlistId: string) => {
    setBusy(true);
    setError(null);
    try {
      await playlistsService.addItems(playlistId, items);
      onClose();
    } catch (cause) {
      console.error("Falha ao adicionar à playlist:", cause);
      setError(t("playlists.addError"));
    } finally {
      setBusy(false);
    }
  };

  const createAndAdd = async (event: React.FormEvent) => {
    event.preventDefault();
    const name = newName.trim();
    if (!name) {
      setError(t("playlists.nameRequired"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await playlistsService.createWithItems(name, items);
      onClose();
    } catch (cause) {
      console.error("Falha ao criar playlist e adicionar faixas:", cause);
      setError(
        isDuplicatePlaylistNameError(cause)
          ? t("playlists.duplicateName")
          : t("playlists.createAndAddError"),
      );
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm"
      role="dialog"
      aria-modal="true"
      aria-label={title}
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !busy) onClose();
      }}
    >
      <div className="w-full max-w-md overflow-hidden rounded-2xl border border-[#333333] bg-[#171717] shadow-2xl">
        <header className="flex items-center justify-between border-b border-[#292929] px-5 py-4">
          <div className="min-w-0">
            <h2 className="truncate text-base font-bold text-white">{title}</h2>
            <p className="mt-1 text-xs text-[#777777]">
              {t("playlists.addItemCount", { count: items.length })}
            </p>
          </div>
          <button
            type="button"
            onClick={onClose}
            disabled={busy}
            className="rounded-lg p-2 text-[#888888] hover:bg-[#252525] hover:text-white disabled:opacity-50"
            title={t("playlists.cancel")}
          >
            <X size={17} />
          </button>
        </header>

        {error && (
          <div className="mx-5 mt-4 rounded-lg border border-red-400/20 bg-red-400/10 px-3 py-2 text-xs text-red-200">
            {error}
          </div>
        )}

        <div className="max-h-72 overflow-y-auto p-3">
          {loading ? (
            <div className="flex justify-center p-8 text-[#777777]">
              <LoaderCircle size={22} className="animate-spin" />
            </div>
          ) : (
            <div className="space-y-1">
              {playlists.map((playlist) => (
                <button
                  key={playlist.id}
                  type="button"
                  disabled={busy}
                  onClick={() => void addToPlaylist(playlist.id)}
                  className="flex w-full items-center gap-3 rounded-lg px-3 py-3 text-left hover:bg-[#242424] disabled:opacity-50"
                >
                  <ListMusic size={17} className="shrink-0 text-[#E5A00D]" />
                  <span className="min-w-0 flex-1 truncate text-sm font-semibold text-white">
                    {playlist.name}
                  </span>
                  <span className="text-[11px] text-[#666666]">
                    {t("playlists.itemCount", { count: playlist.items.length })}
                  </span>
                </button>
              ))}
              <button
                type="button"
                disabled={busy}
                onClick={() => setCreating(true)}
                className="flex w-full items-center gap-3 rounded-lg px-3 py-3 text-left text-[#E5A00D] hover:bg-[#242424] disabled:opacity-50"
              >
                <Plus size={17} />
                <span className="text-sm font-bold">{t("playlists.newPlaylist")}</span>
              </button>
            </div>
          )}
        </div>

        {creating && (
          <form onSubmit={createAndAdd} className="flex gap-2 border-t border-[#292929] p-4">
            <input
              autoFocus
              value={newName}
              onChange={(event) => setNewName(event.target.value)}
              disabled={busy}
              placeholder={t("playlists.namePlaceholder")}
              className="min-w-0 flex-1 rounded-lg border border-[#3A3A3A] bg-[#111111] px-3 py-2 text-sm text-white outline-none focus:border-[#E5A00D]"
            />
            <button
              type="submit"
              disabled={busy}
              className="rounded-lg bg-[#E5A00D] px-4 py-2 text-sm font-bold text-black disabled:opacity-50"
            >
              {busy ? <LoaderCircle size={16} className="animate-spin" /> : t("playlists.create")}
            </button>
          </form>
        )}
      </div>
    </div>
  );
};
