import { logError } from "../utils/frontendDiagnostics";
import React, { useState, useEffect, useRef, useId } from "react";
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
  CircleHelp,
  CircleAlert,
  CheckCircle2,
  LoaderCircle,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { AppConfig, type ConfigSaveOperation } from "../types/config";
import { AudioDevice } from "../types/audio";
import { PlexServerResource } from "../types/plex";
import { audioService } from "../services/audio";
import { configService } from "../services/config";
import { plexService } from "../services/plex";
import { AudioSignalDiagram } from "./AudioSignalDiagram";
import { AppearanceSetting } from "./AppearanceSetting";
import { createDeviceAvailabilityQuery, shouldCheckContextualDevice, type AvailabilityQueryState } from "../utils/deviceAvailability";
import { wizardPlexConnectionReady, wizardSaveErrorKey, type WizardSaveErrorKey } from "../utils/welcomeWizard";

const OPENING_PAGES = ["appearance", "audio", "sources"];

interface Props {
  initialConfig: AppConfig;
  devices: AudioDevice[];
  onFinish: (newConfig: AppConfig) => void;
  onCancel?: () => void;
  onSaveAccepted?: (operation: ConfigSaveOperation) => void;
}

export const WelcomeWizard: React.FC<Props> = ({ initialConfig, devices, onFinish, onCancel, onSaveAccepted }) => {
  const { t, i18n } = useTranslation();
  const titleId = useId();
  const headingRef = useRef<HTMLHeadingElement>(null);
  const [step, setStep] = useState<number>(() => OPENING_PAGES.indexOf(onCancel ? "sources" : "appearance") + 1);
  const [config, setConfig] = useState<AppConfig>({ ...initialConfig, local_folders: [...initialConfig.local_folders] });

  const [useLocal, setUseLocal] = useState<boolean>(onCancel ? initialConfig.local_folders.length > 0 || !initialConfig.plex_token.trim() : true);
  const [usePlex, setUsePlex] = useState<boolean>(onCancel ? Boolean(initialConfig.plex_token.trim()) : false);
  const [isFinishing, setIsFinishing] = useState<boolean>(false);
  const [isPickingFolder, setIsPickingFolder] = useState(false);
  const pickingFolderRef = useRef(false);
  const finishingRef = useRef(false);
  const [completionError, setCompletionError] = useState<WizardSaveErrorKey | "wizard.plexNotReady" | "wizard.folderError" | null>(null);
  const [availability, setAvailability] = useState<AvailabilityQueryState>({ checking: false, results: [], failed: false });
  const availabilityQueryRef = useRef<ReturnType<typeof createDeviceAvailabilityQuery> | null>(null);
  const [showAvailabilityHelp, setShowAvailabilityHelp] = useState(false);

  const [selectedLang, setSelectedLang] = useState(i18n.language || "pt-BR");

  const [pinCode, setPinCode] = useState<string | null>(null);
  const [pinId, setPinId] = useState<number | null>(null);
  const [isPollingPin, setIsPollingPin] = useState(false);
  const [discoveredServers, setDiscoveredServers] = useState<PlexServerResource[]>([]);
  const [loadingServers, setLoadingServers] = useState(false);
  const [plexError, setPlexError] = useState<string | null>(null);
  const authGenerationRef = useRef(0);
  const authActiveRef = useRef(false);
  const mountedRef = useRef(true);
  const pollTimerRef = useRef<number | null>(null);

  useEffect(() => {
    mountedRef.current = true;
    const query = createDeviceAvailabilityQuery(configService.checkDirectDeviceAvailability, setAvailability);
    availabilityQueryRef.current = query;
    return () => {
      mountedRef.current = false;
      authGenerationRef.current++;
      query.dispose();
      availabilityQueryRef.current = null;
    };
  }, []);

  useEffect(() => { headingRef.current?.focus(); }, [step]);

  const handleLanguageChange = (lang: string) => {
    setSelectedLang(lang);
    i18n.changeLanguage(lang);
    localStorage.setItem("sonante_lang", lang);
  };

  const handleAddFolder = async () => {
    if (pickingFolderRef.current) return;
    pickingFolderRef.current = true;
    setIsPickingFolder(true);
    try {
      const selected = await audioService.pickDirectory();
      if (selected) {
        setConfig((prev) => prev.local_folders.includes(selected) ? prev : {
          ...prev,
          local_folders: [...prev.local_folders, selected],
        });
      }
    } catch (err) {
      logError("wizard.pick-folder", err);
      setCompletionError("wizard.folderError");
    } finally {
      pickingFolderRef.current = false;
      setIsPickingFolder(false);
    }
  };

  const handleRemoveFolder = (folderToRemove: string) => {
    setConfig((prev) => ({
      ...prev,
      local_folders: prev.local_folders.filter((f) => f !== folderToRemove),
    }));
  };

  const cancelPlexAuth = () => {
    authGenerationRef.current++;
    authActiveRef.current = false;
    if (pollTimerRef.current) clearInterval(pollTimerRef.current);
    setIsPollingPin(false);
    setPinId(null);
    setPinCode(null);
    setLoadingServers(false);
    setPlexError(null);
  };

  const disconnectPlex = () => {
    cancelPlexAuth();
    setConfig(prev => ({ ...prev, plex_token: "", plex_url: "", plex_server_id: null, plex_server_name: null }));
    setDiscoveredServers([]);
  };

  const handleStartPlexAuth = async () => {
    if (authActiveRef.current || finishingRef.current) return;
    const generation = ++authGenerationRef.current;
    authActiveRef.current = true;
    setIsPollingPin(true);
    setPinCode(null);
    setPinId(null);
    setPlexError(null);
    try {
      const pin = await plexService.createPin();
      if (!mountedRef.current || generation !== authGenerationRef.current) return;
      setPinCode(pin.code);
      setPinId(pin.id);
      await plexService.openExternalUrl(pin.auth_url);
    } catch {
      if (!mountedRef.current || generation !== authGenerationRef.current) return;
      cancelPlexAuth();
      setPlexError("settings.plexConnectionError");
    }
  };

  useEffect(() => {
    if (!pinId || !isPollingPin) return;
    const generation = authGenerationRef.current;
    let reading = false;
    const current = () => mountedRef.current && generation === authGenerationRef.current;
    const timer = window.setInterval(async () => {
      if (reading || !current()) return;
      reading = true;
      let discovering = false;
      try {
        const token = await plexService.checkPin(pinId);
        if (!current()) return;
        if (token) {
          clearInterval(timer);
          setIsPollingPin(false);
          setPinId(null);
          setPinCode(null);
          setPlexError(null);
          setConfig(prev => ({ ...prev, plex_token: token, plex_url: "", plex_server_id: null, plex_server_name: null }));
          setLoadingServers(true);
          discovering = true;
          const servers = await plexService.getServers(token);
          if (!current()) return;
          setDiscoveredServers(servers);
          setLoadingServers(false);
          authActiveRef.current = false;
          if (servers.length > 0) {
            const server = servers[0];
            setConfig(prev => ({ ...prev, plex_url: server.chosen_uri,
              plex_server_id: server.client_identifier, plex_server_name: server.name }));
          } else setPlexError("wizard.noPlexServers");
        }
      } catch {
        if (!current()) return;
        // Raw authentication/discovery errors can contain tokens or routes.
        setPlexError(discovering ? "wizard.plexDiscoveryError" : "settings.plexConnectionError");
        if (discovering) {
          setLoadingServers(false);
          authActiveRef.current = false;
        }
      } finally { reading = false; }
    }, 1500);
    pollTimerRef.current = timer;
    return () => { clearInterval(timer); };
  }, [pinId, isPollingPin]);

  const handleSelectServer = (server: PlexServerResource) => {
    setPlexError(null);
    setConfig((prev) => ({
      ...prev,
      plex_url: server.chosen_uri,
      plex_server_id: server.client_identifier,
      plex_server_name: server.name,
    }));
  };

  const completionErrorMessage = (error: NonNullable<typeof completionError>): string => {
    switch (error) {
      case "settings.localFolderOverlap": return t("settings.localFolderOverlap");
      case "wizard.configPersistenceUncertain": return t("wizard.configPersistenceUncertain");
      case "wizard.deviceBusy": return t("wizard.deviceBusy");
      case "wizard.deviceUnavailable": return t("wizard.deviceUnavailable");
      case "wizard.saveError": return t("wizard.saveError");
      case "wizard.plexNotReady": return t("wizard.plexNotReady");
      case "wizard.folderError": return t("wizard.folderError");
      default: {
        const unsupported: never = error;
        throw new Error(`Unsupported wizard completion error type: ${typeof unsupported}`);
      }
    }
  };

  const handleComplete = async () => {
    if (finishingRef.current || availability.checking) return;
    if (plexRequired && !plexReady) {
      setCompletionError("wizard.plexNotReady");
      return;
    }
    finishingRef.current = true;
    availabilityQueryRef.current?.invalidate();
    setCompletionError(null);
    setIsFinishing(true);
    try {
      const finalConfig: AppConfig = {
        ...config,
        first_run: false,
      };
      const response = await configService.saveConfig(finalConfig);
      if (response.status === "accepted" && onSaveAccepted) { onSaveAccepted(response.operation); return; }
      if (response.status !== "completed") throw new Error(t("settings.librarySaveUncertain"));
      onFinish(finalConfig);
    } catch (err) {
      setCompletionError(wizardSaveErrorKey(err));
    } finally {
      finishingRef.current = false;
      setIsFinishing(false);
    }
  };

  const pages = [...OPENING_PAGES, ...(useLocal ? ["local"] : []), ...(usePlex ? ["plex"] : []), "review"];
  const totalSteps = pages.length;
  const currentPage = pages[step - 1];
  const plexReady = wizardPlexConnectionReady(config) && !loadingServers && !plexError;
  const plexRequired = usePlex || Boolean(config.plex_token.trim());

  const nextStep = () => {
    if (currentPage === "sources" && !useLocal && !usePlex) {
      alert(t("wizard.sourceRequired"));
      return;
    }
    if (currentPage === "plex" && !plexReady) {
      setPlexError("wizard.plexNotReady");
      return;
    }
    setStep(s => Math.min(totalSteps, s + 1));
  };

  const prevStep = () => {
    setStep((s) => Math.max(1, s - 1));
  };

  const isDirect = (config.audio_output_type || "alsa") === "alsa";
  const selectedAvailability = availability.results.find(row => row.id === config.alsa_device)?.state ?? "indeterminate";
  const displayedAvailability = selectedAvailability === "current_sonante" ? "indeterminate" : selectedAvailability;
  const AvailabilityIcon = availability.checking ? LoaderCircle
    : displayedAvailability === "available" ? CheckCircle2 : displayedAvailability === "busy" ? CircleAlert : CircleHelp;
  const canCheckDevice = shouldCheckContextualDevice(initialConfig, config, devices.map(device => device.id));
  const isSelectedInList = devices.some(device => device.id === config.alsa_device);
  const selectedDeviceName = devices.find(device => device.id === config.alsa_device)?.name ?? config.alsa_device;
  const serverName = config.plex_server_name?.trim() || t("wizard.unnamedServer");
  const availabilityHelpId = `${titleId}-availability-help`;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-[var(--theme-surface-base,#0C0C0C)] select-none p-3 sm:p-6 text-[color:var(--theme-fg-primary,#ffffff)] font-sans">
      <div role="dialog" onKeyDown={event => { if (event.key === "Escape" && !finishingRef.current && onCancel) { event.preventDefault(); onCancel(); } }} aria-modal="true" aria-labelledby={titleId} className="bg-[var(--theme-surface-base,#161616)] border border-[color:var(--theme-border-soft,#2B2B2B)] rounded-2xl w-full max-w-4xl h-[min(820px,94dvh)] min-h-0 flex flex-col shadow-2xl overflow-hidden">
        {/* Topo do Wizard */}
        <div className="px-4 sm:px-8 py-4 border-b border-[color:var(--theme-border-soft,#242424)] flex flex-wrap gap-3 items-center justify-between bg-[var(--theme-surface-panel,#191919)] shrink-0">
          <div className="flex items-center space-x-3">
            <div className="w-9 h-9 rounded-xl bg-[#E5A00D]/10 border border-[color:var(--theme-border-accent,#E5A00D)]/30 flex items-center justify-center text-[color:var(--theme-fg-accent,#E5A00D)]">
              <Sparkles size={20} />
            </div>
            <div>
              <h1 id={titleId} className="text-base font-black tracking-wider text-[color:var(--theme-fg-accent,#E5A00D)]">SONANTE</h1>
              <p className="text-[11px] text-[color:var(--theme-fg-muted,#888888)]">{t("wizard.subtitle")}</p>
            </div>
          </div>

          <div className="flex items-center space-x-3">
            {onCancel && <button type="button" disabled={isFinishing} onClick={onCancel}
              className="rounded-lg px-3 py-2 text-xs text-[color:var(--theme-fg-secondary,#AAAAAA)] hover:text-[color:var(--theme-fg-primary,#ffffff)] focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] disabled:opacity-40">{t("settings.cancel")}</button>}
            {/* Alternador de Idioma */}
            <div className="flex items-center space-x-1 bg-[var(--theme-surface-base,#121212)] border border-[color:var(--theme-border-soft,#282828)] rounded-lg p-0.5 text-[11px]">
              <Languages size={12} className="text-[color:var(--theme-fg-muted,#777777)] ml-1.5 mr-0.5" />
              <button
                type="button"
                disabled={isFinishing}
                onClick={() => handleLanguageChange("pt-BR")}
                className={`px-2 py-0.5 rounded font-semibold transition-colors cursor-pointer ${
                  selectedLang.startsWith("pt") ? "bg-[#E5A00D] text-black" : "text-[color:var(--theme-fg-muted,#777777)] hover:text-[color:var(--theme-fg-primary,#ffffff)]"
                }`}
              >
                PT
              </button>
              <button
                type="button"
                disabled={isFinishing}
                onClick={() => handleLanguageChange("en")}
                className={`px-2 py-0.5 rounded font-semibold transition-colors cursor-pointer ${
                  selectedLang.startsWith("en") ? "bg-[#E5A00D] text-black" : "text-[color:var(--theme-fg-muted,#777777)] hover:text-[color:var(--theme-fg-primary,#ffffff)]"
                }`}
              >
                EN
              </button>
            </div>

            <span className="text-xs font-mono text-[color:var(--theme-fg-muted,#888888)]">
              {t("wizard.step", { step, total: totalSteps })}
            </span>
          </div>
        </div>

        {/* Conteúdo */}
        <div className="p-4 sm:p-6 flex-1 min-h-0 overflow-y-auto">
          {currentPage === "appearance" && (
            <div className="space-y-5 animate-in fade-in duration-150">
              <h2 ref={headingRef} tabIndex={-1} className="text-xl font-bold text-[color:var(--theme-fg-primary,#ffffff)]">
                {t("wizard.appearanceTitle")}
              </h2>
              <AppearanceSetting />
            </div>
          )}

          {/* MODO DE SAÍDA */}
          {currentPage === "audio" && (
            <div className="space-y-5 animate-in fade-in duration-150">
              <div className="space-y-1.5">
                <h2 ref={headingRef} tabIndex={-1} className="text-xl font-bold text-[color:var(--theme-fg-primary,#ffffff)] flex items-center space-x-2">
                  <Sliders size={20} className="text-[color:var(--theme-fg-accent,#E5A00D)]" />
                  <span>{t("wizard.step1Title")}</span>
                </h2>
                <p className="text-xs text-[color:var(--theme-fg-secondary,#999999)] leading-relaxed">
                  {t("wizard.step1Desc")}
                </p>
              </div>

              {/* Botões dos 2 Modos */}
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 pt-1">
                <button
                  type="button"
                  aria-pressed={isDirect}
                  disabled={isFinishing}
                  onClick={() => {
                    if (isDirect || finishingRef.current) return;
                    const proposed: AppConfig = { ...config, audio_output_type: "alsa" };
                    availabilityQueryRef.current?.invalidate();
                    setConfig(proposed);
                    if (shouldCheckContextualDevice(initialConfig, proposed, devices.map(device => device.id))) {
                      void availabilityQueryRef.current?.check(proposed.alsa_device);
                    }
                  }}
                  className={`p-3.5 rounded-xl border-2 text-left transition-all cursor-pointer flex flex-col justify-between ${
                    isDirect
                      ? "border-[color:var(--theme-border-accent,#E5A00D)] bg-[var(--theme-surface-accent,#221B0E)]"
                      : "border-[color:var(--theme-border-soft,#262626)] bg-[var(--theme-surface-base,#141414)] hover:bg-[var(--theme-surface-panel,#181818)]"
                  }`}
                >
                  <div className="flex items-center space-x-2">
                    <ShieldCheck size={18} className={isDirect ? "text-[color:var(--theme-fg-accent,#E5A00D)]" : "text-[color:var(--theme-fg-muted,#777777)]"} />
                    <span className="font-bold text-xs text-[color:var(--theme-fg-primary,#ffffff)]">{t("settings.alsaMode")}</span>
                  </div>
                  <span className="text-[10px] text-[color:var(--theme-fg-muted,#777777)] mt-1">
                    {t("settings.directModeCaption")}
                  </span>
                </button>

                <button
                  type="button"
                  aria-pressed={!isDirect}
                  disabled={isFinishing}
                  onClick={() => {
                    if (finishingRef.current) return;
                    availabilityQueryRef.current?.invalidate();
                    setConfig({ ...config, audio_output_type: "pipewire" });
                  }}
                  className={`p-3.5 rounded-xl border-2 text-left transition-all cursor-pointer flex flex-col justify-between ${
                    !isDirect
                      ? "border-[color:var(--theme-border-accent,#E5A00D)] bg-[var(--theme-surface-accent,#221B0E)]"
                      : "border-[color:var(--theme-border-soft,#262626)] bg-[var(--theme-surface-base,#141414)] hover:bg-[var(--theme-surface-panel,#181818)]"
                  }`}
                >
                  <div className="flex items-center space-x-2">
                    <Radio size={18} className={!isDirect ? "text-[color:var(--theme-fg-accent,#E5A00D)]" : "text-[color:var(--theme-fg-muted,#777777)]"} />
                    <span className="font-bold text-xs text-[color:var(--theme-fg-primary,#ffffff)]">{t("settings.pipewireMode")}</span>
                  </div>
                  <span className="text-[10px] text-[color:var(--theme-fg-muted,#777777)] mt-1">
                    {t("settings.sharedModeCaption")}
                  </span>
                </button>
              </div>

              {/* Configuração Contextual */}
              {isDirect ? (
                <div className="space-y-3 pt-1">
                  <div className="space-y-1.5">
                    <label htmlFor={`${titleId}-device`} className="block text-xs font-bold text-[color:var(--theme-fg-secondary,#CCCCCC)] uppercase tracking-wider">
                      {t("settings.alsaDevice")}
                    </label>
                    <select
                      id={`${titleId}-device`}
                      value={config.alsa_device}
                      disabled={isFinishing}
                      onFocus={() => {
                        if (!finishingRef.current && !availability.checking && selectedAvailability === "busy" && canCheckDevice) {
                          void availabilityQueryRef.current?.check(config.alsa_device);
                        }
                      }}
                      onChange={(e) => {
                        const id = e.target.value;
                        if (finishingRef.current || id === config.alsa_device) return;
                        const proposed = { ...config, alsa_device: id };
                        availabilityQueryRef.current?.invalidate();
                        setConfig(proposed);
                        if (shouldCheckContextualDevice(initialConfig, proposed, devices.map(device => device.id))) {
                          void availabilityQueryRef.current?.check(id);
                        }
                      }}
                      className="w-full bg-[var(--theme-surface-base,#111111)] border border-[color:var(--theme-border-soft,#333333)] rounded-xl px-4 py-3 text-xs text-[color:var(--theme-fg-primary,#ffffff)] outline-none focus:border-[color:var(--theme-border-accent,#E5A00D)] cursor-pointer"
                    >
                      {!isSelectedInList && (
                        <option value={config.alsa_device}>{t("settings.currentDevice", { id: config.alsa_device || t("settings.deviceAvailability.indeterminate") })}</option>
                      )}
                      {devices.map((dev) => (
                        <option key={dev.id} value={dev.id} className="bg-[var(--theme-surface-panel,#1A1A1A)] py-1">
                          {dev.name}
                        </option>
                      ))}
                    </select>
                  </div>

                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="flex items-center gap-2">
                      <div role="status" aria-live="polite" aria-atomic="true" className="flex items-center gap-1.5 text-xs text-[color:var(--theme-fg-secondary,#CCCCCC)]">
                        <AvailabilityIcon size={14} aria-hidden="true" className={availability.checking ? "animate-spin" : undefined} />
                        <span>{t(availability.checking ? "settings.deviceAvailability.checking" : `settings.deviceAvailability.${displayedAvailability}`)}</span>
                      </div>
                      <button type="button" aria-label={t("settings.deviceAvailability.helpLabel")} title={t("settings.deviceAvailability.helpLabel")}
                        aria-expanded={showAvailabilityHelp} aria-controls={availabilityHelpId}
                        onClick={() => setShowAvailabilityHelp(open => !open)}
                        className="p-1 rounded text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]">
                        <CircleHelp size={15} aria-hidden="true" />
                      </button>
                    </div>
                    <button type="button" disabled={availability.checking || isFinishing || !canCheckDevice}
                      onClick={() => { void availabilityQueryRef.current?.check(config.alsa_device); }}
                      className="text-[11px] text-[color:var(--theme-fg-accent,#E5A00D)] hover:text-[color:var(--theme-fg-primary,#ffffff)] disabled:opacity-50">
                      {t("settings.deviceAvailability.check")}
                    </button>
                  </div>
                  <div id={availabilityHelpId} hidden={!showAvailabilityHelp} className="text-[11px] text-[color:var(--theme-fg-secondary,#AAAAAA)] leading-relaxed space-y-1 p-3 rounded-lg bg-[var(--theme-surface-base,#121212)]">
                    <p>{t("settings.deviceAvailability.helpObservation")}</p>
                    <p>{t("settings.deviceAvailability.helpShared")}</p>
                    <p>{t("settings.deviceAvailability.helpBusy")}</p>
                    <p>{t("settings.deviceAvailability.helpLimits")}</p>
                  </div>
                  {selectedAvailability === "busy" && !availability.checking && <p role="alert" className="text-xs text-[color:var(--theme-fg-accent,#E5A00D)]">{t("settings.deviceAvailability.busyAction")}</p>}
                  {config.alsa_device === "default" && <p className="text-xs text-[color:var(--theme-fg-accent,#C9A45D)]">{t("settings.defaultEndpointNotice")}</p>}
                </div>
              ) : (
                <p className="text-xs text-[color:var(--theme-fg-secondary,#AAAAAA)]">{t("settings.sharedActiveNotice")}</p>
              )}
              <AudioSignalDiagram mode={!isDirect || config.alsa_device === "default" ? "shared" : "direct"} device={config.alsa_device} />
            </div>
          )}

          {/* ETAPA 2: FONTES DE MÍDIA */}
          {currentPage === "sources" && (
            <div className="space-y-5 animate-in fade-in duration-150">
              <div className="space-y-1.5">
                <h2 ref={headingRef} tabIndex={-1} className="text-xl font-bold text-[color:var(--theme-fg-primary,#ffffff)]">{t("wizard.step2Title")}</h2>
                <p className="text-xs text-[color:var(--theme-fg-secondary,#999999)]">{t("wizard.step2Desc")}</p>
              </div>

              <div className="grid grid-cols-1 sm:grid-cols-2 gap-4 pt-2">
                <button
                  type="button" aria-pressed={useLocal} disabled={isFinishing}
                  onClick={() => setUseLocal(!useLocal)}
                  className={`p-5 rounded-xl border-2 transition-all cursor-pointer flex flex-col justify-between text-left focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] ${
                    useLocal
                      ? "border-[color:var(--theme-border-accent,#E5A00D)] bg-[var(--theme-surface-accent,#221B0E)]"
                      : "border-[color:var(--theme-border-soft,#262626)] bg-[var(--theme-surface-base,#141414)] hover:bg-[var(--theme-surface-panel,#1A1A1A)]"
                  }`}
                >
                  <div className="space-y-2">
                    <div className="w-10 h-10 rounded-lg bg-[var(--theme-surface-raised,#252525)] flex items-center justify-center text-[color:var(--theme-fg-accent,#E5A00D)]">
                      <HardDrive size={20} />
                    </div>
                    <h3 className="font-bold text-sm text-[color:var(--theme-fg-primary,#ffffff)]">{t("sidebar.local")}</h3>
                    <p className="text-[11px] text-[color:var(--theme-fg-muted,#888888)] leading-relaxed">
                      {t("wizard.step2LocalDesc")}
                    </p>
                  </div>
                  <div className="pt-4 flex items-center justify-between text-xs">
                    <span className={useLocal ? "text-[color:var(--theme-fg-accent,#E5A00D)] font-bold" : "text-[color:var(--theme-fg-muted,#666666)]"}>
                      {useLocal ? t("wizard.enabled") : t("wizard.disabled")}
                    </span>
                    <Check size={16} aria-hidden="true" className={useLocal ? "text-[color:var(--theme-fg-accent,#E5A00D)]" : "opacity-20"} />
                  </div>
                </button>

                <button
                  type="button" aria-pressed={usePlex} disabled={isFinishing}
                  onClick={() => { if (usePlex) cancelPlexAuth(); setUsePlex(!usePlex); }}
                  className={`p-5 rounded-xl border-2 transition-all cursor-pointer flex flex-col justify-between text-left focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] ${
                    usePlex
                      ? "border-[color:var(--theme-border-accent,#E5A00D)] bg-[var(--theme-surface-accent,#221B0E)]"
                      : "border-[color:var(--theme-border-soft,#262626)] bg-[var(--theme-surface-base,#141414)] hover:bg-[var(--theme-surface-panel,#1A1A1A)]"
                  }`}
                >
                  <div className="space-y-2">
                    <div className="w-10 h-10 rounded-lg bg-[var(--theme-surface-raised,#252525)] flex items-center justify-center text-[color:var(--theme-fg-accent,#E5A00D)]">
                      <Server size={20} />
                    </div>
                    <h3 className="font-bold text-sm text-[color:var(--theme-fg-primary,#ffffff)]">{t("sidebar.plex")}</h3>
                    <p className="text-[11px] text-[color:var(--theme-fg-muted,#888888)] leading-relaxed">
                      {t("wizard.step2PlexDesc")}
                    </p>
                  </div>
                  <div className="pt-4 flex items-center justify-between text-xs">
                    <span className={usePlex ? "text-[color:var(--theme-fg-accent,#E5A00D)] font-bold" : "text-[color:var(--theme-fg-muted,#666666)]"}>
                      {usePlex ? t("wizard.enabled") : t("wizard.disabled")}
                    </span>
                    <Check size={16} aria-hidden="true" className={usePlex ? "text-[color:var(--theme-fg-accent,#E5A00D)]" : "opacity-20"} />
                  </div>
                </button>
              </div>
            </div>
          )}

          {/* ETAPA 3: PASTAS LOCAIS */}
          {currentPage === "local" && (
            <div className="space-y-4 animate-in fade-in duration-150">
              <div className="flex items-center justify-between">
                <div>
                  <h2 ref={headingRef} tabIndex={-1} className="text-xl font-bold text-[color:var(--theme-fg-primary,#ffffff)]">{t("wizard.step3LocalTitle")}</h2>
                  <p className="text-xs text-[color:var(--theme-fg-secondary,#999999)] mt-0.5">{t("wizard.step3LocalDesc")}</p>
                </div>

                <button
                  type="button"
                  onClick={handleAddFolder}
                  disabled={isPickingFolder}
                  className="flex items-center space-x-2 px-4 py-2 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-md transition-transform active:scale-95 cursor-pointer disabled:cursor-not-allowed disabled:opacity-50"
                >
                  <FolderPlus size={16} />
                  <span>{t("settings.addFolder")}</span>
                </button>
              </div>

              <div className="bg-[var(--theme-surface-base,#111111)] border border-[color:var(--theme-border-soft,#262626)] rounded-xl p-3 min-h-[160px] max-h-[220px] overflow-y-auto space-y-2">
                {config.local_folders.length === 0 ? (
                  <div className="h-32 flex flex-col items-center justify-center text-[color:var(--theme-fg-muted,#666666)] space-y-2">
                    <Disc3 size={32} className="opacity-40" />
                    <span className="text-xs">{t("settings.noFolders")}</span>
                  </div>
                ) : (
                  config.local_folders.map((folder, idx) => (
                    <div
                      key={folder}
                      className="flex items-center justify-between p-2.5 rounded-lg bg-[var(--theme-surface-panel,#181818)] border border-[color:var(--theme-border-soft,#242424)] text-xs"
                    >
                      <div className="flex items-center space-x-2.5 truncate mr-3">
                        <span className="text-[color:var(--theme-fg-muted,#888888)] font-mono text-[11px]">{idx + 1}.</span>
                        <span className="text-[color:var(--theme-fg-primary,#ffffff)] truncate font-medium" title={folder}>
                          {folder}
                        </span>
                      </div>
                      <button
                        onClick={() => handleRemoveFolder(folder)}
                        className="text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-error,#FF4D4D)] p-1 rounded-md transition-colors cursor-pointer shrink-0"
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
          {currentPage === "plex" && (
            <div className="space-y-4 animate-in fade-in duration-150">
              <div>
                <h2 ref={headingRef} tabIndex={-1} className="text-xl font-bold text-[color:var(--theme-fg-primary,#ffffff)] flex items-center space-x-2">
                  <Server size={20} className="text-[color:var(--theme-fg-accent,#E5A00D)]" />
                  <span>{t("wizard.step4PlexTitle")}</span>
                </h2>
                <p className="text-xs text-[color:var(--theme-fg-secondary,#999999)] mt-0.5">{t("wizard.step4PlexDesc")}</p>
              </div>

              {!config.plex_token ? (
                <div className="bg-[var(--theme-surface-base,#121212)] border border-[color:var(--theme-border-soft,#262626)] rounded-xl p-6 flex flex-col items-center text-center gap-4">
                  {isPollingPin ? (
                    <>
                      <div role="status" className="flex items-center gap-2 text-[color:var(--theme-fg-accent,#E5A00D)]">
                        <RefreshCw size={20} aria-hidden="true" className="animate-spin" />
                        <span className="text-sm">{t("settings.waitingBrowser")}</span>
                      </div>
                      {pinCode && <span className="text-2xl font-mono font-black text-[color:var(--theme-fg-accent,#E5A00D)] tracking-widest">{pinCode}</span>}
                      <button type="button" onClick={cancelPlexAuth} className="text-xs text-[color:var(--theme-fg-secondary,#AAAAAA)] underline">{t("settings.cancel")}</button>
                    </>
                  ) : (
                    <button type="button" onClick={handleStartPlexAuth}
                      className="flex items-center gap-2 px-6 py-2.5 rounded-xl bg-[#E5A00D] text-black font-bold text-xs">
                      <ExternalLink size={16} aria-hidden="true" />{t("settings.connectPlex")}
                    </button>
                  )}
                </div>
              ) : (
                <div className="space-y-3">
                  <div className="flex items-center justify-between gap-3 p-3 rounded-xl bg-[var(--theme-surface-base,#121212)] border border-[color:var(--theme-border-soft,#262626)] text-xs">
                    <div role="status" className={plexReady ? "text-[color:var(--theme-fg-success,#79C68B)]" : "text-[color:var(--theme-fg-secondary,#AAAAAA)]"}>
                      {loadingServers ? (
                        <span className="flex items-center gap-2"><RefreshCw size={14} aria-hidden="true" className="animate-spin" />{t("settings.searchingServers")}</span>
                      ) : plexReady ? (
                        <span className="flex items-center gap-2"><CheckCircle2 size={16} aria-hidden="true" />{t("wizard.connected")}</span>
                      ) : t("wizard.plexNotReady")}
                    </div>
                    <button type="button" onClick={disconnectPlex} className="text-[color:var(--theme-fg-secondary,#AAAAAA)] hover:text-[color:var(--theme-fg-error,#FF4D4D)] underline">{t("settings.disconnect")}</button>
                  </div>
                  {!loadingServers && (discoveredServers.length > 1 ? (
                    <div className="space-y-1.5">
                      <label htmlFor={`${titleId}-server`} className="block text-xs text-[color:var(--theme-fg-secondary,#CCCCCC)]">{t("settings.activeServer")}</label>
                      <select id={`${titleId}-server`} value={config.plex_server_id || ""}
                        onChange={event => {
                          const server = discoveredServers.find(server => server.client_identifier === event.target.value);
                          if (server) handleSelectServer(server);
                        }} className="w-full bg-[var(--theme-surface-base,#111111)] border border-[color:var(--theme-border-soft,#333333)] rounded-xl px-4 py-3 text-xs">
                        {discoveredServers.map(server => <option key={server.client_identifier} value={server.client_identifier}>{server.name || t("wizard.unnamedServer")}</option>)}
                      </select>
                    </div>
                  ) : plexReady && <p className="text-sm text-[color:var(--theme-fg-primary,#ffffff)]">{serverName}</p>)}
                </div>
              )}
              {plexError && <p role="alert" className="text-xs text-[color:var(--theme-fg-error,#FF8686)]">{plexError === "settings.plexConnectionError" ? t("settings.plexConnectionError") : t(`wizard.${plexError.slice("wizard.".length)}`)}</p>}
            </div>
          )}

          {/* ETAPA FINAL: REVISÃO & CONCLUSÃO */}
          {currentPage === "review" && (
            <div className="space-y-5 animate-in fade-in duration-150">
              <div className="space-y-1.5">
                <h2 ref={headingRef} tabIndex={-1} className="text-xl font-bold text-[color:var(--theme-fg-primary,#ffffff)] flex items-center space-x-2">
                  <Check size={22} className="text-[color:var(--theme-fg-accent,#E5A00D)]" />
                  <span>{t("wizard.stepFinalTitle")}</span>
                </h2>
                <p className="text-xs text-[color:var(--theme-fg-secondary,#999999)] leading-relaxed">
                  {t("wizard.stepFinalDesc")}
                </p>
              </div>

              <div className="bg-[var(--theme-surface-base,#121212)] border border-[color:var(--theme-border-soft,#242424)] rounded-xl p-4 space-y-2 text-xs">
                <div className="flex justify-between py-1 border-b border-[color:var(--theme-border-soft,#1F1F1F)]">
                  <span className="text-[color:var(--theme-fg-muted,#888888)]">{t("settings.audioOutput")}:</span>
                  <span className="font-mono text-[color:var(--theme-fg-primary,#ffffff)] truncate max-w-xs">
                    {isDirect ? t("settings.alsaMode") : t("settings.pipewireMode")}
                  </span>
                </div>
                {isDirect && <div className="flex justify-between gap-3 py-1 border-b border-[color:var(--theme-border-soft,#1F1F1F)]">
                  <span className="text-[color:var(--theme-fg-muted,#888888)]">{t("settings.alsaDevice")}:</span>
                  <span className="text-[color:var(--theme-fg-primary,#ffffff)] truncate">{selectedDeviceName}</span>
                </div>}
                <div className="flex justify-between py-1 border-b border-[color:var(--theme-border-soft,#1F1F1F)]">
                  <span className="text-[color:var(--theme-fg-muted,#888888)]">{t("sidebar.local")}:</span>
                  <span className="text-[color:var(--theme-fg-primary,#ffffff)]">{t("wizard.foldersCount", { count: config.local_folders.length })}</span>
                </div>
                <div className="flex justify-between py-1">
                  <span className="text-[color:var(--theme-fg-muted,#888888)]">{t("sidebar.plex")}:</span>
                  <span className="text-[color:var(--theme-fg-primary,#ffffff)]">
                    {plexReady ? `${t("wizard.connected")} • ${serverName}` : plexRequired ? t("wizard.plexNotReady") : t("wizard.disabled")}
                  </span>
                </div>
              </div>
            </div>
          )}

          {completionError && <p role="alert" className="mt-4 text-xs text-[color:var(--theme-fg-error,#FF8686)]">{completionErrorMessage(completionError)}</p>}
        </div>
        {/* Rodapé */}
        <div className="px-4 sm:px-6 py-4 border-t border-[color:var(--theme-border-soft,#242424)] flex items-center justify-between shrink-0">
          {step > 1 ? (
            <button
              type="button"
              onClick={prevStep}
              disabled={isFinishing}
              className="flex items-center space-x-2 px-4 py-2 rounded-xl bg-[var(--theme-surface-raised,#222222)] hover:bg-[var(--theme-surface-hover,#2A2A2A)] text-xs font-bold text-[color:var(--theme-fg-secondary,#CCCCCC)] hover:text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer"
            >
              <ArrowLeft size={15} />
              <span>{t("wizard.back")}</span>
            </button>
          ) : (
            <div />
          )}

          {step < totalSteps ? (
            <button
              type="button"
              onClick={nextStep}
              disabled={isFinishing || (currentPage === "plex" && !plexReady)}
              className="flex items-center space-x-2 px-6 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-md transition-transform active:scale-95 cursor-pointer"
            >
              <span>{t("wizard.next")}</span>
              <ArrowRight size={15} />
            </button>
          ) : (
            <button
              type="button"
              onClick={handleComplete}
              disabled={isFinishing || availability.checking || (plexRequired && !plexReady)}
              className="flex items-center space-x-2 px-6 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-md transition-transform active:scale-95 cursor-pointer disabled:opacity-50"
            >
              <Check size={16} />
              <span>{isFinishing ? t("wizard.starting") : t("wizard.start")}</span>
            </button>
          )}
        </div>
      </div>
    </div>
  );
};

export default WelcomeWizard;
