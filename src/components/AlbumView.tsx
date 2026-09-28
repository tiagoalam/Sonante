import React, { useEffect, useState } from "react";
import { ArrowLeft, Play, Pause, Disc3, Clock } from "lucide-react";
import { PlexAlbum, PlexTrack } from "../types/plex";
import { PlaybackStatus } from "../types/audio";
import { plexService } from "../services/plex";
import { audioService } from "../services/audio";

interface Props {
  album: PlexAlbum;
  onBack: () => void;
  onSelectArtist?: (artist: { rating_key: string; name: string }) => void;
}

export const AlbumView: React.FC<Props> = ({ album, onBack, onSelectArtist } ) => {
  const [tracks, setTracks] = useState<PlexTrack[]>([]);
  const [loading, setLoading] = useState(true);
  const [status, setStatus] = useState<PlaybackStatus | null>(null);

  // Carrega as faixas do álbum
  useEffect(() => {
    setLoading(true);
    plexService
      .getAlbumTracks(album.rating_key)
      .then(setTracks)
      .catch(console.error)
      .finally(() => setLoading(false));
  }, [album.rating_key]);

  // Telemetria ativa: sincroniza o status do MPD a cada segundo
  useEffect(() => {
    const updateStatus = async () => {
      try {
        const s = await audioService.getStatus();
        setStatus(s);
      } catch (err) {
        console.error("Erro ao obter status de áudio:", err);
      }
    };

    updateStatus();
    const interval = setInterval(updateStatus, 1000);
    return () => clearInterval(interval);
  }, []);

  // Formata duração de milissegundos para m:ss
  const formatTime = (ms: number) => {
    const totalSeconds = Math.floor(ms / 1000);
    const minutes = Math.floor(totalSeconds / 60);
    const seconds = totalSeconds % 60;
    return `${minutes}:${seconds.toString().padStart(2, "0")}`;
  };

  // Tempo total do álbum formatado
  const totalDurationMs = tracks.reduce((acc, t) => acc + t.duration_ms, 0);
  const totalMinutes = Math.floor(totalDurationMs / 60000);

  // Verifica se uma faixa específica é a que está no DAC agora
  const isTrackActive = (track: PlexTrack) => {
    if (!status || !status.current_file) return false;
    return (
      track.play_uri === status.current_file ||
      track.play_uri.endsWith(status.current_file) ||
      status.current_file.endsWith(track.title) ||
      status.current_file.includes(track.rating_key)
    );
  };

  // Verifica se alguma faixa deste álbum está tocando no momento
  const isAlbumPlaying = tracks.some(isTrackActive) && status?.state === "play";

  // Disparo de reprodução a partir de uma faixa específica
  const handlePlayTrack = async (index: number) => {
    const active = isTrackActive(tracks[index]);

    if (active) {
      audioService.togglePlay().catch(console.error);
      return;
    }

    const metaTracks = tracks.map((t) => ({
      title: t.title,
      artist: album.artist,
      album: album.title,
      thumb: t.thumb || album.thumb || null,
      uri: t.play_uri,
    }));

    try {
      await audioService.playTracks(metaTracks, index);
      const updated = await audioService.getStatus();
      setStatus(updated);
    } catch (err) {
      console.error("Falha ao tocar faixas:", err);
    }
  };
  // Botão mestre "Reproduzir Disco"
  const handleMasterPlayToggle = () => {
    if (isAlbumPlaying) {
      audioService.togglePlay().catch(console.error);
    } else if (tracks.length > 0) {
      handlePlayTrack(0);
    }
  };

  return (
    <main className="flex-1 flex flex-col overflow-y-auto bg-gradient-to-b from-[#1C1812] via-[#121212] to-[#121212] select-none p-8">
      {/* Botão de Retorno */}
      <button
        onClick={onBack}
        className="flex items-center space-x-2 text-xs font-bold text-[#888888] hover:text-[#E5A00D] transition-colors mb-6 cursor-pointer w-fit"
      >
        <ArrowLeft size={16} />
        <span>Voltar para a Biblioteca</span>
      </button>

      {/* Cabeçalho do Álbum */}
      <div className="flex items-end space-x-6 pb-8 border-b border-[#252525]">
        <div className="w-52 h-52 bg-[#1C1C1C] rounded-xl overflow-hidden shadow-2xl shrink-0 border border-[#2B2B2B]">
          {album.thumb ? (
            <img
              src={album.thumb}
              alt={album.title}
              className="w-full h-full object-cover"
            />
          ) : (
            <div className="w-full h-full flex items-center justify-center text-[#444444]">
              <Disc3 size={64} />
            </div>
          )}
        </div>

        <div className="flex flex-col justify-end space-y-2">
          <span className="text-[11px] font-bold text-[#E5A00D] tracking-widest uppercase">
            Álbum
          </span>
          <h2 className="text-3xl font-black text-white tracking-tight leading-tight">
            {album.title}
          </h2>
	  <button
            onClick={() => {
              if (album.artist_rating_key && onSelectArtist) {
                onSelectArtist({
                  rating_key: album.artist_rating_key,
                  name: album.artist,
                });
              }
            }}
            className="text-base font-semibold text-[#CCCCCC] hover:text-[#E5A00D] transition-colors text-left cursor-pointer w-fit"
          >
            {album.artist}
          </button>
          <div className="flex items-center space-x-3 text-xs text-[#777777] pt-1">
            {album.year && <span>{album.year}</span>}
            {album.year && <span>•</span>}
            <span>{tracks.length} {tracks.length === 1 ? "faixa" : "faixas"}</span>
            <span>•</span>
            <span>{totalMinutes} min</span>
          </div>

          <div className="pt-3">
            <button
              onClick={handleMasterPlayToggle}
              className="flex items-center space-x-2 px-6 py-2.5 rounded-full bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-lg transition-transform active:scale-95 cursor-pointer"
            >
              {isAlbumPlaying ? (
                <>
                  <Pause size={16} fill="black" />
                  <span>Pausar</span>
                </>
              ) : (
                <>
                  <Play size={16} fill="black" className="ml-0.5" />
                  <span>Reproduzir Disco</span>
                </>
              )}
            </button>
          </div>
        </div>
      </div>

      {/* Lista de Faixas */}
      <div className="mt-6 flex-1">
        {loading ? (
          <div className="h-40 flex items-center justify-center text-xs text-[#666666]">
            Carregando faixas...
          </div>
        ) : (
          <div className="w-full">
            {/* Cabeçalho da Tabela */}
            <div className="grid grid-cols-[48px_1fr_80px] px-4 py-2 border-b border-[#222222] text-[11px] font-bold text-[#666666] uppercase tracking-wider">
              <span className="text-center">#</span>
              <span>Título</span>
              <span className="text-right flex items-center justify-end">
                <Clock size={14} />
              </span>
            </div>

            {/* Linhas das Faixas */}
            <div className="divide-y divide-[#1A1A1A]">
              {tracks.map((track, idx) => {
                const active = isTrackActive(track);
                const isPlaying = active && status?.state === "play";

                return (
                  <div
                    key={track.rating_key}
                    onClick={() => handlePlayTrack(idx)}
                    className={`group grid grid-cols-[48px_1fr_80px] items-center px-4 py-3 rounded-lg text-xs transition-colors cursor-pointer ${
                      active
                        ? "bg-[#251E10] text-[#E5A00D]"
                        : "hover:bg-[#1A1A1A] text-[#CCCCCC]"
                    }`}
                  >
                    {/* Número da Faixa / Indicador Animado / Botão de Play */}
                    <div className="flex items-center justify-center">
                      {isPlaying ? (
                        /* Equalizador Animado */
                        <div className="flex items-end space-x-0.5 h-3.5 w-3.5">
                          <span className="w-0.5 h-3 bg-[#E5A00D] animate-pulse rounded-full" />
                          <span className="w-0.5 h-1.5 bg-[#E5A00D] animate-pulse delay-75 rounded-full" />
                          <span className="w-0.5 h-3.5 bg-[#E5A00D] animate-pulse delay-150 rounded-full" />
                        </div>
                      ) : active ? (
                        <Pause size={14} className="text-[#E5A00D]" />
                      ) : (
                        <>
                          <span className="group-hover:hidden text-[#666666] font-mono">
                            {track.track_index || idx + 1}
                          </span>
                          <Play
                            size={14}
                            className="hidden group-hover:block text-white"
                            fill="white"
                          />
                        </>
                      )}
                    </div>

                    {/* Nome da Faixa */}
                    <span
                      className={`truncate pr-4 font-medium ${
                        active ? "text-[#E5A00D] font-bold" : "text-white"
                      }`}
                    >
                      {track.title}
                    </span>

                    {/* Duração */}
                    <span
                      className={`text-right font-mono text-[11px] ${
                        active ? "text-[#E5A00D]" : "text-[#777777]"
                      }`}
                    >
                      {formatTime(track.duration_ms)}
                    </span>
                  </div>
                );
              })}
            </div>
          </div>
        )}
      </div>
    </main>
  );
};
