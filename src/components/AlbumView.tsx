import React, { useEffect, useState } from "react";
import {
  ArrowLeft,
  Play,
  Clock,
  Disc3,
  User,
  Sparkles,
  Heart,
} from "lucide-react";
import { PlexAlbum, PlexTrack, SelectedArtist } from "../types/plex";
import { FavoriteAlbum } from "../types/favorite";
import { plexService } from "../services/plex";
import { audioService } from "../services/audio";
import { favoritesService } from "../services/favorites";

interface Props {
  album: PlexAlbum;
  onBack: () => void;
  onSelectArtist?: (artist: SelectedArtist) => void;
  onToggleFavorite?: () => void;
}

export const AlbumView: React.FC<Props> = ({ album, onBack, onSelectArtist, onToggleFavorite }) => {
  const [tracks, setTracks] = useState<PlexTrack[]>([]);
  const [loading, setLoading] = useState(true);
  const [isFavorite, setIsFavorite] = useState(false);

  const albumKey = String(album.rating_key || "");

  useEffect(() => {
    setLoading(true);
    plexService
      .getAlbumTracks(albumKey)
      .then(setTracks)
      .catch(console.error)
      .finally(() => setLoading(false));

    favoritesService
      .getFavorites()
      .then((favs) => {
        setIsFavorite(favs.some((f) => f.source === "plex" && String(f.id) === albumKey));
      })
      .catch(console.error);
  }, [albumKey]);

  const handleToggleFav = async () => {
    if (!albumKey) return;
    const favItem: FavoriteAlbum = {
      id: albumKey,
      source: "plex",
      title: album.title,
      artist: album.artist,
      year: album.year != null ? String(album.year) : undefined,
      thumb: album.thumb || null,
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
      console.error("Erro ao favoritar álbum Plex:", err);
    }
  };

  const handlePlayTracks = (startIndex: number = 0) => {
    const metaTracks = tracks.map((t) => ({
      title: t.title,
      artist: album.artist,
      album: album.title,
      thumb: t.thumb || album.thumb || null,
      uri: t.play_uri,
    }));

    if (metaTracks.length > 0) {
      audioService.playTracks(metaTracks, startIndex);
    }
  };

  const formatDuration = (ms: number) => {
    const totalSeconds = Math.floor(ms / 1000);
    const minutes = Math.floor(totalSeconds / 60);
    const seconds = totalSeconds % 60;
    return `${minutes}:${seconds.toString().padStart(2, "0")}`;
  };

  return (
    <div className="flex-1 flex flex-col h-full bg-[#121212] overflow-hidden select-none">
      <div className="p-8 pb-4 border-b border-[#222222] flex items-center">
        <button
          onClick={onBack}
          className="flex items-center space-x-2 text-xs font-semibold text-[#888888] hover:text-white transition-colors cursor-pointer"
        >
          <ArrowLeft size={16} />
          <span>Voltar para a biblioteca</span>
        </button>
      </div>

      <div className="flex-1 overflow-y-auto p-8 space-y-8">
        <div className="flex items-end space-x-6">
          <div className="w-56 h-56 rounded-xl bg-[#202020] border border-[#2B2B2B] overflow-hidden shrink-0 shadow-2xl flex items-center justify-center">
            {album.thumb ? (
              <img
                src={album.thumb}
                alt={album.title}
                className="w-full h-full object-cover"
              />
            ) : (
              <Disc3 size={64} className="text-[#444444]" />
            )}
          </div>

          <div className="space-y-3">
            <span className="text-xs font-bold text-[#E5A00D] uppercase tracking-wider flex items-center space-x-1">
              <Sparkles size={13} />
              <span>Álbum Plex</span>
            </span>

            <h1 className="text-3xl font-black text-white">{album.title}</h1>

            <div className="flex items-center space-x-2 text-sm text-[#CCCCCC]">
              {album.artist_rating_key && onSelectArtist ? (
                <button
                  onClick={() =>
                    onSelectArtist({
                      rating_key: album.artist_rating_key!,
                      name: album.artist,
                    })
                  }
                  className="font-medium hover:text-[#E5A00D] transition-colors cursor-pointer flex items-center space-x-1.5"
                >
                  <User size={15} />
                  <span>{album.artist}</span>
                </button>
              ) : (
                <span className="font-medium">{album.artist}</span>
              )}

              {album.year && (
                <>
                  <span className="text-[#666666]">•</span>
                  <span className="text-[#888888]">{album.year}</span>
                </>
              )}

              <span className="text-[#666666]">•</span>
              <span className="text-[#888888]">{tracks.length} faixas</span>
            </div>

            <div className="pt-2 flex items-center space-x-3">
              <button
                onClick={() => handlePlayTracks(0)}
                disabled={loading || tracks.length === 0}
                className="flex items-center space-x-2 px-6 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-lg transition-transform active:scale-95 cursor-pointer disabled:opacity-50"
              >
                <Play size={16} fill="black" />
                <span>Tocar Álbum</span>
              </button>

              <button
                onClick={handleToggleFav}
                className="p-2.5 rounded-xl bg-[#1E1E1E] border border-[#2B2B2B] hover:bg-[#282828] text-white transition-colors cursor-pointer"
                title={isFavorite ? "Remover dos favoritos" : "Adicionar aos favoritos"}
              >
                <Heart
                  size={16}
                  className={isFavorite ? "text-[#E5A00D]" : "text-[#888888]"}
                  fill={isFavorite ? "#E5A00D" : "none"}
                />
              </button>
            </div>
          </div>
        </div>

        <div className="bg-[#141414] border border-[#222222] rounded-xl overflow-hidden divide-y divide-[#1D1D1D]">
          <div className="grid grid-cols-12 px-4 py-2.5 text-[11px] font-bold text-[#666666] uppercase tracking-wider bg-[#181818]">
            <span className="col-span-1 text-center">#</span>
            <span className="col-span-8">Título</span>
            <span className="col-span-3 text-right flex items-center justify-end space-x-1">
              <Clock size={12} />
              <span>Duração</span>
            </span>
          </div>

          {loading ? (
            <div className="p-8 text-center text-xs text-[#666666]">
              Carregando faixas do servidor...
            </div>
          ) : tracks.length === 0 ? (
            <div className="p-8 text-center text-xs text-[#666666]">
              Nenhuma faixa encontrada neste álbum.
            </div>
          ) : (
            tracks.map((track, idx) => (
              <div
                key={track.rating_key}
                onClick={() => handlePlayTracks(idx)}
                className="grid grid-cols-12 px-4 py-3 text-xs items-center hover:bg-[#1E1E1E] transition-colors cursor-pointer group"
              >
                <span className="col-span-1 text-center font-mono text-[#666666] group-hover:text-[#E5A00D]">
                  {track.index || idx + 1}
                </span>

                <div className="col-span-8 flex flex-col pr-2">
                  <span className="font-semibold text-white group-hover:text-[#E5A00D] transition-colors truncate">
                    {track.title}
                  </span>
                  <span className="text-[11px] text-[#777777] truncate">
                    {album.artist}
                  </span>
                </div>

                <span className="col-span-3 text-right font-mono text-[#888888]">
                {formatDuration(track.duration || 0)}
		</span>
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
};

export default AlbumView;
