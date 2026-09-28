import React, { useEffect, useState } from "react";
import {
  Play,
  Pause,
  SkipBack,
  SkipForward,
  Volume2,
  VolumeX,
  ListMusic,
  Disc3,
} from "lucide-react";
import { PlaybackStatus } from "../types/audio";
import { audioService } from "../services/audio";

interface Props {
  onToggleQueue?: () => void;
  isQueueOpen?: boolean;
}

export const PlayerBar: React.FC<Props> = ({ onToggleQueue, isQueueOpen }) => {
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
  const [seekValue, setSeekValue] = useState(0);

  // Sincronização periódica da telemetria
  useEffect(() => {
    let isMounted = true;
    const update = async () => {
      try {
        const s = await audioService.getStatus();
        if (isMounted) {
          setStatus(s);
          if (!isSeeking) {
            setSeekValue(s.elapsed);
          }
        }
      } catch (err) {
        console.error("Erro na telemetria do player:", err);
      }
    };

    update();
    const interval = setInterval(update, 1000);
    return () => {
      isMounted = false;
      clearInterval(interval);
    };
  }, [isSeeking]);

  const handleTogglePlay = async () => {
    try {
      await audioService.togglePlay();
      const s = await audioService.getStatus();
      setStatus(s);
    } catch (err) {
      console.error("Falha ao alternar reprodução:", err);
    }
  };

  const handleNext = async () => {
    try {
      await audioService.next();
    } catch (err) {
      console.error("Falha ao avançar faixa:", err);
    }
  };

  const handlePrevious = async () => {
    try {
      await audioService.previous();
    } catch (err) {
      console.error("Falha ao retroceder faixa:", err);
    }
  };

  const handleSeekChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setSeekValue(parseFloat(e.target.value));
  };

  const handleSeekCommit = async () => {
    setIsSeeking(false);
    try {
      await audioService.seek(seekValue);
    } catch (err) {
      console.error("Falha ao buscar posição:", err);
    }
  };

  const handleVolumeChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const val = parseInt(e.target.value, 10);
    try {
      await audioService.setVolume(val);
      setStatus((prev) => ({ ...prev, volume: val }));
    } catch (err) {
      console.error("Falha ao ajustar volume:", err);
    }
  };

  const handleToggleMute = async () => {
    const currentVol = status.volume ?? 100;
    const nextVol = currentVol > 0 ? 0 : 100;
    try {
      await audioService.setVolume(nextVol);
      setStatus((prev) => ({ ...prev, volume: nextVol }));
    } catch (err) {
      console.error("Falha ao alternar mudo:", err);
    }
  };

  const formatTime = (secs: number) => {
    if (isNaN(secs) || secs < 0) return "0:00";
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${s.toString().padStart(2, "0")}`;
  };

  // Parser com blindagem contra tokens e URLs do Plex
  const formatAudioBadge = () => {
    const rawFormat = status.audio_format?.trim() || "";
    const file = status.current_file?.trim() || "";

    if (!rawFormat && !file) return null;

    // 1. Isola e descarta qualquer parâmetro de query (?X-Plex-Token=...)
    let cleanUrl = file;
    try {
      cleanUrl = decodeURIComponent(file);
    } catch (_) {}
    cleanUrl = cleanUrl.split(/[?#&]/)[0];

    const rawExt = cleanUrl.split(".").pop()?.toUpperCase() || "";
    const ext = rawExt.replace(/[^A-Z0-9]/g, "").slice(0, 5);
    const isDsdExtension = ext === "DSF" || ext === "DFF" || ext === "ISO";

    // 2. MPD reportando fluxo DSD direto (ex: "dsd64:2", "dsd128:2")
    if (rawFormat.toLowerCase().startsWith("dsd")) {
      const [dsdType] = rawFormat.split(":");
      const upper = dsdType.toUpperCase();
      let freq = "2.8 MHz";
      if (upper.includes("128")) freq = "5.6 MHz";
      else if (upper.includes("256")) freq = "11.2 MHz";
      else if (upper.includes("512")) freq = "22.5 MHz";

      return `${upper} • ${freq} / 1-BIT`;
    }

    // 3. Arquivo DSD em reprodução via DoP (DSD over PCM)
    if (isDsdExtension) {
      const [rateStr] = rawFormat.split(":");
      const rateNum = parseInt(rateStr, 10);
      if (rateNum === 176400 || rateNum === 176000) {
        return `DSD64 • 2.8 MHZ / DoP (1-BIT)`;
      } else if (rateNum === 352800 || rateNum === 352000) {
        return `DSD128 • 5.6 MHZ / DoP (1-BIT)`;
      }
      return `${ext} • DSD (DoP)`;
    }

    // 4. Fluxos PCM padrão (FLAC, WAV, ALAC, AIFF, etc.)
    const parts = rawFormat.split(":");
    if (parts.length >= 2) {
      const rateNum = parseInt(parts[0], 10);
      const bitDepth = parts[1];

      let codec = "PCM";
      if (["FLAC", "WAV", "AIFF", "ALAC", "M4A", "MP3", "OGG", "AAC"].includes(ext)) {
        codec = ext;
      }

      if (!isNaN(rateNum)) {
        const khzVal = (rateNum / 1000).toFixed(1).replace(".0", "");
        const bitsVal = !isNaN(parseInt(bitDepth, 10)) ? ` / ${bitDepth}-BIT` : "";
        return `${codec} ${khzVal} KHZ${bitsVal}`;
      }
    }

    return rawFormat || (ext ? `${ext} AUDIO` : null);
  };

  // Garante que o título exibido nunca contenha tokens
  const cleanTitle = (status.title || "").split(/[?&]/)[0];
  const badgeText = formatAudioBadge();

  return (
    <footer className="h-20 bg-[#161616] border-t border-[#262626] px-6 flex items-center justify-between z-40 select-none">
      {/* 1. Informações da Faixa e Capa */}
      <div className="flex items-center space-x-3.5 w-1/4 min-w-[200px]">
        <div className="w-12 h-12 rounded-lg bg-[#202020] border border-[#2B2B2B] overflow-hidden flex items-center justify-center shrink-0 shadow-sm">
          {status.thumb ? (
            <img src={status.thumb} alt="Capa" className="w-full h-full object-cover" />
          ) : (
            <Disc3 size={24} className="text-[#444444]" />
          )}
        </div>

        <div className="flex flex-col min-w-0 pr-2">
          <span
            className="text-xs font-bold text-white truncate"
            title={cleanTitle || "Nenhuma faixa em reprodução"}
          >
            {cleanTitle || "Nenhuma faixa em reprodução"}
          </span>
          <span
            className="text-[11px] text-[#888888] truncate mt-0.5"
            title={status.artist ? `${status.artist} — ${status.album}` : status.album || ""}
          >
            {status.artist
              ? `${status.artist}${status.album ? ` — ${status.album}` : ""}`
              : status.album || "--"}
          </span>
        </div>
      </div>

      {/* 2. Controles Centrais de Transporte e Barra de Progresso */}
      <div className="flex flex-col items-center justify-center flex-1 max-w-xl px-4">
        <div className="flex items-center space-x-5 mb-1.5">
          <button
            onClick={handlePrevious}
            className="text-[#888888] hover:text-white transition-colors cursor-pointer"
            title="Faixa Anterior (Seta Esquerda)"
          >
            <SkipBack size={18} />
          </button>

          <button
            onClick={handleTogglePlay}
            className="w-9 h-9 rounded-full bg-white hover:bg-[#E5A00D] text-black flex items-center justify-center shadow-md transition-all active:scale-95 cursor-pointer"
            title="Reproduzir / Pausar (Espaço)"
          >
            {status.state === "play" ? (
              <Pause size={17} fill="black" />
            ) : (
              <Play size={17} fill="black" className="ml-0.5" />
            )}
          </button>

          <button
            onClick={handleNext}
            className="text-[#888888] hover:text-white transition-colors cursor-pointer"
            title="Próxima Faixa (Seta Direita)"
          >
            <SkipForward size={18} />
          </button>
        </div>

        {/* Barra de Progresso */}
        <div className="w-full flex items-center space-x-2 text-[10px] font-mono text-[#777777]">
          <span className="w-8 text-right">
            {formatTime(isSeeking ? seekValue : status.elapsed)}
          </span>

          <input
            type="range"
            min={0}
            max={status.duration > 0 ? status.duration : 100}
            step={0.1}
            value={isSeeking ? seekValue : status.elapsed}
            onMouseDown={() => setIsSeeking(true)}
            onChange={handleSeekChange}
            onMouseUp={handleSeekCommit}
            className="flex-1 h-1 bg-[#2C2C2C] rounded-lg appearance-none cursor-pointer accent-[#E5A00D] hover:bg-[#383838] transition-colors"
          />

          <span className="w-8">{formatTime(status.duration)}</span>
        </div>
      </div>

      {/* 3. Lado Direito: Fila, Volume e Badge de Áudio */}
      <div className="flex items-center justify-end space-x-4 w-1/3 min-w-[280px]">
        {/* Botão Fila */}
        <button
          onClick={onToggleQueue}
          className={`p-1.5 rounded-lg transition-colors cursor-pointer ${
            isQueueOpen
              ? "text-[#E5A00D] bg-[#292212]"
              : "text-[#888888] hover:text-white hover:bg-[#202020]"
          }`}
          title="Fila de Reprodução"
        >
          <ListMusic size={18} />
        </button>

        {/* Volume */}
        <div className="flex items-center space-x-2">
          <button
            onClick={handleToggleMute}
            className="text-[#888888] hover:text-white transition-colors cursor-pointer"
            title="Mudo (M)"
          >
            {(status.volume ?? 100) === 0 ? <VolumeX size={17} /> : <Volume2 size={17} />}
          </button>

          <input
            type="range"
            min={0}
            max={100}
            value={status.volume ?? 100}
            onChange={handleVolumeChange}
            className="w-20 h-1 bg-[#2C2C2C] rounded-lg appearance-none cursor-pointer accent-[#E5A00D]"
          />

          <span className="text-[10px] font-mono text-[#666666] w-7 text-right">
            {status.volume ?? 100}%
          </span>
        </div>

        {/* Badge Hi-Res / DSD Limpo */}
        {badgeText && (
          <div className="border border-[#E5A00D]/40 bg-[#1F190D] px-2.5 py-1 rounded-md text-[10px] font-mono font-bold text-[#E5A00D] tracking-wider shrink-0 shadow-xs">
            {badgeText}
          </div>
        )}
      </div>
    </footer>
  );
};
