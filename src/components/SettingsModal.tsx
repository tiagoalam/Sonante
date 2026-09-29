import React, { useEffect, useState, useRef } from "react";
import {
  X,
  Check,
  Server,
  HardDrive,
  Sliders,
  ShieldCheck,
  FolderPlus,
  Trash2,
  RefreshCw,
  ExternalLink,
  ChevronDown,
  ChevronUp,
  LogOut,
  CheckCircle2,
  Languages,
  Radio,
  Info,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { AppConfig } from "../types/config";
import { AudioDevice } from "../types/audio";
import { PlexServerResource } from "../types/plex";
import { configService } from "../services/config";
import { audioService } from "../services/audio";
import { plexService } from "../services/plex";

interface Props {
  onClose: () => void;
  onSaved: () => void;
}

export const SettingsModal: React.FC<Props> = ({ onClose, onSaved }) => {
  const { t, i18n } = useTranslation();
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [selectedLang, setSelectedLang] = useState(i18n.language || "pt-BR");

  // Estados do Fluxo de Login Plex (PIN)
  const [pinCode, setPinCode] = useState<string | null>(null);
  const [pinId, setPinId] = useState<number | null>(null);
  const [isPollingPin, setIsPollingPin] = useState(false);
  const [discoveredServers, setDiscoveredServers] = useState<PlexServerResource[]>([]);
  const [loadingServers, setLoadingServers] = useState(false);
  const [showManualPlex, setShowManualPlex] = useState(false);
  const pollTimerRef = useRef<number | null>(null);

  useEffect(() => {
    Promise.all([configService.getConfig(), configService.getAudioDevices()])
      .then(([cfg, devs]) => {
        if (devs.length > 0 && (!cfg.alsa_device || !devs.some((d) => d.id === cfg.alsa_device))) {
          cfg.alsa_device = devs[0].id;
        }
        setConfig(cfg);
        setDevices(devs);
        if (cfg.plex_token && cfg.plex_token.trim().length > 0) {
          setLoadingServers(true);
          plexService
            .getServers(cfg.plex_token)
            .then(setDiscoveredServers)
            .catch(console.error)
            .finally(() => setLoadingServers(false));
        }
      })
      .catch(console.error)
      .finally(() => setLoading(false));
  }, []);

  const handleLanguageChange = (lang: string) => {
    setSelectedLang(lang);
    i18n.changeLanguage(lang);
    localStorage.setItem("sonante_lang", lang);
  };

  const handleAddFolder = async () => {
    if (!config) return;
    try {
      const selected = await audioService.pickDirectory();
      if (selected && !config.local_folders.includes(selected)) {
        setConfig({
          ...config,
          local_folders: [...config.local_folders, selected],
        });
      }
    } catch (err) {
      console.error("Falha ao selecionar pasta:", err);
    }
  };

  const handleRemoveFolder = (folderToRemove: string) => {
    if (!config) return;
    setConfig({
      ...config,
      local_folders: config.local_folders.filter((f) => f !== folderToRemove),
    });
  };

  const handleForceRescan = async () => {
    setScanning(true);
    try {
      await audioService.rescanLibrary();
      setTimeout(() => setScanning(false), 800);
    } catch (err) {
      console.error("Erro ao forçar varredura:", err);
      setScanning(false);
    }
  };

  const handleStartPlexAuth = async () => {
    setIsPollingPin(true);
    setPinCode(null);
    try {
      const pin = await plexService.createPin();
      setPinCode(pin.code);
      setPinId(pin.id);
      await plexService.openExternalUrl(pin.auth_url);
    } catch (err) {
      console.error("Erro ao gerar PIN do Plex:", err);
      setIsPollingPin(false);
      alert("Não foi possível conectar aos servidores do Plex.");
    }
  };

  const handleCancelPlexAuth = () => {
    setIsPollingPin(false);
    setPinId(null);
    setPinCode(null);
    if (pollTimerRef.current) clearInterval(pollTimerRef.current);
  };

  const handleDisconnectPlex = () => {
    if (!config) return;
    setConfig({
      ...config,
      plex_token: "",
      plex_url: "",
    });
    setDiscoveredServers([]);
  };

  useEffect(() => {
    if (!pinId || !isPollingPin || !config) return;

    pollTimerRef.current = window.setInterval(async () => {
      try {
        const token = await plexService.checkPin(pinId);
        if (token) {
          setIsPollingPin(false);
          setPinId(null);
          setPinCode(null);
          setConfig((prev) => (prev ? { ...prev, plex_token: token } : null));

          setLoadingServers(true);
          const servers = await plexService.getServers(token);
          setDiscoveredServers(servers);
          setLoadingServers(false);

          if (servers.length > 0) {
            setConfig((prev) => (prev ? { ...prev, plex_url: servers[0].chosen_uri } : null));
          }
        }
      } catch (err) {
        console.error("Erro no polling do PIN Plex:", err);
      }
    }, 1500);

    return () => {
      if (pollTimerRef.current) clearInterval(pollTimerRef.current);
    };
  }, [pinId, isPollingPin, config]);

  const handleSave = async () => {
    if (!config) return;
    setSaving(true);
    try {
      await configService.saveConfig(config);
      await audioService.rescanLibrary();
      onSaved();
      onClose();
    } catch (err) {
      console.error("Falha ao gravar configurações:", err);
      alert("Erro ao gravar as configurações.");
    } finally {
      setSaving(false);
    }
  };

  if (loading || !config) return null;

  const isExclusive = (config.audio_output_type || "alsa") === "alsa";
  const isSelectedInList = devices.some((d) => d.id === config.alsa_device);
  const isPlexConnected = Boolean(config.plex_token && config.plex_token.trim().length > 0);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-xs select-none p-4">
      <div className="bg-[#181818] border border-[#2B2B2B] rounded-xl w-full max-w-2xl max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
        {/* Cabeçalho */}
        <div className="px-6 py-4 border-b border-[#262626] flex items-center justify-between bg-[#1D1D1D]">
          <div className="flex items-center space-x-2.5">
            <Sliders size={20} className="text-[#E5A00D]" />
            <h2 className="text-base font-bold text-white tracking-wide">{t("settings.title")}</h2>
          </div>
          <button
            onClick={onClose}
            className="p-1.5 text-[#888888] hover:text-white rounded-lg hover:bg-[#252525] transition-colors cursor-pointer"
          >
            <X size={18} />
          </button>
        </div>

        {/* Corpo com Scroll */}
        <div className="p-6 overflow-y-auto space-y-6 flex-1 text-xs">
          {/* Seção 0: Idioma da Interface */}
          <div className="space-y-3">
            <h3 className="text-[11px] font-bold text-[#E5A00D] uppercase tracking-wider flex items-center space-x-1.5">
              <Languages size={14} />
              <span>{t("settings.language")}</span>
            </h3>
            <div className="flex space-x-3">
              <button
                type="button"
                onClick={() => handleLanguageChange("pt-BR")}
                className={`flex-1 py-2.5 px-4 rounded-xl border font-semibold text-xs transition-all cursor-pointer ${
                  selectedLang.startsWith("pt")
                    ? "border-[#E5A00D] bg-[#221B0E] text-[#E5A00D]"
                    : "border-[#2B2B2B] bg-[#121212] text-[#888888] hover:text-white"
                }`}
              >
                Português (Brasil)
              </button>

              <button
                type="button"
                onClick={() => handleLanguageChange("en")}
                className={`flex-1 py-2.5 px-4 rounded-xl border font-semibold text-xs transition-all cursor-pointer ${
                  selectedLang.startsWith("en")
                    ? "border-[#E5A00D] bg-[#221B0E] text-[#E5A00D]"
                    : "border-[#2B2B2B] bg-[#121212] text-[#888888] hover:text-white"
                }`}
              >
                English (US)
              </button>
            </div>
          </div>

          {/* Seção 1: Saída de Áudio (Opção A) */}
          <div className="space-y-3 pt-3 border-t border-[#242424]">
            <h3 className="text-[11px] font-bold text-[#E5A00D] uppercase tracking-wider">
              {t("settings.audioOutput")}
            </h3>

            {/* Alternador de Modos Claros */}
            <div>
              <label className="block text-[#CCCCCC] font-semibold mb-2 text-xs">
                {t("settings.audioBackend")}
              </label>
              <div className="grid grid-cols-2 gap-3">
                <button
                  type="button"
                  onClick={() => setConfig({ ...config, audio_output_type: "alsa" })}
                  className={`p-3 rounded-xl text-left border transition-all cursor-pointer flex flex-col justify-between ${
                    isExclusive
                      ? "bg-[#221B0E] border-[#E5A00D] text-white"
                      : "bg-[#141414] border-[#2A2A2A] text-[#888888] hover:border-[#3A3A3A] hover:text-white"
                  }`}
                >
                  <div className="flex items-center space-x-2">
                    <ShieldCheck size={16} className={isExclusive ? "text-[#E5A00D]" : "text-[#777777]"} />
                    <span className="font-bold text-xs">{t("settings.alsaMode")}</span>
                  </div>
                  <span className="text-[10px] text-[#777777] mt-1 line-clamp-2">
                    hw:CARD,DEV • Direct ALSA
                  </span>
                </button>

                <button
                  type="button"
                  onClick={() => setConfig({ ...config, audio_output_type: "pipewire" })}
                  className={`p-3 rounded-xl text-left border transition-all cursor-pointer flex flex-col justify-between ${
                    !isExclusive
                      ? "bg-[#221B0E] border-[#E5A00D] text-white"
                      : "bg-[#141414] border-[#2A2A2A] text-[#888888] hover:border-[#3A3A3A] hover:text-white"
                  }`}
                >
                  <div className="flex items-center space-x-2">
                    <Radio size={16} className={!isExclusive ? "text-[#E5A00D]" : "text-[#777777]"} />
                    <span className="font-bold text-xs">{t("settings.pipewireMode")}</span>
                  </div>
                  <span className="text-[10px] text-[#777777] mt-1 line-clamp-2">
                    PipeWire / PulseAudio / dmix
                  </span>
                </button>
              </div>
            </div>

            {/* Painel Contextual do Modo Selecionado */}
            {isExclusive ? (
              <div className="space-y-2.5 pt-1">
                <label className="block text-[#CCCCCC] font-semibold text-xs">
                  {t("settings.alsaBitPerfect")}
                </label>
                <select
                  value={config.alsa_device}
                  onChange={(e) => setConfig({ ...config, alsa_device: e.target.value })}
                  className="w-full bg-[#121212] border border-[#333333] rounded-lg px-3 py-2 text-white outline-none focus:border-[#E5A00D] cursor-pointer text-sm"
                >
                  {!isSelectedInList && config.alsa_device && (
                    <option value={config.alsa_device} className="bg-[#1A1A1A] text-white py-1">
                      {t("settings.currentDevice", { id: config.alsa_device })}
                    </option>
                  )}
                  {devices.map((dev) => (
                    <option key={dev.id} value={dev.id} className="bg-[#1A1A1A] text-white py-1">
                      {dev.name}
                    </option>
                  ))}
                </select>

                <div className="p-2.5 bg-[#171717] border border-[#262626] rounded-lg text-[11px] text-[#888888] leading-relaxed flex items-start space-x-2">
                  <Info size={14} className="text-[#E5A00D] shrink-0 mt-0.5" />
                  <span>{t("settings.exclusiveNotice")}</span>
                </div>
              </div>
            ) : (
              <div className="p-3 bg-[#141414] border border-[#262626] rounded-xl space-y-2 text-xs">
                <div className="flex items-center space-x-2 text-[#E5A00D] font-bold">
                  <CheckCircle2 size={15} />
                  <span>{t("settings.sharedActiveNotice")}</span>
                </div>
                <p className="text-[11px] text-[#888888] leading-relaxed">
                  {t("settings.pipewireNotice")}
                </p>
              </div>
            )}
          </div>

          {/* Seção 2: Pastas Locais */}
          <div className="space-y-3 pt-3 border-t border-[#242424]">
            <div className="flex items-center justify-between">
              <h3 className="text-[11px] font-bold text-[#E5A00D] uppercase tracking-wider flex items-center space-x-1.5">
                <HardDrive size={14} />
                <span>{t("settings.localFolders")}</span>
              </h3>

              <div className="flex items-center space-x-2">
                <button
                  type="button"
                  onClick={handleForceRescan}
                  disabled={scanning}
                  className="flex items-center space-x-1.5 px-2.5 py-1 rounded bg-[#242424] hover:bg-[#2C2C2C] text-[#CCCCCC] hover:text-white transition-colors cursor-pointer"
                  title={t("settings.checkNew")}
                >
                  <RefreshCw size={12} className={scanning ? "animate-spin text-[#E5A00D]" : ""} />
                  <span>{t("settings.checkNew")}</span>
                </button>

                <button
                  type="button"
                  onClick={handleAddFolder}
                  className="flex items-center space-x-1.5 px-3 py-1 rounded bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold transition-transform active:scale-95 cursor-pointer"
                >
                  <FolderPlus size={13} />
                  <span>{t("settings.addFolder")}</span>
                </button>
              </div>
            </div>

            <div className="bg-[#121212] border border-[#2B2B2B] rounded-lg p-2.5 max-h-36 overflow-y-auto space-y-1.5">
              {config.local_folders.length === 0 ? (
                <span className="text-[11px] text-[#666666] block text-center py-2">
                  {t("settings.noFolders")}
                </span>
              ) : (
                config.local_folders.map((f, idx) => (
                  <div
                    key={f}
                    className="flex items-center justify-between p-2 rounded bg-[#181818] border border-[#242424]"
                  >
                    <span className="truncate pr-2 text-white font-medium" title={f}>
                      <span className="text-[#777777] font-mono mr-1.5">{idx + 1}.</span>
                      {f}
                    </span>
                    <button
                      onClick={() => handleRemoveFolder(f)}
                      className="text-[#888888] hover:text-[#FF4D4D] p-1 transition-colors cursor-pointer"
                    >
                      <Trash2 size={14} />
                    </button>
                  </div>
                ))
              )}
            </div>
          </div>

          {/* Seção 3: Motor de Áudio */}
          <div className="space-y-3 pt-3 border-t border-[#242424]">
            <h3 className="text-[11px] font-bold text-[#E5A00D] uppercase tracking-wider flex items-center space-x-1.5">
              <ShieldCheck size={14} />
              <span>{t("settings.audioEngine")}</span>
            </h3>

            {/* Chave de DoP - Ativa apenas em modo Bit-Perfect */}
            <div
              className={`flex items-center justify-between p-3 rounded-lg border transition-opacity ${
                isExclusive
                  ? "bg-[#141414] border-[#262626]"
                  : "bg-[#111111] border-[#202020] opacity-50"
              }`}
            >
              <div className="flex flex-col pr-4">
                <span className="text-white font-semibold">{t("settings.dop")}</span>
                <span className="text-[11px] text-[#777777] mt-0.5">
                  {isExclusive ? t("settings.dopDesc") : t("settings.dopOnlyExclusive")}
                </span>
              </div>
              <input
                type="checkbox"
                disabled={!isExclusive}
                checked={isExclusive && config.dop_enabled}
                onChange={(e) => setConfig({ ...config, dop_enabled: e.target.checked })}
                className="w-4 h-4 accent-[#E5A00D] cursor-pointer shrink-0 disabled:cursor-not-allowed"
              />
            </div>

            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-[#CCCCCC] font-semibold mb-1">{t("settings.ramBuffer")}</label>
                <select
                  value={config.audio_buffer_size_kb}
                  onChange={(e) =>
                    setConfig({ ...config, audio_buffer_size_kb: parseInt(e.target.value, 10) })
                  }
                  className="w-full bg-[#121212] border border-[#333333] rounded-lg px-3 py-2 text-white outline-none focus:border-[#E5A00D] cursor-pointer"
                >
                  <option value={4096}>4 MB</option>
                  <option value={8192}>8 MB</option>
                  <option value={16384}>16 MB</option>
                  <option value={32768}>32 MB</option>
                </select>
              </div>

              <div>
                <label className="block text-[#CCCCCC] font-semibold mb-1">{t("settings.replayGain")}</label>
                <select
                  value={config.replay_gain}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      replay_gain: e.target.value as "off" | "track" | "album",
                    })
                  }
                  className="w-full bg-[#121212] border border-[#333333] rounded-lg px-3 py-2 text-white outline-none focus:border-[#E5A00D] cursor-pointer"
                >
                  <option value="off">{t("settings.rgOff")}</option>
                  <option value="album">{t("settings.rgAlbum")}</option>
                  <option value="track">{t("settings.rgTrack")}</option>
                </select>
              </div>
            </div>
          </div>

          {/* Seção 4: Conexão Plex */}
          <div className="space-y-3 pt-3 border-t border-[#242424]">
            <div className="flex items-center justify-between">
              <h3 className="text-[11px] font-bold text-[#E5A00D] uppercase tracking-wider flex items-center space-x-1.5">
                <Server size={14} />
                <span>{t("settings.plexConnection")}</span>
              </h3>

              {isPlexConnected && (
                <button
                  type="button"
                  onClick={handleDisconnectPlex}
                  className="flex items-center space-x-1 text-[11px] text-[#FF4D4D] hover:text-[#FF6666] font-semibold transition-colors cursor-pointer"
                >
                  <LogOut size={13} />
                  <span>{t("settings.disconnect")}</span>
                </button>
              )}
            </div>

            {!isPlexConnected ? (
              <div className="bg-[#121212] border border-[#2B2B2B] rounded-xl p-5 flex flex-col items-center justify-center text-center space-y-3">
                {isPollingPin ? (
                  <div className="flex flex-col items-center space-y-2.5">
                    <div className="flex items-center space-x-2 text-[#E5A00D]">
                      <RefreshCw size={18} className="animate-spin" />
                      <span className="font-bold text-white">{t("settings.waitingBrowser")}</span>
                    </div>

                    {pinCode && (
                      <div className="font-mono text-xl font-black text-[#E5A00D] bg-[#1C1810] border border-[#E5A00D]/40 px-4 py-1.5 rounded-lg tracking-widest">
                        {pinCode}
                      </div>
                    )}

                    <button
                      type="button"
                      onClick={handleCancelPlexAuth}
                      className="text-[11px] text-[#888888] hover:text-white underline cursor-pointer pt-1"
                    >
                      {t("settings.cancel")}
                    </button>
                  </div>
                ) : (
                  <>
                    <span className="text-[#888888]">{t("settings.noPlexConnected")}</span>
                    <button
                      type="button"
                      onClick={handleStartPlexAuth}
                      className="flex items-center space-x-2 px-5 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs transition-transform active:scale-95 cursor-pointer shadow"
                    >
                      <ExternalLink size={14} />
                      <span>{t("settings.connectPlex")}</span>
                    </button>
                  </>
                )}
              </div>
            ) : (
              <div className="space-y-3">
                <div className="flex items-center justify-between p-2.5 rounded-lg bg-[#141F14] border border-[#4BB543]/40 text-xs">
                  <div className="flex items-center space-x-2 text-[#4BB543]">
                    <CheckCircle2 size={16} />
                    <span className="font-bold">{t("settings.sessionActive")}</span>
                  </div>
                </div>

                <div className="space-y-2">
                  <label className="block text-[#CCCCCC] font-semibold">{t("settings.activeServer")}</label>
                  {loadingServers ? (
                    <div className="text-[#888888] flex items-center space-x-2 py-2">
                      <RefreshCw size={13} className="animate-spin text-[#E5A00D]" />
                      <span>{t("settings.searchingServers")}</span>
                    </div>
                  ) : discoveredServers.length === 0 ? (
                    <div className="text-[#888888] p-2 bg-[#121212] rounded-lg border border-[#262626]">
                      {config.plex_url || "Nenhum servidor selecionado."}
                    </div>
                  ) : (
                    <div className="space-y-1.5 max-h-36 overflow-y-auto">
                      {discoveredServers.map((srv) => {
                        const isSelected = config.plex_url === srv.chosen_uri;
                        const localConn = srv.connections.find((c) => c.local);
                        return (
                          <div
                            key={srv.client_identifier}
                            onClick={() => setConfig({ ...config, plex_url: srv.chosen_uri })}
                            className={`p-2.5 rounded-lg border transition-all cursor-pointer flex items-center justify-between ${
                              isSelected
                                ? "border-[#E5A00D] bg-[#221B0E]"
                                : "border-[#262626] bg-[#121212] hover:bg-[#181818]"
                            }`}
                          >
                            <div>
                              <span className="font-bold text-white block">{srv.name}</span>
                              <span className="text-[10px] text-[#777777] font-mono">
                                {localConn ? `${localConn.address}:${localConn.port} (LAN)` : srv.chosen_uri}
                              </span>
                            </div>
                            <input
                              type="radio"
                              checked={isSelected}
                              onChange={() => setConfig({ ...config, plex_url: srv.chosen_uri })}
                              className="accent-[#E5A00D] cursor-pointer"
                            />
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>
              </div>
            )}

            {/* Ajustes Manuais */}
            <div className="pt-2">
              <button
                type="button"
                onClick={() => setShowManualPlex(!showManualPlex)}
                className="flex items-center space-x-1.5 text-[11px] text-[#777777] hover:text-[#CCCCCC] transition-colors cursor-pointer"
              >
                {showManualPlex ? <ChevronUp size={13} /> : <ChevronDown size={13} />}
                <span>{t("settings.manualConfig")}</span>
              </button>

              {showManualPlex && (
                <div className="grid grid-cols-2 gap-3 pt-2 animate-in fade-in duration-100">
                  <div>
                    <label className="block text-[11px] text-[#999999] mb-1">{t("settings.serverUrl")}</label>
                    <input
                      type="text"
                      value={config.plex_url}
                      onChange={(e) => setConfig({ ...config, plex_url: e.target.value })}
                      placeholder="http://192.168.1.100:32400"
                      className="w-full bg-[#121212] border border-[#333333] rounded-lg px-3 py-1.5 text-white outline-none focus:border-[#E5A00D]"
                    />
                  </div>
                  <div>
                    <label className="block text-[11px] text-[#999999] mb-1">{t("settings.plexToken")}</label>
                    <input
                      type="password"
                      value={config.plex_token}
                      onChange={(e) => setConfig({ ...config, plex_token: e.target.value })}
                      placeholder="Token manual"
                      className="w-full bg-[#121212] border border-[#333333] rounded-lg px-3 py-1.5 text-white outline-none focus:border-[#E5A00D]"
                    />
                  </div>
                </div>
              )}
            </div>
          </div>
        </div>

        {/* Rodapé */}
        <div className="px-6 py-4 border-t border-[#262626] bg-[#1D1D1D] flex items-center justify-end space-x-3">
          <button
            onClick={onClose}
            className="px-4 py-2 rounded-lg text-xs font-semibold text-[#888888] hover:text-white hover:bg-[#262626] transition-colors cursor-pointer"
          >
            {t("settings.cancel")}
          </button>

          <button
            onClick={handleSave}
            disabled={saving}
            className="flex items-center space-x-2 px-5 py-2 rounded-lg bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-md transition-transform active:scale-95 cursor-pointer disabled:opacity-50"
          >
            <Check size={15} />
            <span>{saving ? t("wizard.starting") : t("settings.save")}</span>
          </button>
        </div>
      </div>
    </div>
  );
};

export default SettingsModal;
