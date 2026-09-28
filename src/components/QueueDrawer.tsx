import React, { useEffect, useState } from "react";
import { X, ListMusic, Play, Disc3, Trash2 } from "lucide-react";
import { TrackMetadata, PlaybackStatus } from "../types/audio";
import { audioService } from "../services/audio";

interface Props {
  isOpen: boolean;
  onClose: () => void;
  status: PlaybackStatus;
}

export const QueueDrawer: React.FC<Props> = ({ isOpen, onClose, status }) => {
  const [queue, setQueue] = useState<TrackMetadata[]>([]);

  useEffect(() => {
    if (isOpen) {
      audioService.getQueue().then(setQueue).catch(console.error);
    }
  }, [isOpen, status.current_file]);

  if (!isOpen) return null;

  const handlePlayIndex = async (index: number) => {
    try {
      await audioService.playQueueIndex(index);
    } catch (err) {
      console.error("Falha ao selecionar faixa da fila:", err);
    }
  };

  const handleClearQueue = async () => {
    try {
      await audioService.clearQueue();
      setQueue([]);
    } catch (err) {
      console.error("Falha ao limpar a fila:", err);
    }
  };

  return (
    <div className="fixed inset-y-0 right-0 z-50 flex">
      {/* Fundo escuro semi-transparente */}
      <div
        className="fixed inset-0 bg-black/60 backdrop-blur-xs transition-opacity"
        onClick={onClose}
      />

      {/* Painel Lateral */}
      <div className="relative w-96 bg-[#161616] border-l border-[#262626] shadow-2xl flex flex-col z-10 select-none pb-24">
        {/* Cabeçalho */}
        <div className="p-5 border-b border-[#242424] flex items-center justify-between bg-[#1A1A1A]">
          <div className="flex items-center space-x-2.5">
            <ListMusic className="text-[#E5A00D]" size={20} />
            <h3 className="text-sm font-bold text-white tracking-wide">Fila de Reprodução</h3>
          </div>

          <div className="flex items-center space-x-2">
            <span className="text-[11px] font-mono text-[#888888] bg-[#222222] px-2 py-0.5 rounded-full">
              {queue.length} {queue.length === 1 ? "faixa" : "faixas"}
            </span>

            {queue.length > 0 && (
              <button
                onClick={handleClearQueue}
                className="p-1.5 text-[#888888] hover:text-[#FF4D4D] rounded-lg hover:bg-[#252525] transition-colors cursor-pointer"
                title="Limpar Fila de Reprodução"
              >
                <Trash2 size={15} />
              </button>
            )}

            <button
              onClick={onClose}
              className="p-1.5 text-[#888888] hover:text-white rounded-lg hover:bg-[#252525] transition-colors cursor-pointer"
            >
              <X size={16} />
            </button>
          </div>
        </div>

        {/* Lista de Faixas */}
        <div className="flex-1 overflow-y-auto p-4 space-y-1">
          {queue.length === 0 ? (
            <div className="h-full flex flex-col items-center justify-center text-[#666666] space-y-2">
              <Disc3 size={36} className="opacity-40" />
              <span className="text-xs">A fila de reprodução está vazia.</span>
            </div>
          ) : (
            queue.map((track, idx) => {
              const isActive =
                track.uri === status.current_file ||
                status.current_file.endsWith(track.uri) ||
                (status.title && track.title === status.title);

              return (
                <div
                  key={`${track.uri}-${idx}`}
                  onClick={() => handlePlayIndex(idx)}
                  className={`group flex items-center space-x-3 p-2.5 rounded-lg text-xs transition-colors cursor-pointer ${
                    isActive
                      ? "bg-[#2A2312] border border-[#E5A00D]/40"
                      : "hover:bg-[#1E1E1E] border border-transparent"
                  }`}
                >
                  {/* Posição / Equalizador / Play */}
                  <div className="w-6 flex items-center justify-center shrink-0">
                    {isActive && status.state === "play" ? (
                      <div className="flex items-end space-x-0.5 h-3.5 w-3.5">
                        <span className="w-0.5 h-3 bg-[#E5A00D] animate-pulse rounded-full" />
                        <span className="w-0.5 h-1.5 bg-[#E5A00D] animate-pulse delay-75 rounded-full" />
                        <span className="w-0.5 h-3.5 bg-[#E5A00D] animate-pulse delay-150 rounded-full" />
                      </div>
                    ) : (
                      <>
                        <span
                          className={`font-mono text-[11px] group-hover:hidden ${
                            isActive ? "text-[#E5A00D] font-bold" : "text-[#666666]"
                          }`}
                        >
                          {idx + 1}
                        </span>
                        <Play size={12} className="hidden group-hover:block text-white" fill="white" />
                      </>
                    )}
                  </div>

                  {/* Capa */}
                  <div className="w-10 h-10 rounded bg-[#202020] overflow-hidden shrink-0">
                    {track.thumb ? (
                      <img src={track.thumb} alt="" className="w-full h-full object-cover" />
                    ) : (
                      <div className="w-full h-full flex items-center justify-center text-[#444444]">
                        <Disc3 size={16} />
                      </div>
                    )}
                  </div>

                  {/* Informações da Faixa */}
                  <div className="flex flex-col min-w-0 flex-1 pr-1">
                    <span
                      className={`truncate font-semibold ${
                        isActive ? "text-[#E5A00D]" : "text-white"
                      }`}
                      title={track.title}
                    >
                      {track.title}
                    </span>
                    <span className="text-[11px] text-[#777777] truncate mt-0.5">
                      {track.artist ? `${track.artist} — ${track.album}` : track.album}
                    </span>
                  </div>
                </div>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
};
