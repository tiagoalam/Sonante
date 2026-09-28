import React, { useEffect, useState } from "react";
import { X, Trash2, Disc3, Play, Volume2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { QueueTrack, PlaybackStatus } from "../types/audio";
import { audioService } from "../services/audio";

interface Props {
  isOpen: boolean;
  onClose: () => void;
  status: PlaybackStatus;
}

export const QueueDrawer: React.FC<Props> = ({ isOpen, onClose, status }) => {
  const { t } = useTranslation();
  const [queue, setQueue] = useState<QueueTrack[]>([]);
  const [loading, setLoading] = useState(false);

  const fetchQueue = async () => {
    try {
      const q = await audioService.getQueue();
      setQueue(q);
    } catch (err) {
      console.error("Falha ao carregar fila:", err);
    }
  };

  useEffect(() => {
    if (isOpen) {
      fetchQueue();
    }
  }, [isOpen, status.current_file]);

  const handlePlayIndex = async (index: number) => {
    try {
      await audioService.playQueueIndex(index);
    } catch (err) {
      console.error("Erro ao tocar índice da fila:", err);
    }
  };

  const handleClear = async () => {
    setLoading(true);
    try {
      await audioService.clearQueue();
      setQueue([]);
    } catch (err) {
      console.error("Erro ao limpar fila:", err);
    } finally {
      setLoading(false);
    }
  };

  const formatDuration = (secs?: number) => {
    if (!secs || isNaN(secs)) return "--:--";
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${s.toString().padStart(2, "0")}`;
  };

  if (!isOpen) return null;

  return (
    <aside className="fixed top-0 right-0 bottom-20 w-80 bg-[#161616] border-l border-[#262626] shadow-2xl z-40 flex flex-col select-none animate-in slide-in-from-right duration-200">
      {/* Topo da Gaveta */}
      <div className="p-4 border-b border-[#242424] flex items-center justify-between bg-[#191919]">
        <div>
          <h2 className="text-sm font-bold text-white tracking-wide">{t("queue.title")}</h2>
          <span className="text-[11px] text-[#777777]">
            {queue.length} {t("favorites.tracks")}
          </span>
        </div>

        <div className="flex items-center space-x-1.5">
          {queue.length > 0 && (
            <button
              onClick={handleClear}
              disabled={loading}
              className="p-1.5 text-[#888888] hover:text-[#FF4D4D] rounded-lg transition-colors cursor-pointer"
              title={t("queue.clear")}
            >
              <Trash2 size={16} />
            </button>
          )}

          <button
            onClick={onClose}
            className="p-1.5 text-[#888888] hover:text-white rounded-lg hover:bg-[#252525] transition-colors cursor-pointer"
          >
            <X size={18} />
          </button>
        </div>
      </div>
      {/* Lista de Faixas */}
      <div className="flex-1 overflow-y-auto divide-y divide-[#1D1D1D] p-1">
        {queue.length === 0 ? (
          <div className="h-48 flex flex-col items-center justify-center text-[#666666] space-y-2">
            <Disc3 size={32} className="opacity-40" />
            <span className="text-xs">{t("queue.empty")}</span>
	    </div>
        ) : (
          queue.map((item) => {
            const isCurrent =
              status.state === "play" &&
              status.title &&
              item.title &&
              status.title.toLowerCase() === item.title.toLowerCase();

            return (
              <div
                key={item.id}
                onClick={() => handlePlayIndex(item.pos)}
                className={`flex items-center justify-between p-2.5 rounded-lg transition-colors cursor-pointer group ${
                  isCurrent ? "bg-[#252014] text-[#E5A00D]" : "hover:bg-[#1C1C1C] text-white"
                }`}
              >
                <div className="flex items-center space-x-3 min-w-0 pr-2">
                  <span className="text-xs font-mono text-[#666666] w-5 text-right shrink-0">
                    {isCurrent ? (
                      <Volume2 size={13} className="text-[#E5A00D] animate-pulse" />
                    ) : (
                      item.pos + 1
                    )}
                  </span>

                  <div className="flex flex-col min-w-0">
                    <span className="text-xs font-medium truncate">{item.title || item.file}</span>
                    {item.artist && (
                      <span className="text-[11px] text-[#777777] truncate">{item.artist}</span>
                    )}
                  </div>
                </div>

                <div className="flex items-center space-x-2 shrink-0">
                  <span className="text-[11px] font-mono text-[#666666]">
                    {formatDuration(item.duration)}
                  </span>
                  <Play
                    size={12}
                    className="text-[#666666] group-hover:text-white opacity-0 group-hover:opacity-100 transition-opacity"
                  />
                </div>
              </div>
            );
          })
        )}
      </div>
    </aside>
  );
};

export default QueueDrawer;
