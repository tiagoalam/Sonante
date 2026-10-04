import React, { useState, useEffect, useRef } from "react";
import {
  Sparkles,
  Sliders,
  FolderPlus,
  Server,
  Trash2,
  Check,
  ArrowRight,
  ArrowLeft,
  HardDrive,
  ShieldCheck,
  Disc3,
  ExternalLink,
  RefreshCw,
  Languages,
  Radio,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { AppConfig } from "../types/config";
import { AudioDevice } from "../types/audio";
import { PlexServerResource } from "../types/plex";
import { audioService } from "../services/audio";
import { configService } from "../services/config";
import { plexService } from "../services/plex";

interface Props {
  initialConfig: AppConfig;
  devices: AudioDevice[];
  onFinish: (newConfig: AppConfig) => void;
}

export const WelcomeWizard: React.FC<Props> = ({ initialConfig, devices, onFinish }) => {
  const { t, i18n } = useTranslation();
  const [step, setStep] = useState<number>(1);
  const [config, setConfig] = useState<AppConfig>({ ...initialConfig });

  const [useLocal, setUseLocal] = useState<boolean>(true);
  const [usePlex, setUsePlex] = useState<boolean>(false);
  const [isFinishing, setIsFinishing] = useState<boolean>(false);

  const [selectedLang, setSelectedLang] = useState(i18n.language || "pt-BR");

  const [pinCode, setPinCode] = useState<string | null>(null);
  const [pinId, setPinId] = useState<number | null>(null);
  const [isPollingPin, setIsPollingPin] = useState(false);
  const [discoveredServers, setDiscoveredServers] = useState<PlexServerResource[]>([]);
  const [selectedServerUri, setSelectedServerUri] = useState<string>(config.plex_url || "");
  const [loadingServers, setLoadingServers] = useState(false);
  const pollTimerRef = useRef<number | null>(null);

  useEffect(() => {
    if (devices.length > 0) {
      const exists = devices.some((d) => d.id === config.alsa_device);
      if (!config.alsa_device || !exists) {
        setConfig((prev) => ({ ...prev, alsa_device: devices[0].id }));
      }
    }
  }, [devices]);

  const handleLanguageChange = (lang: string) => {
    setSelectedLang(lang);
    i18n.changeLanguage(lang);
    localStorage.setItem("sonante_lang", lang);
  };

  const handleAddFolder = async () => {
    try {
      const selected = await audioService.pickDirectory();
      if (selected && !config.local_folders.includes(selected)) {
        setConfig((prev) => ({
          ...prev,
          local_folders: [...prev.local_folders, selected],
        }));
      }
    } catch (err) {
      console.error("Erro ao selecionar diretório:", err);
    }
  };

  const handleRemoveFolder = (folderToRemove: string) => {
    setConfig((prev) => ({
      ...prev,
      local_folders: prev.local_folders.filter((f) => f !== folderToRemove),
    }));
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
      console.error("Falha ao iniciar autenticação Plex:", err);
      setIsPollingPin(false);
      alert(t("wizard.plexConnectionError"));
    }
  };

  useEffect(() => {
    if (!pinId || !isPollingPin) return;

    pollTimerRef.current = window.setInterval(async () => {
      try {
        const token = await plexService.checkPin(pinId);
        if (token) {
          setIsPollingPin(false);
          setPinId(null);
          setConfig((prev) => ({ ...prev, plex_token: token }));

          setLoadingServers(true);
          const servers = await plexService.getServers(token);
          setDiscoveredServers(servers);
          setLoadingServers(false);

          if (servers.length > 0) {
            const server = servers[0];
            const defaultUri = server.chosen_uri;
            setSelectedServerUri(defaultUri);
            setConfig((prev) => ({
              ...prev,
              plex_url: defaultUri,
              plex_server_id: server.client_identifier,
              plex_server_name: server.name,
            }));
          }
        }
      } catch (err) {
        console.error("Erro ao verificar status do PIN:", err);
      }
    }, 1500);

    return () => {
      if (pollTimerRef.current) clearInterval(pollTimerRef.current);
    };
  }, [pinId, isPollingPin]);

  const handleSelectServer = (server: PlexServerResource) => {
    setSelectedServerUri(server.chosen_uri);
    setConfig((prev) => ({
      ...prev,
      plex_url: server.chosen_uri,
      plex_server_id: server.client_identifier,
      plex_server_name: server.name,
    }));
  };

  const handleComplete = async () => {
    setIsFinishing(true);
    try {
      const finalConfig: AppConfig = {
        ...config,
        first_run: false,
      };
      await configService.saveConfig(finalConfig);
      onFinish(finalConfig);
    } catch (err) {
      console.error("Falha ao salvar configuração inicial:", err);
      alert(t("wizard.saveError"));
    } finally {
      setIsFinishing(false);
    }
  };

  const totalSteps = 2 + (useLocal ? 1 : 0) + (usePlex ? 1 : 0) + 1;

  const nextStep = () => {
    if (step === 2 && !useLocal && !usePlex) {
      alert(t("wizard.selectSourceError"));
      return;
    }
    setStep((s) => s + 1);
  };

  const prevStep = () => {
    setStep((s) => Math.max(1, s - 1));
  };

  const isDirect = (config.audio_output_type || "alsa") === "alsa";

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-[#0C0C0C] select-none p-6 text-white font-sans">
      <div className="bg-[#161616] border border-[#2B2B2B] rounded-2xl w-full max-w-2xl min-h-[540px] flex flex-col shadow-2xl overflow-hidden animate-in fade-in duration-200">
        {/* Topo do Wizard */}
        <div className="px-8 py-5 border-b border-[#242424] flex items-center justify-between bg-[#191919]">
          <div className="flex items-center space-x-3">
            <div className="w-9 h-9 rounded-xl bg-[#E5A00D]/10 border border-[#E5A00D]/30 flex items-center justify-center text-[#E5A00D]">
              <Sparkles size={20} />
            </div>
            <div>
              <h1 className="text-base font-black tracking-wider text-[#E5A00D]">SONANTE</h1>
              <p className="text-[11px] text-[#888888]">{t("wizard.subtitle")}</p>
            </div>
          </div>

          <div className="flex items-center space-x-3">
            {/* Alternador de Idioma */}
            <div className="flex items-center space-x-1 bg-[#121212] border border-[#282828] rounded-lg p-0.5 text-[11px]">
              <Languages size={12} className="text-[#777777] ml-1.5 mr-0.5" />
              <button
                type="button"
                onClick={() => handleLanguageChange("pt-BR")}
                className={`px-2 py-0.5 rounded font-semibold transition-colors cursor-pointer ${
                  selectedLang.startsWith("pt") ? "bg-[#E5A00D] text-black" : "text-[#777777] hover:text-white"
                }`}
              >
                PT
              </button>
              <button
                type="button"
                onClick={() => handleLanguageChange("en")}
                className={`px-2 py-0.5 rounded font-semibold transition-colors cursor-pointer ${
                  selectedLang.startsWith("en") ? "bg-[#E5A00D] text-black" : "text-[#777777] hover:text-white"
                }`}
              >
                EN
              </button>
              <button
                type="button"
                onClick={() => handleLanguageChange("es")}
                className={`px-2 py-0.5 rounded font-semibold transition-colors cursor-pointer ${
                  selectedLang.startsWith("es") ? "bg-[#E5A00D] text-black" : "text-[#777777] hover:text-white"
                }`}
                title="Español"
              >
                ES
              </button>
            </div>

            <span className="text-xs font-mono text-[#888888]">
              {t("wizard.step", { step })}
            </span>
          </div>
        </div>

        {/* Conteúdo */}
        <div className="p-8 flex-1 flex flex-col justify-between">
          {/* ETAPA 1: ESCOLHA DO MODO DE SAÍDA (Opção A) */}
          {step === 1 && (
            <div className="space-y-5 animate-in fade-in duration-150">
              <div className="space-y-1.5">
                <h2 className="text-xl font-bold text-white flex items-center space-x-2">
                  <Sliders size={20} className="text-[#E5A00D]" />
                  <span>{t("wizard.step1Title")}</span>
                </h2>
                <p className="text-xs text-[#999999] leading-relaxed">
                  {t("wizard.step1Desc")}
                </p>
              </div>

              {/* Botões dos 2 Modos */}
              <div className="grid grid-cols-2 gap-3 pt-1">
                <button
                  type="button"
                  onClick={() => setConfig({ ...config, audio_output_type: "alsa" })}
                  className={`p-3.5 rounded-xl border-2 text-left transition-all cursor-pointer flex flex-col justify-between ${
                    isDirect
                      ? "border-[#E5A00D] bg-[#221B0E]"
                      : "border-[#262626] bg-[#141414] hover:bg-[#181818]"
                  }`}
                >
                  <div className="flex items-center space-x-2">
                    <ShieldCheck size={18} className={isDirect ? "text-[#E5A00D]" : "text-[#777777]"} />
                    <span className="font-bold text-xs text-white">{t("settings.alsaMode")}</span>
                  </div>
                  <span className="text-[10px] text-[#777777] mt-1">
                    hw:CARD,DEV • ALSA Direct
                  </span>
                </button>

                <button
                  type="button"
                  onClick={() => setConfig({ ...config, audio_output_type: "pipewire" })}
                  className={`p-3.5 rounded-xl border-2 text-left transition-all cursor-pointer flex flex-col justify-between ${
                    !isDirect
                      ? "border-[#E5A00D] bg-[#221B0E]"
                      : "border-[#262626] bg-[#141414] hover:bg-[#181818]"
                  }`}
                >
                  <div className="flex items-center space-x-2">
                    <Radio size={18} className={!isDirect ? "text-[#E5A00D]" : "text-[#777777]"} />
                    <span className="font-bold text-xs text-white">{t("settings.pipewireMode")}</span>
                  </div>
                  <span className="text-[10px] text-[#777777] mt-1">
                    PipeWire / PulseAudio
                  </span>
                </button>
              </div>

              {/* Configuração Contextual */}
              {isDirect ? (
                <div className="space-y-3 pt-1">
                  <div className="space-y-1.5">
                    <label className="block text-xs font-bold text-[#CCCCCC] uppercase tracking-wider">
                      {t("settings.alsaDevice")}
                    </label>
                    <select
                      value={config.alsa_device}
                      onChange={(e) => setConfig({ ...config, alsa_device: e.target.value })}
                      className="w-full bg-[#111111] border border-[#333333] rounded-xl px-4 py-3 text-xs text-white outline-none focus:border-[#E5A00D] cursor-pointer"
                    >
                      {devices.map((dev) => (
                        <option key={dev.id} value={dev.id} className="bg-[#1A1A1A] py-1">
                          {dev.name}
                        </option>
                      ))}
                    </select>
                  </div>

                  <div className="bg-[#1B1812] border border-[#E5A00D]/30 p-3.5 rounded-xl flex items-start space-x-3 text-xs text-[#CCCCCC]">
                    <ShieldCheck size={18} className="text-[#E5A00D] shrink-0 mt-0.5" />
                    <span>{t("wizard.step1Direct")}</span>
                  </div>
                </div>
              ) : (
                <div className="bg-[#141414] border border-[#262626] p-4 rounded-xl space-y-2 text-xs">
                  <div className="flex items-center space-x-2 text-[#E5A00D] font-bold">
                    <Check size={15} />
                    <span>{t("settings.sharedActiveNotice")}</span>
                  </div>
                  <p className="text-[11px] text-[#888888] leading-relaxed">
                    {t("wizard.step1Shared")}
                  </p>
                </div>
              )}
            </div>
          )}

          {/* ETAPA 2: FONTES DE MÍDIA */}
          {step === 2 && (
            <div className="space-y-5 animate-in fade-in duration-150">
              <div className="space-y-1.5">
                <h2 className="text-xl font-bold text-white">{t("wizard.step2Title")}</h2>
                <p className="text-xs text-[#999999]">{t("wizard.step2Desc")}</p>
              </div>

              <div className="grid grid-cols-2 gap-4 pt-2">
                <div
                  onClick={() => setUseLocal(!useLocal)}
                  className={`p-5 rounded-xl border-2 transition-all cursor-pointer flex flex-col justify-between ${
                    useLocal
                      ? "border-[#E5A00D] bg-[#221B0E]"
                      : "border-[#262626] bg-[#141414] hover:bg-[#1A1A1A]"
                  }`}
                >
                  <div className="space-y-2">
                    <div className="w-10 h-10 rounded-lg bg-[#252525] flex items-center justify-center text-[#E5A00D]">
                      <HardDrive size={20} />
                    </div>
                    <h3 className="font-bold text-sm text-white">{t("sidebar.local")}</h3>
                    <p className="text-[11px] text-[#888888] leading-relaxed">
                      {t("wizard.step2LocalDesc")}
                    </p>
                  </div>
                  <div className="pt-4 flex items-center justify-between text-xs">
                    <span className={useLocal ? "text-[#E5A00D] font-bold" : "text-[#666666]"}>
                      {useLocal ? t("wizard.enabled") : t("wizard.disabled")}
                    </span>
                    <input
                      type="checkbox"
                      checked={useLocal}
                      onChange={() => {}}
                      className="w-4 h-4 accent-[#E5A00D] cursor-pointer"
                    />
                  </div>
                </div>

                <div
                  onClick={() => setUsePlex(!usePlex)}
                  className={`p-5 rounded-xl border-2 transition-all cursor-pointer flex flex-col justify-between ${
                    usePlex
                      ? "border-[#E5A00D] bg-[#221B0E]"
                      : "border-[#262626] bg-[#141414] hover:bg-[#1A1A1A]"
                  }`}
                >
                  <div className="space-y-2">
                    <div className="w-10 h-10 rounded-lg bg-[#252525] flex items-center justify-center text-[#E5A00D]">
                      <Server size={20} />
                    </div>
                    <h3 className="font-bold text-sm text-white">{t("sidebar.plex")}</h3>
                    <p className="text-[11px] text-[#888888] leading-relaxed">
                      {t("wizard.step2PlexDesc")}
                    </p>
                  </div>
                  <div className="pt-4 flex items-center justify-between text-xs">
                    <span className={usePlex ? "text-[#E5A00D] font-bold" : "text-[#666666]"}>
                      {usePlex ? t("wizard.enabled") : t("wizard.disabled")}
                    </span>
                    <input
                      type="checkbox"
                      checked={usePlex}
                      onChange={() => {}}
                      className="w-4 h-4 accent-[#E5A00D] cursor-pointer"
                    />
                  </div>
                </div>
              </div>
            </div>
          )}

          {/* ETAPA 3: PASTAS LOCAIS */}
          {step === 3 && useLocal && (
            <div className="space-y-4 animate-in fade-in duration-150">
              <div className="flex items-center justify-between">
                <div>
                  <h2 className="text-xl font-bold text-white">{t("wizard.step3LocalTitle")}</h2>
                  <p className="text-xs text-[#999999] mt-0.5">{t("wizard.step3LocalDesc")}</p>
                </div>

                <button
                  type="button"
                  onClick={handleAddFolder}
                  className="flex items-center space-x-2 px-4 py-2 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-md transition-transform active:scale-95 cursor-pointer"
                >
                  <FolderPlus size={16} />
                  <span>{t("settings.addFolder")}</span>
                </button>
              </div>

              <div className="bg-[#111111] border border-[#262626] rounded-xl p-3 min-h-[160px] max-h-[220px] overflow-y-auto space-y-2">
                {config.local_folders.length === 0 ? (
                  <div className="h-32 flex flex-col items-center justify-center text-[#666666] space-y-2">
                    <Disc3 size={32} className="opacity-40" />
                    <span className="text-xs">{t("settings.noFolders")}</span>
                  </div>
                ) : (
                  config.local_folders.map((folder, idx) => (
                    <div
                      key={folder}
                      className="flex items-center justify-between p-2.5 rounded-lg bg-[#181818] border border-[#242424] text-xs"
                    >
                      <div className="flex items-center space-x-2.5 truncate mr-3">
                        <span className="text-[#888888] font-mono text-[11px]">{idx + 1}.</span>
                        <span className="text-white truncate font-medium" title={folder}>
                          {folder}
                        </span>
                      </div>
                      <button
                        onClick={() => handleRemoveFolder(folder)}
                        className="text-[#888888] hover:text-[#FF4D4D] p-1 rounded-md transition-colors cursor-pointer shrink-0"
                      >
                        <Trash2 size={15} />
                      </button>
                    </div>
                  ))
                )}
              </div>
            </div>
          )}

          {/* ETAPA 4: AUTENTICAÇÃO PLEX */}
          {((step === 3 && !useLocal && usePlex) || (step === 4 && useLocal && usePlex)) && (
            <div className="space-y-4 animate-in fade-in duration-150">
              <div>
                <h2 className="text-xl font-bold text-white flex items-center space-x-2">
                  <Server size={20} className="text-[#E5A00D]" />
                  <span>{t("wizard.step4PlexTitle")}</span>
                </h2>
                <p className="text-xs text-[#999999] mt-0.5">{t("wizard.step4PlexDesc")}</p>
              </div>

              {!config.plex_token ? (
                <div className="bg-[#121212] border border-[#262626] rounded-xl p-6 flex flex-col items-center justify-center text-center space-y-4">
                  {isPollingPin ? (
                    <div className="space-y-3 flex flex-col items-center">
                      <div className="flex items-center space-x-2 text-[#E5A00D]">
                        <RefreshCw size={22} className="animate-spin" />
                        <span className="text-sm font-bold">{t("settings.waitingBrowser")}</span>
                      </div>
                      {pinCode && (
                        <div className="bg-[#1C1810] border border-[#E5A00D]/40 px-5 py-2.5 rounded-xl">
                          <span className="text-2xl font-mono font-black text-[#E5A00D] tracking-widest">
                            {pinCode}
                          </span>
                        </div>
                      )}
                    </div>
                  ) : (
                    <>
                      <div className="w-12 h-12 rounded-xl bg-[#E5A00D]/10 border border-[#E5A00D]/20 flex items-center justify-center text-[#E5A00D]">
                        <ExternalLink size={24} />
                      </div>
                      <button
                        type="button"
                        onClick={handleStartPlexAuth}
                        className="flex items-center space-x-2 px-6 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-lg transition-transform active:scale-95 cursor-pointer"
                      >
                        <Server size={16} />
                        <span>{t("settings.connectPlex")}</span>
                      </button>
                    </>
                  )}
                </div>
              ) : (
                <div className="space-y-3">
                  <div className="flex items-center justify-between p-3 rounded-xl bg-[#141F14] border border-[#4BB543]/40 text-xs">
                    <div className="flex items-center space-x-2 text-[#4BB543]">
                      <Check size={16} />
                      <span className="font-bold">{t("settings.sessionActive")}</span>
                    </div>
                    <button
                      type="button"
                      onClick={() => {
                        setConfig((prev) => ({
                          ...prev,
                          plex_token: "",
                          plex_url: "",
                          plex_server_id: null,
                          plex_server_name: null,
                        }));
                        setDiscoveredServers([]);
                      }}
                      className="text-[#888888] hover:text-[#FF4D4D] text-[11px] underline cursor-pointer"
                    >
                      {t("settings.disconnect")}
                    </button>
                  </div>

                  <div className="space-y-2">
                    <label className="block text-xs font-bold text-[#CCCCCC]">
                      {t("settings.activeServer")}
                    </label>

                    {loadingServers ? (
                      <div className="p-4 text-center text-xs text-[#777777] flex items-center justify-center space-x-2">
                        <RefreshCw size={14} className="animate-spin text-[#E5A00D]" />
                        <span>{t("settings.searchingServers")}</span>
                      </div>
                    ) : (
                      <div className="space-y-2 max-h-40 overflow-y-auto">
                        {discoveredServers.map((srv) => {
                          const isSelected = selectedServerUri === srv.chosen_uri;
                          const localConn = srv.connections.find((c) => c.local);
                          return (
                            <div
                              key={srv.client_identifier}
                              onClick={() => handleSelectServer(srv)}
                              className={`p-3 rounded-xl border transition-all cursor-pointer flex items-center justify-between ${
                                isSelected
                                  ? "border-[#E5A00D] bg-[#221B0E]"
                                  : "border-[#262626] bg-[#121212] hover:bg-[#181818]"
                              }`}
                            >
                              <div className="space-y-0.5">
                                <span className="text-xs font-bold text-white block">{srv.name}</span>
                                <span className="text-[11px] text-[#777777] font-mono">
                                  {localConn ? `${localConn.address}:${localConn.port} (LAN)` : srv.chosen_uri}
                                </span>
                              </div>
                              <input
                                type="radio"
                                checked={isSelected}
                                onChange={() => handleSelectServer(srv)}
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
            </div>
          )}

          {/* ETAPA FINAL: REVISÃO & CONCLUSÃO */}
          {((step === 3 && !useLocal && !usePlex) ||
            (step === 4 && (!useLocal || !usePlex)) ||
            step === 5) && (
            <div className="space-y-5 animate-in fade-in duration-150">
              <div className="space-y-1.5">
                <h2 className="text-xl font-bold text-white flex items-center space-x-2">
                  <Check size={22} className="text-[#E5A00D]" />
                  <span>{t("wizard.stepFinalTitle")}</span>
                </h2>
                <p className="text-xs text-[#999999] leading-relaxed">
                  {t("wizard.stepFinalDesc")}
                </p>
              </div>

              <div className="bg-[#121212] border border-[#242424] rounded-xl p-4 space-y-2 text-xs">
                <div className="flex justify-between py-1 border-b border-[#1F1F1F]">
                  <span className="text-[#888888]">{t("settings.audioOutput")}:</span>
                  <span className="font-mono text-white truncate max-w-xs">
                    {isDirect ? config.alsa_device : t("settings.pipewireMode")}
                  </span>
                </div>
                <div className="flex justify-between py-1 border-b border-[#1F1F1F]">
                  <span className="text-[#888888]">{t("sidebar.local")}:</span>
                  <span className="text-white">{t("wizard.foldersCount", { count: config.local_folders.length })}</span>
                </div>
                <div className="flex justify-between py-1">
                  <span className="text-[#888888]">{t("sidebar.plex")}:</span>
                  <span className="text-white">
                    {config.plex_url ? t("wizard.connected") : t("wizard.disabled")}
                  </span>
                </div>
              </div>
            </div>
          )}

          {/* Rodapé */}
          <div className="pt-6 border-t border-[#242424] flex items-center justify-between">
            {step > 1 ? (
              <button
                type="button"
                onClick={prevStep}
                className="flex items-center space-x-2 px-4 py-2 rounded-xl bg-[#222222] hover:bg-[#2A2A2A] text-xs font-bold text-[#CCCCCC] hover:text-white transition-colors cursor-pointer"
              >
                <ArrowLeft size={15} />
                <span>{t("wizard.back")}</span>
              </button>
            ) : (
              <div />
            )}

            {step < totalSteps - 1 ? (
              <button
                type="button"
                onClick={nextStep}
                className="flex items-center space-x-2 px-6 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-md transition-transform active:scale-95 cursor-pointer"
              >
                <span>{t("wizard.next")}</span>
                <ArrowRight size={15} />
              </button>
            ) : (
              <button
                type="button"
                onClick={handleComplete}
                disabled={isFinishing}
                className="flex items-center space-x-2 px-6 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-md transition-transform active:scale-95 cursor-pointer disabled:opacity-50"
              >
                <Check size={16} />
                <span>{isFinishing ? t("wizard.starting") : t("wizard.start")}</span>
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};

export default WelcomeWizard;
