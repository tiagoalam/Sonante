import React, { useEffect, useState, useRef } from "react";
import {
  Play,
  Pause,
  SkipBack,
  SkipForward,
  Volume2,
  VolumeX,
  Disc3,
  ListMusic,
  ShieldCheck,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { PlaybackStatus } from "../types/audio";
import { audioService } from "../services/audio";

interface Props {
  onToggleQueue: () => void;
  isQueueOpen: boolean;
  onNavigateToArtist?: (artistName: string) => void;
  onNavigateToAlbum?: () => void;
}

export const PlayerBar: React.FC<Props> = ({ onToggleQueue, isQueueOpen }) => {
  const { t } = useTranslation();
  const [status, setStatus] = useState<PlaybackStatus>({
    state: "stop",
    elapsed: 0.0,
    duration: 0.0,
    audio_format: "",
    current_file: "",
    title: "",
    artist: "",
    album: "",
    thumb: null,
    volume: 100,
    is_updating: false,
  });

  const [isSeeking, setIsSeeking] = useState(false);
  const [seekValue, setSeekValue] = useState(0.0);
  const [prevVolume, setPrevVolume] = useState(100);

  const isSeekingRef = useRef(false);
  useEffect(() => {
    isSeekingRef.current = isSeeking;
  }, [isSeeking]);

  useEffect(() => {
    const update = async () => {
      try {
        const s = await audioService.getStatus();
        setStatus(s);
        if (!isSeekingRef.current) {
          setSeekValue(s.elapsed);
        }
      } catch (err) {
        console.error("Erro ao sincronizar status na barra de reprodução:", err);
      }
    };

    update();
    const interval = setInterval(update, 1000);
    return () => clearInterval(interval);
  }, []);

  const handlePlayToggle = async () => {
    try {
      await audioService.togglePlay();
      const s = await audioService.getStatus();
      setStatus(s);
    } catch (err) {
      console.error("Erro no play/pause:", err);
    }
  };

  const handleNext = async () => {
    try {
      await audioService.next();
      const s = await audioService.getStatus();
      setStatus(s);
    } catch (err) {
      console.error("Erro na faixa seguinte:", err);
    }
  };

  const handlePrevious = async () => {
    try {
      await audioService.previous();
      const s = await audioService.getStatus();
      setStatus(s);
    } catch (err) {
      console.error("Erro na faixa anterior:", err);
    }
  };

  const handleSeekCommit = async (val: number) => {
    setIsSeeking(false);
    try {
      await audioService.seek(val);
      setSeekValue(val);
    } catch (err) {
      console.error("Erro ao buscar posição:", err);
    }
  };

  const handleVolumeChange = async (val: number) => {
    try {
      await audioService.setVolume(val);
      setStatus((prev) => ({ ...prev, volume: val }));
    } catch (err) {
      console.error("Erro ao alterar volume:", err);
    }
  };

  const handleToggleMute = async () => {
    const current = status.volume ?? 100;
    if (current > 0) {
      setPrevVolume(current);
      await handleVolumeChange(0);
    } else {
      await handleVolumeChange(prevVolume > 0 ? prevVolume : 100);
    }
  };

  const formatTime = (secs: number) => {
    if (!secs || isNaN(secs) || secs < 0) return "0:00";
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${s.toString().padStart(2, "0")}`;
  };

  // Trata faixas paradas ou títulos padrão enviados pelo backend
  const isNoTrack =
    !status.title ||
    status.state === "stop" ||
    status.title.toLowerCase().includes("nenhuma") ||
    status.title.toLowerCase().includes("no track");

  const displayTitle = isNoTrack ? t("player.noTrack") : status.title;
  const displayArtist = isNoTrack
    ? "Sonante Bit-Perfect"
    : status.artist || (status.album ? status.album : "Sonante Bit-Perfect");

  const isDsd = status.audio_format.toUpperCase().includes("DSD");
  const isHiRes =
    isDsd ||
    status.audio_format.includes("96") ||
    status.audio_format.includes("192") ||
    status.audio_format.includes("384") ||
    status.audio_format.includes("24-bit") ||
    status.audio_format.includes("32-bit");

  return (
    <footer className="h-20 bg-[#161616] border-t border-[#262626] flex items-center justify-between px-6 z-40 select-none">
      {/* 1. Metadados e Capa */}
      <div className="flex items-center space-x-3.5 w-1/4 min-w-[200px]">
        <div className="w-12 h-12 rounded-lg bg-[#202020] border border-[#2B2B2B] overflow-hidden shrink-0 flex items-center justify-center shadow-md">
          {status.thumb && !isNoTrack ? (
            <img src={status.thumb} alt="" className="w-full h-full object-cover" />
          ) : (
            <Disc3 size={24} className="text-[#444444]" />
          )}
        </div>

        <div className="flex flex-col min-w-0 pr-2">
          <span className="text-xs font-bold text-white truncate" title={displayTitle}>
            {displayTitle}
          </span>
          <span className="text-[11px] text-[#888888] truncate mt-0.5" title={displayArtist}>
            {displayArtist}
          </span>

          {status.audio_format && (
            <div className="flex items-center space-x-1.5 mt-1">
              <span
                className={`text-[9px] font-mono px-1.5 py-0.2 rounded font-bold uppercase tracking-wider ${
                  isHiRes
                    ? "bg-[#E5A00D]/20 text-[#E5A00D] border border-[#E5A00D]/30"
                    : "bg-[#252525] text-[#AAAAAA]"
                }`}
              >
                {status.audio_format}
              </span>
              <span
                className="text-[9px] font-mono text-[#4BB543] flex items-center space-x-0.5"
                title={t("player.exclusive")}
              >
                <ShieldCheck size={10} className="shrink-0" />
                <span>{t("player.bitPerfect")}</span>
              </span>
            </div>
          )}
        </div>
      </div>

      {/* 2. Controlos de Reprodução & Barra de Progresso */}
      <div className="flex flex-col items-center justify-center flex-1 max-w-xl px-4 space-y-1.5">
        <div className="flex items-center space-x-5">
          <button
            onClick={handlePrevious}
            className="text-[#888888] hover:text-white transition-colors cursor-pointer"
            title={t("player.previous")}
          >
            <SkipBack size={18} />
          </button>

          <button
            onClick={handlePlayToggle}
            className="w-9 h-9 rounded-full bg-white hover:bg-[#E5A00D] text-black flex items-center justify-center shadow-lg transition-transform active:scale-95 cursor-pointer"
            title={status.state === "play" ? t("player.pause") : t("player.play")}
          >
            {status.state === "play" ? (
              <Pause size={17} fill="black" />
            ) : (
              <Play size={17} className="ml-0.5" fill="black" />
            )}
          </button>

          <button
            onClick={handleNext}
            className="text-[#888888] hover:text-white transition-colors cursor-pointer"
            title={t("player.next")}
          >
            <SkipForward size={18} />
          </button>
        </div>

        {/* Barra de Progresso (Seek) */}
        <div className="w-full flex items-center space-x-2.5 text-[10px] font-mono text-[#777777]">
          <span className="w-8 text-right">
            {formatTime(isSeeking ? seekValue : status.elapsed)}
          </span>

          <input
            type="range"
            min={0}
            max={status.duration > 0 ? status.duration : 100}
            step={0.5}
            value={isSeeking ? seekValue : status.elapsed}
            disabled={status.duration <= 0}
            onMouseDown={() => setIsSeeking(true)}
            onChange={(e) => setSeekValue(parseFloat(e.target.value))}
            onMouseUp={(e) => handleSeekCommit(parseFloat((e.target as HTMLInputElement).value))}
            className="flex-1 h-1 bg-[#262626] rounded-lg appearance-none cursor-pointer accent-[#E5A00D] disabled:opacity-30 disabled:cursor-default"
          />

          <span className="w-8 text-left">{formatTime(status.duration)}</span>
        </div>
      </div>

      {/* 3. Volume e Botão de Gaveta de Fila */}
      <div className="flex items-center justify-end space-x-4 w-1/4 min-w-[180px]">
        <div className="flex items-center space-x-2">
          <button
            onClick={handleToggleMute}
            className="text-[#888888] hover:text-white transition-colors cursor-pointer"
            title={(status.volume ?? 100) > 0 ? t("player.mute") : t("player.unmute")}
          >
            {(status.volume ?? 100) === 0 ? <VolumeX size={16} /> : <Volume2 size={16} />}
          </button>

          <input
            type="range"
            min={0}
            max={100}
            value={status.volume ?? 100}
            onChange={(e) => handleVolumeChange(parseInt(e.target.value, 10))}
            className="w-20 h-1 bg-[#262626] rounded-lg appearance-none cursor-pointer accent-[#E5A00D]"
          />
        </div>

        <button
          onClick={onToggleQueue}
          className={`p-2 rounded-lg border transition-colors cursor-pointer ${
            isQueueOpen
              ? "bg-[#252014] border-[#E5A00D] text-[#E5A00D]"
              : "border-[#262626] text-[#888888] hover:text-white hover:bg-[#202020]"
          }`}
          title={t("player.queue")}
        >
          <ListMusic size={17} />
        </button>
      </div>
    </footer>
  );
};

export default PlayerBar;
