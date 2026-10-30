import { logError } from "../utils/frontendDiagnostics";
import React, { useEffect, useState, useRef, useId } from "react";
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
  CircleAlert,
  CircleHelp,
  LoaderCircle,
  Languages,
  Radio,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { SettingsPages, type SettingsPage } from "./SettingsPages";
import { AppearanceSetting } from "./AppearanceSetting";
import { AudioSignalDiagram } from "./AudioSignalDiagram";
import { AppConfig, ConfigSaveOperation, ConfigSaveWarning } from "../types/config";
import { AudioDevice } from "../types/audio";
import { PlexServerResource } from "../types/plex";
import { configService } from "../services/config";
import { audioService } from "../services/audio";
import { plexService } from "../services/plex";
import { audioRestartExpected, localFoldersOnlyChanged } from "../utils/audioConfigChange";
import { refreshCatalogAfterSuccessfulCheck } from "../utils/localAlbumCatalog";
import { createDeviceAvailabilityQuery, newPhysicalOutputChoice, shouldCheckContextualDevice, type AvailabilityQueryState } from "../utils/deviceAvailability";

interface Props {
  initialPage?: SettingsPage;
  initialDraft?: AppConfig | null;
  initialFolderOverlap?: boolean;
  onClose: () => void;
  onSaved: (audioRestartExpected: boolean, warning?: ConfigSaveWarning) => void;
  onSaveStarted: (audioRestartExpected: boolean) => boolean;
  onSaveFinished: () => void;
  onSaveAccepted: (operation: ConfigSaveOperation) => void;
  onStartLocalFoldersSave: (config: AppConfig) => boolean;
  onLibraryRescanned: () => void;
  isLibraryUpdating: boolean;
}

export const formatConfigError = (error: unknown): string => {
  let message: string;
  if (typeof error === "string") {
    message = error;
  } else if (error instanceof Error) {
    message = error.message;
  } else {
    try {
      message = JSON.stringify(error) ?? String(error);
    } catch {
      message = String(error);
    }
  }

  return message
    .replace(/([?&]X-Plex-Token=)[^&\s"']+/gi, "$1[REDACTED]")
    .replace(/https?:\/\/[^\s"']+/gi, "[URI REDACTED]")
    .replace(/(ACK\s+\[[^\]]+\]\s+\{add\}).*/gi, "$1 [ADD DETAILS REDACTED]");
};

export const SettingsModal: React.FC<Props> = ({
  initialPage,
  initialDraft,
  initialFolderOverlap,
  onClose,
  onSaved,
  onSaveStarted,
  onSaveFinished,
  onSaveAccepted,
  onStartLocalFoldersSave,
  onLibraryRescanned,
  isLibraryUpdating,
}) => {
  const { t, i18n } = useTranslation();
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [isPickingFolder, setIsPickingFolder] = useState(false);
  const [scanning, setScanning] = useState<"check" | null>(null);
  const [scanUnchanged, setScanUnchanged] = useState(false);
  const [scanError, setScanError] = useState<string | null>(null);
  const [selectedLang, setSelectedLang] = useState(i18n.language || "pt-BR");

  // Estados do Fluxo de Login Plex (PIN)
  const [pinCode, setPinCode] = useState<string | null>(null);
  const [pinId, setPinId] = useState<number | null>(null);
  const [isPollingPin, setIsPollingPin] = useState(false);
  const [discoveredServers, setDiscoveredServers] = useState<PlexServerResource[]>([]);
  const [loadingServers, setLoadingServers] = useState(false);
  const [showManualPlex, setShowManualPlex] = useState(false);
  const pollTimerRef = useRef<number | null>(null);
  const mountedRef = useRef(true);
  const authGenerationRef = useRef(0);
  const authActiveRef = useRef(false);
  const originalConfigRef = useRef<AppConfig | null>(null);
  const saveInProgressRef = useRef(false);
  const scanInProgressRef = useRef(false);
  const pickingFolderRef = useRef(false);
  const [availability, setAvailability] = useState<AvailabilityQueryState>({ checking: false, results: [], failed: false });
  const [showAvailabilityHelp, setShowAvailabilityHelp] = useState(false);
  const availabilityQueryRef = useRef<ReturnType<typeof createDeviceAvailabilityQuery> | null>(null);

  useEffect(() => {
    mountedRef.current = true;
    const query = createDeviceAvailabilityQuery(configService.checkDirectDeviceAvailability, setAvailability);
    availabilityQueryRef.current = query;
    return () => {
      mountedRef.current = false;
      authGenerationRef.current++;
      authActiveRef.current = false;
      query.dispose();
      availabilityQueryRef.current = null;
    };
  }, []);

  useEffect(() => {
    let active = true;
    Promise.all([configService.getConfig(), configService.getAudioDevices()])
      .then(([cfg, devs]) => {
        if (!active) return;
        originalConfigRef.current = { ...cfg, local_folders: [...cfg.local_folders] };
        setConfig(initialDraft ? { ...initialDraft, local_folders: [...initialDraft.local_folders] } : cfg);
        setDevices(devs);
        if (cfg.plex_token && cfg.plex_token.trim().length > 0) {
          const generation = authGenerationRef.current;
          const current = () => active && mountedRef.current && generation === authGenerationRef.current;
          setLoadingServers(true);
          plexService.getServers(cfg.plex_token)
            .then(servers => { if (current()) setDiscoveredServers(servers); })
            .catch(error => { if (current()) logError("plex.servers-discover", error); })
            .finally(() => { if (current()) setLoadingServers(false); });
        }
      })
      .catch(error => { if (active) logError("settings.load", error); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, []);

  useEffect(() => {
    if (loading) return;
    const previous = document.activeElement;
    closeRef.current?.focus();
    return () => { if (previous instanceof HTMLElement && previous.isConnected) previous.focus(); };
  }, [loading]);

  const handleLanguageChange = (lang: string) => {
    setSelectedLang(lang);
    i18n.changeLanguage(lang);
    localStorage.setItem("sonante_lang", lang);
  };

  const handleAddFolder = async () => {
    if (!config || pickingFolderRef.current) return;
    pickingFolderRef.current = true;
    setIsPickingFolder(true);
    try {
      const selected = await audioService.pickDirectory();
      if (selected) {
        setConfig((prev) => !prev || prev.local_folders.includes(selected) ? prev : {
          ...prev,
          local_folders: [...prev.local_folders, selected],
        });
      }
    } catch (err) {
      logError("settings.pick-folder", err);
    } finally {
      pickingFolderRef.current = false;
      setIsPickingFolder(false);
    }
  };

  const handleRemoveFolder = (folderToRemove: string) => {
    if (!config) return;
    setConfig({
      ...config,
      local_folders: config.local_folders.filter((f) => f !== folderToRemove),
    });
  };

  const handleCheckNew = async () => {
    if (scanInProgressRef.current) return;
    scanInProgressRef.current = true;
    setScanning("check");
    setScanUnchanged(false);
    setScanError(null);
    try {
      const changed = await refreshCatalogAfterSuccessfulCheck(
        audioService.checkLocalLibrary,
        onLibraryRescanned,
      );
      setScanUnchanged(!changed);
    } catch (err) {
      setScanError(formatConfigError(err).includes("LOCAL_INDEX_") ? t("indexCoverage.failed") : formatConfigError(err));
    } finally {
      scanInProgressRef.current = false;
      setScanning(null);
    }
  };

  const handleCancelPlexAuth = () => {
    authGenerationRef.current++;
    authActiveRef.current = false;
    setIsPollingPin(false);
    setPinId(null);
    setPinCode(null);
    setLoadingServers(false);
    if (pollTimerRef.current !== null) {
      clearInterval(pollTimerRef.current);
      pollTimerRef.current = null;
    }
  };

  const handleStartPlexAuth = async () => {
    if (!mountedRef.current || authActiveRef.current || saveInProgressRef.current) return;
    const generation = ++authGenerationRef.current;
    const current = () => mountedRef.current && generation === authGenerationRef.current;
    authActiveRef.current = true;
    setIsPollingPin(true);
    setPinCode(null);
    setPinId(null);
    setLoadingServers(false);
    try {
      const pin = await plexService.createPin();
      if (!current()) return;
      setPinCode(pin.code);
      setPinId(pin.id);
      await plexService.openExternalUrl(pin.auth_url);
    } catch (err) {
      if (!current()) return;
      handleCancelPlexAuth();
      logError("plex.pin-create", err);
      alert(t("settings.plexConnectionError"));
    }
  };

  const handleDisconnectPlex = () => {
    handleCancelPlexAuth();
    if (!config) return;
    setConfig({
      ...config,
      plex_token: "",
      plex_url: "",
      plex_server_id: null,
      plex_server_name: null,
    });
    setDiscoveredServers([]);
  };

  useEffect(() => {
    if (!pinId || !isPollingPin) return;
    const generation = authGenerationRef.current;
    const current = () => mountedRef.current && generation === authGenerationRef.current;
    let reading = false;
    let stopped = false;
    const timer = window.setInterval(async () => {
      if (stopped || reading || !current()) return;
      reading = true;
      let discovering = false;
      try {
        const token = await plexService.checkPin(pinId);
        if (stopped || !current()) return;
        if (token) {
          clearInterval(timer);
          if (pollTimerRef.current === timer) pollTimerRef.current = null;
          setIsPollingPin(false);
          setPinId(null);
          setPinCode(null);
          setConfig((prev) => (prev ? { ...prev, plex_token: token } : null));
          setLoadingServers(true);
          discovering = true;
          const servers = await plexService.getServers(token);
          // Successful authentication stops polling; its discovery still belongs
          // to this attempt until cancellation, replacement or unmount.
          if (!current()) return;
          setDiscoveredServers(servers);
          setLoadingServers(false);
          authActiveRef.current = false;
          if (servers.length > 0) {
            const server = servers[0];
            setConfig((prev) => prev ? {
              ...prev,
              plex_url: server.chosen_uri,
              plex_server_id: server.client_identifier,
              plex_server_name: server.name,
            } : null);
          }
        }
      } catch (err) {
        if (!current()) return;
        logError("plex.pin-status", err);
        if (discovering) {
          setLoadingServers(false);
          authActiveRef.current = false;
        }
      } finally { reading = false; }
    }, 1500);
    pollTimerRef.current = timer;
    return () => {
      stopped = true;
      clearInterval(timer);
      if (pollTimerRef.current === timer) pollTimerRef.current = null;
    };
  }, [pinId, isPollingPin]);

  const handleSave = async () => {
    if (!config || saveInProgressRef.current) return;
    saveInProgressRef.current = true;
    // A displayed busy result is momentary. Retry once on Save rather than
    // permanently refusing a choice based on an old observation.
    if (newPhysicalOutputChoice(originalConfigRef.current, config)
      && availability.results.some(row => row.id === config.alsa_device && row.state === "busy")) {
      setSaving(true);
      const result = await availabilityQueryRef.current?.check(config.alsa_device);
      if (!result || result.state === "busy") {
        saveInProgressRef.current = false;
        setSaving(false);
        return;
      }
      setSaving(false);
    }
    availabilityQueryRef.current?.invalidate();
    const restartExpected = originalConfigRef.current
      ? audioRestartExpected(originalConfigRef.current, config)
      : true;
    if (originalConfigRef.current && localFoldersOnlyChanged(originalConfigRef.current, config)) {
      if (onStartLocalFoldersSave(config)) onClose();
      saveInProgressRef.current = false;
      return;
    }
    if (!onSaveStarted(restartExpected)) {
      saveInProgressRef.current = false;
      return;
    }
    setSaving(true);
    let transferred = false;
    try {
      const response = await configService.saveConfig(config);
      if (response.status === "accepted") {
        transferred = true;
        onSaveAccepted(response.operation);
        onClose();
        return;
      }
      onSaved(restartExpected, response.warning);
      onClose();
    } catch (err) {
      const diagnostic = formatConfigError(err);
      if(diagnostic === "DIRECT_CHOICE_CANCELLED") return; // Preserve this complete draft.
      logError("settings.save", err);
      if (diagnostic === "LOCAL_SOURCE_NEW_OVERLAP") {
        alert(t("settings.localFolderOverlap"));
        return; // Keep this draft and the confirmed baseline for correction.
      }
      try {
        const confirmedConfig = await configService.getConfig();
        originalConfigRef.current = { ...confirmedConfig, local_folders: [...confirmedConfig.local_folders] };
        setConfig(confirmedConfig);
      } catch (reloadError) {
        logError(
          "settings.reload-confirmed",
          reloadError,
        );
      }
      if (diagnostic === "LOCAL_FOLDERS_MIXED_CONFIG_CHANGE") {
        alert(t("settings.separateFolderChanges"));
      } else {
        const summary = diagnostic.startsWith("CONFIG_PERSISTENCE_UNCERTAIN:")
          ? t("settings.configPersistenceUncertain")
          : diagnostic.startsWith("Falha ao persistir configuração:")
            ? t("settings.configPersistenceError")
            : diagnostic.startsWith("Falha ao iniciar nova saída:")
              ? t("settings.outputStartError")
              : t("settings.outputConfigChangeError");
        alert(`${summary}\n\n${t("settings.backendReportedStep")}\n${diagnostic}`);
      }
    } finally {
      saveInProgressRef.current = false;
      setSaving(false);
      if (!transferred) onSaveFinished();
    }
  };

  if (loading || !config) return null;

  const isDirect = (config.audio_output_type || "alsa") === "alsa";
  const isSelectedInList = devices.some((d) => d.id === config.alsa_device);
  const isPlexConnected = Boolean(config.plex_token && config.plex_token.trim().length > 0);
  const canCheckSelectedDevice = config.alsa_device.startsWith("hw:")
    && (isSelectedInList || config.alsa_device === originalConfigRef.current?.alsa_device);
  const selectedAvailability = availability.results.find(row => row.id === config.alsa_device)?.state
    ?? (originalConfigRef.current?.audio_output_type === "alsa"
      && !originalConfigRef.current.first_run
      && originalConfigRef.current.alsa_device !== "default"
      && originalConfigRef.current.alsa_device === config.alsa_device ? "current_sonante" : "indeterminate");
  // The existing ownership marker comes from configuration/identity, not proof
  // that MPD opened the PCM. Keep it for query safety, but do not claim live use.
  const displayedAvailability = selectedAvailability === "current_sonante" ? "indeterminate" : selectedAvailability;
  const AvailabilityIcon = availability.checking ? LoaderCircle
    : displayedAvailability === "available" ? CheckCircle2 : displayedAvailability === "busy" ? CircleAlert : CircleHelp;
  const availabilityHelpId = `${titleId}-availability-help`;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-xs select-none p-4">
      <div ref={dialogRef} role="dialog" aria-modal="true" aria-labelledby={titleId}
        onKeyDown={event => {
          if (event.key !== "Tab") return;
          const controls = [...(dialogRef.current?.querySelectorAll<HTMLElement>("button, input, select, summary, a[href], [tabindex='0']") ?? [])]
            .filter(element => element.tabIndex >= 0 && !element.closest("[hidden]") && !element.hasAttribute("disabled")
              && !element.closest("details:not([open]) > :not(summary)"));
          const first = controls[0], last = controls[controls.length - 1];
          if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
          else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
        }} className="bg-[var(--theme-surface-panel,#181818)] border border-[color:var(--theme-border-soft,#2B2B2B)] rounded-xl w-full max-w-6xl h-[min(800px,90dvh)] max-h-[90dvh] flex flex-col shadow-2xl overflow-hidden">
        {/* Cabeçalho */}
        <div className="px-6 py-4 border-b border-[color:var(--theme-border-soft,#262626)] flex items-center justify-between bg-[var(--theme-surface-panel,#1D1D1D)] shrink-0">
          <div className="flex items-center space-x-2.5">
            <Sliders size={20} className="text-[color:var(--theme-fg-accent,#E5A00D)]" />
            <h2 id={titleId} className="text-base font-bold text-[color:var(--theme-fg-primary,#ffffff)] tracking-wide">{t("settings.title")}</h2>
          </div>
          <button
            onClick={onClose}
            ref={closeRef}
            aria-label={t("settings.close")}
            disabled={saving}
            className="p-1.5 text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] rounded-lg hover:bg-[var(--theme-surface-raised,#252525)] transition-colors cursor-pointer disabled:cursor-not-allowed disabled:opacity-50"
          >
            <X size={18} />
          </button>
        </div>

        {initialFolderOverlap && <div role="alert" className="px-6 py-3 text-sm text-[color:var(--theme-fg-accent,var(--color-amber-300))]">{t("settings.localFolderOverlap")}</div>}
        <SettingsPages
          initialPage={initialPage}
          general={<>
            <AppearanceSetting />
            {/* Idioma da interface */}
            <div className="space-y-3">
              <h3 className="text-[11px] font-bold text-[color:var(--theme-fg-accent,#E5A00D)] uppercase tracking-wider flex items-center space-x-1.5">
                <Languages size={14} />
                <span>{t("settings.language")}</span>
              </h3>
              <div className="flex space-x-3">
                <button
                  type="button"
                  onClick={() => handleLanguageChange("pt-BR")}
                  className={`flex-1 py-2.5 px-4 rounded-xl border font-semibold text-xs transition-all cursor-pointer ${
                    selectedLang.startsWith("pt")
                      ? "border-[color:var(--theme-border-accent,#E5A00D)] bg-[var(--theme-surface-accent,#221B0E)] text-[color:var(--theme-fg-accent,#E5A00D)]"
                      : "border-[color:var(--theme-border-soft,#2B2B2B)] bg-[var(--theme-surface-base,#121212)] text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)]"
                  }`}
                >
                  Português (Brasil)
                </button>

                <button
                  type="button"
                  onClick={() => handleLanguageChange("en")}
                  className={`flex-1 py-2.5 px-4 rounded-xl border font-semibold text-xs transition-all cursor-pointer ${
                    selectedLang.startsWith("en")
                      ? "border-[color:var(--theme-border-accent,#E5A00D)] bg-[var(--theme-surface-accent,#221B0E)] text-[color:var(--theme-fg-accent,#E5A00D)]"
                      : "border-[color:var(--theme-border-soft,#2B2B2B)] bg-[var(--theme-surface-base,#121212)] text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)]"
                  }`}
                >
                  English (US)
                </button>
              </div>
            </div>
          </>}
          audio={<>
            {/* Saída e dispositivo */}
            <div className="space-y-3 pt-3 border-t border-[color:var(--theme-border-soft,#242424)]">
              <h3 className="text-[11px] font-bold text-[color:var(--theme-fg-accent,#E5A00D)] uppercase tracking-wider">
                {t("settings.audioGroups.output")}
              </h3>

              {/* Alternador de Modos Claros */}
              <div>
                <label className="block text-[color:var(--theme-fg-secondary,#CCCCCC)] font-semibold mb-2 text-xs">
                  {t("settings.audioBackend")}
                </label>
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                  <button
                    type="button"
                    aria-pressed={isDirect}
                    onClick={() => {
                      if (isDirect || saveInProgressRef.current) return;
                      const proposed: AppConfig = { ...config, audio_output_type: "alsa" };
                      availabilityQueryRef.current?.invalidate();
                      setConfig(proposed);
                      if (shouldCheckContextualDevice(originalConfigRef.current, proposed, devices.map(device => device.id))) {
                        void availabilityQueryRef.current?.check(proposed.alsa_device);
                      }
                    }}
                    className={`p-3 rounded-xl text-left border transition-all cursor-pointer flex flex-col justify-between ${
                      isDirect
                        ? "bg-[var(--theme-surface-accent,#221B0E)] border-[color:var(--theme-border-accent,#E5A00D)] text-[color:var(--theme-fg-primary,#ffffff)]"
                        : "bg-[var(--theme-surface-base,#141414)] border-[color:var(--theme-border-soft,#2A2A2A)] text-[color:var(--theme-fg-muted,#888888)] hover:border-[color:var(--theme-border-soft,#3A3A3A)] hover:text-[color:var(--theme-fg-primary,#ffffff)]"
                    }`}
                  >
                    <div className="flex items-center space-x-2">
                      <ShieldCheck size={16} className={isDirect ? "text-[color:var(--theme-fg-accent,#E5A00D)]" : "text-[color:var(--theme-fg-muted,#777777)]"} />
                      <span className="font-bold text-xs">{t("settings.alsaMode")}</span>
                    </div>
                    <span className="text-[10px] text-[color:var(--theme-fg-muted,#777777)] mt-1 line-clamp-2">
                      {t("settings.directModeCaption")}
                    </span>
                  </button>

                  <button
                    type="button"
                    aria-pressed={!isDirect}
                    onClick={() => {
                      availabilityQueryRef.current?.invalidate();
                      setConfig({ ...config, audio_output_type: "pipewire" });
                    }}
                    className={`p-3 rounded-xl text-left border transition-all cursor-pointer flex flex-col justify-between ${
                      !isDirect
                        ? "bg-[var(--theme-surface-accent,#221B0E)] border-[color:var(--theme-border-accent,#E5A00D)] text-[color:var(--theme-fg-primary,#ffffff)]"
                        : "bg-[var(--theme-surface-base,#141414)] border-[color:var(--theme-border-soft,#2A2A2A)] text-[color:var(--theme-fg-muted,#888888)] hover:border-[color:var(--theme-border-soft,#3A3A3A)] hover:text-[color:var(--theme-fg-primary,#ffffff)]"
                    }`}
                  >
                    <div className="flex items-center space-x-2">
                      <Radio size={16} className={!isDirect ? "text-[color:var(--theme-fg-accent,#E5A00D)]" : "text-[color:var(--theme-fg-muted,#777777)]"} />
                      <span className="font-bold text-xs">{t("settings.pipewireMode")}</span>
                    </div>
                    <span className="text-[10px] text-[color:var(--theme-fg-muted,#777777)] mt-1 line-clamp-2">
                      {t("settings.sharedModeCaption")}
                    </span>
                  </button>
                </div>
              </div>

              {/* Painel Contextual do Modo Selecionado */}
              {isDirect ? (
                <div className="space-y-2.5 pt-1">
                  <label htmlFor={`${titleId}-device`} className="block text-[color:var(--theme-fg-secondary,#CCCCCC)] font-semibold text-xs">
                    {t("settings.alsaDevice")}
                  </label>
                  <select
                    id={`${titleId}-device`}
                    value={config.alsa_device}
                    // Native selects have no portable popup-open event. A new
                    // focus entry (mouse or keyboard) refreshes a busy observation;
                    // reopening while already focused retains the manual retry.
                    onFocus={() => {
                      if (!saveInProgressRef.current && !availability.checking && selectedAvailability === "busy"
                        && shouldCheckContextualDevice(originalConfigRef.current, config, devices.map(device => device.id))) {
                        void availabilityQueryRef.current?.check(config.alsa_device);
                      }
                    }}
                    onChange={(e) => {
                      const id = e.target.value;
                      if (id === config.alsa_device) return;
                      availabilityQueryRef.current?.invalidate();
                      setConfig({ ...config, alsa_device: id });
                      if (id.startsWith("hw:") && devices.some(device => device.id === id)) {
                        void availabilityQueryRef.current?.check(id);
                      }
                    }}
                    className="w-full bg-[var(--theme-surface-base,#121212)] border border-[color:var(--theme-border-soft,#333333)] rounded-lg px-3 py-2 text-[color:var(--theme-fg-primary,#ffffff)] outline-none focus:border-[color:var(--theme-border-accent,#E5A00D)] cursor-pointer text-sm"
                  >
                    {!isSelectedInList && config.alsa_device && (
                      <option value={config.alsa_device} className="bg-[var(--theme-surface-panel,#1A1A1A)] text-[color:var(--theme-fg-primary,#ffffff)] py-1">
                        {t("settings.currentDevice", { id: config.alsa_device })}
                      </option>
                    )}
                    {devices.map((dev) => (
                      <option key={dev.id} value={dev.id} className="bg-[var(--theme-surface-panel,#1A1A1A)] text-[color:var(--theme-fg-primary,#ffffff)] py-1">
                        {dev.name}
                      </option>
                    ))}
                  </select>

                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="flex items-center gap-2">
                      <div role="status" aria-live="polite" aria-atomic="true"
                        className={`flex items-center gap-1.5 text-xs font-medium ${displayedAvailability === "busy" ? "text-[color:var(--theme-fg-accent,#E5A00D)]" : displayedAvailability === "available" ? "text-[color:var(--theme-fg-success,#79C68B)]" : "text-[color:var(--theme-fg-secondary,#AAAAAA)]"}`}>
                        <AvailabilityIcon size={14} aria-hidden="true" className={availability.checking ? "animate-spin" : undefined} />
                        <span>{t(availability.checking ? "settings.deviceAvailability.checking" : `settings.deviceAvailability.${displayedAvailability}`)}</span>
                      </div>
                      <button type="button" aria-label={t("settings.deviceAvailability.helpLabel")}
                        title={t("settings.deviceAvailability.helpLabel")}
                        aria-expanded={showAvailabilityHelp} aria-controls={availabilityHelpId}
                        onClick={() => setShowAvailabilityHelp(open => !open)}
                        className="p-1 rounded text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]">
                        <CircleHelp size={15} aria-hidden="true" />
                      </button>
                    </div>
                    <button type="button"
                      disabled={availability.checking || saving || !canCheckSelectedDevice}
                      onClick={() => { void availabilityQueryRef.current?.check(config.alsa_device); }}
                      className="text-[11px] text-[color:var(--theme-fg-accent,#E5A00D)] hover:text-[color:var(--theme-fg-primary,#ffffff)] disabled:opacity-50 disabled:cursor-not-allowed">
                      {t("settings.deviceAvailability.check")}
                    </button>
                  </div>
                  <div id={availabilityHelpId} hidden={!showAvailabilityHelp}
                    className="text-[11px] text-[color:var(--theme-fg-secondary,#AAAAAA)] leading-relaxed space-y-1 rounded-lg bg-[var(--theme-surface-base,#121212)] p-3">
                    <p>{t("settings.deviceAvailability.helpObservation")}</p>
                    <p>{t("settings.deviceAvailability.helpShared")}</p>
                    <p>{t("settings.deviceAvailability.helpBusy")}</p>
                    <p>{t("settings.deviceAvailability.helpLimits")}</p>
                  </div>
                  {selectedAvailability === "busy" && !availability.checking && newPhysicalOutputChoice(originalConfigRef.current, config) && (
                    <p role="alert" className="text-[11px] text-[color:var(--theme-fg-accent,#E5A00D)]">{t("settings.deviceAvailability.busyAction")}</p>
                  )}
                </div>
              ) : (
                <p className="text-[11px] text-[color:var(--theme-fg-secondary,#AAAAAA)]">{t("settings.sharedActiveNotice")}</p>
              )}
            </div>

            <AudioSignalDiagram mode={!isDirect || config.alsa_device === "default" ? "shared" : "direct"} device={config.alsa_device} />
            {originalConfigRef.current && !originalConfigRef.current.first_run && audioRestartExpected(originalConfigRef.current, config) && (
              <p role="status" className="text-[11px] text-[color:var(--theme-fg-accent,#C9A45D)]">{t("settings.audioChangeNotice")}</p>
            )}
            {isDirect && config.alsa_device === "default" && (
              <p className="text-[11px] text-[color:var(--theme-fg-accent,#C9A45D)]">{t("settings.defaultEndpointNotice")}</p>
            )}
            <div className="space-y-3 pt-3 border-t border-[color:var(--theme-border-soft,#242424)]">
              <h3 className="text-[11px] font-bold text-[color:var(--theme-fg-accent,#E5A00D)] uppercase tracking-wider">{t("settings.audioGroups.processing")}</h3>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 items-start">
                <div>
                  <label htmlFor={`${titleId}-replaygain`} className="block text-[color:var(--theme-fg-secondary,#CCCCCC)] font-semibold mb-1">{t("settings.replayGain")}</label>
                  <select
                    id={`${titleId}-replaygain`}
                    value={config.replay_gain}
                    onChange={(e) =>
                      setConfig({
                        ...config,
                        replay_gain: e.target.value as "off" | "track" | "album",
                      })
                    }
                    className="w-full bg-[var(--theme-surface-base,#121212)] border border-[color:var(--theme-border-soft,#333333)] rounded-lg px-3 py-2 text-[color:var(--theme-fg-primary,#ffffff)] outline-none focus:border-[color:var(--theme-border-accent,#E5A00D)] cursor-pointer"
                  >
                    <option value="off">{t("settings.rgOff")}</option>
                    <option value="album">{t("settings.rgAlbum")}</option>
                    <option value="track">{t("settings.rgTrack")}</option>
                  </select>
                </div>
                {/* Chave de DoP - Disponível apenas em ALSA Direct */}
                <div
                  className={`flex items-center justify-between p-3 rounded-lg border transition-opacity ${
                    isDirect
                      ? "bg-[var(--theme-surface-base,#141414)] border-[color:var(--theme-border-soft,#262626)]"
                      : "bg-[var(--theme-surface-base,#111111)] border-[color:var(--theme-border-soft,#202020)] opacity-50"
                  }`}
                >
                  <div className="flex flex-col pr-4">
                    <span className="text-[color:var(--theme-fg-primary,#ffffff)] font-semibold">{t("settings.dop")}</span>
                    <span className="text-[11px] text-[color:var(--theme-fg-muted,#777777)] mt-0.5">
                      {isDirect ? t("settings.dopRequirement") : t("settings.dopOnlyDirect")}
                    </span>
                  </div>
                  <input
                    type="checkbox"
                    aria-label={t("settings.dop")}
                    disabled={!isDirect}
                    checked={isDirect && config.dop_enabled}
                    onChange={(e) => setConfig({ ...config, dop_enabled: e.target.checked })}
                    className="w-4 h-4 accent-[var(--theme-control-accent,#E5A00D)] cursor-pointer shrink-0 disabled:cursor-not-allowed"
                  />
                </div>

              </div>
              <details className="text-[11px] text-[color:var(--theme-fg-secondary,#999999)] leading-relaxed space-y-2">
                <summary className="cursor-pointer text-[color:var(--theme-fg-secondary,#BBBBBB)]">{t("settings.processingDetails")}</summary>
                <p>{t("settings.processingNotice")}</p>
                <p>{t("settings.volumeNotice")}</p>
                <p>{t("settings.dopTransportNotice")}</p>
              </details>
            </div>
            <div className="space-y-3 pt-3 border-t border-[color:var(--theme-border-soft,#242424)]">
              <h3 className="text-[11px] font-bold text-[color:var(--theme-fg-accent,#E5A00D)] uppercase tracking-wider">{t("settings.audioGroups.performance")}</h3>
              <div>
                <label htmlFor={`${titleId}-buffer`} className="block text-[color:var(--theme-fg-secondary,#CCCCCC)] font-semibold mb-1">{t("settings.ramBuffer")}</label>
                <select
                  id={`${titleId}-buffer`}
                  value={config.audio_buffer_size_kb}
                  onChange={(e) =>
                    setConfig({ ...config, audio_buffer_size_kb: parseInt(e.target.value, 10) })
                  }
                  className="w-full bg-[var(--theme-surface-base,#121212)] border border-[color:var(--theme-border-soft,#333333)] rounded-lg px-3 py-2 text-[color:var(--theme-fg-primary,#ffffff)] outline-none focus:border-[color:var(--theme-border-accent,#E5A00D)] cursor-pointer"
                >
                  <option value={4096}>4 MB</option>
                  <option value={8192}>8 MB</option>
                  <option value={16384}>16 MB</option>
                  <option value={32768}>32 MB</option>
                </select>
              </div>
              <details className="text-[11px] text-[color:var(--theme-fg-secondary,#999999)] leading-relaxed space-y-2">
                <summary className="cursor-pointer text-[color:var(--theme-fg-secondary,#BBBBBB)]">{t("settings.performanceDetails")}</summary>
                <p>{t("settings.bufferNotice")}</p>
              </details>
            </div>
          </>}
          local={<>
            {/* Fontes e atualização */}
            <div className="space-y-3 pt-3 border-t border-[color:var(--theme-border-soft,#242424)]">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <h3 className="text-[11px] font-bold text-[color:var(--theme-fg-accent,#E5A00D)] uppercase tracking-wider flex items-center space-x-1.5">
                  <HardDrive size={14} />
                  <span>{t("settings.localFolders")}</span>
                </h3>

                <div className="flex flex-wrap items-center justify-end gap-2">
                  <button
                    type="button"
                    onClick={handleCheckNew}
                    disabled={scanning !== null || isLibraryUpdating || saving}
                    className="flex items-center space-x-1.5 px-2.5 py-1 rounded bg-[var(--theme-surface-raised,#242424)] hover:bg-[var(--theme-surface-hover,#2C2C2C)] text-[color:var(--theme-fg-secondary,#CCCCCC)] hover:text-[color:var(--theme-fg-primary,#ffffff)] transition-colors cursor-pointer disabled:cursor-not-allowed disabled:opacity-50"
                    title={t("settings.checkNew")}
                  >
                    <RefreshCw size={12} className={scanning === "check" || isLibraryUpdating ? "animate-spin text-[color:var(--theme-fg-accent,#E5A00D)]" : ""} />
                    <span>{t("settings.checkNew")}</span>
                  </button>

                  <button
                    type="button"
                    onClick={handleAddFolder}
                    disabled={isPickingFolder}
                    className="flex items-center space-x-1.5 px-3 py-1 rounded bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold transition-transform active:scale-95 cursor-pointer disabled:cursor-not-allowed disabled:opacity-50"
                  >
                    <FolderPlus size={13} />
                    <span>{t("settings.addFolder")}</span>
                  </button>
                </div>
              </div>

              {scanUnchanged && (
                <p role="status" className="text-[11px] text-[color:var(--theme-fg-secondary,#999999)]">{t("settings.noStructuralChanges")}</p>
              )}
              {scanError && (
                <p role="alert" className="text-[11px] text-[color:var(--theme-fg-error,var(--color-red-400))]">{scanError}</p>
              )}

              <div className="bg-[var(--theme-surface-base,#121212)] border border-[color:var(--theme-border-soft,#2B2B2B)] rounded-lg p-2.5 max-h-36 overflow-y-auto space-y-1.5">
                {config.local_folders.length === 0 ? (
                  <span className="text-[11px] text-[color:var(--theme-fg-muted,#666666)] block text-center py-2">
                    {t("settings.noFolders")}
                  </span>
                ) : (
                  config.local_folders.map((f, idx) => (
                    <div
                      key={f}
                      className="flex items-center justify-between p-2 rounded bg-[var(--theme-surface-panel,#181818)] border border-[color:var(--theme-border-soft,#242424)]"
                    >
                      <span className="truncate pr-2 text-[color:var(--theme-fg-primary,#ffffff)] font-medium" title={f}>
                        <span className="text-[color:var(--theme-fg-muted,#777777)] font-mono mr-1.5">{idx + 1}.</span>
                        {f}
                      </span>
                      <button
                        onClick={() => handleRemoveFolder(f)}
                        className="text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-error,#FF4D4D)] p-1 transition-colors cursor-pointer"
                      >
                        <Trash2 size={14} />
                      </button>
                    </div>
                  ))
                )}
              </div>
            </div>

            <label className="flex items-start gap-2 pt-3 border-t border-[color:var(--theme-border-soft,#242424)] cursor-pointer">
              <input
                type="checkbox"
                checked={config.online_artwork_enabled}
                onChange={(event) => setConfig({ ...config, online_artwork_enabled: event.target.checked })}
                className="mt-0.5 accent-[var(--theme-control-accent,#E5A00D)]"
              />
              <span className="space-y-1">
                <span className="block text-xs text-[color:var(--theme-fg-primary,#ffffff)]">{t("settings.onlineArtwork")}</span>
                <span className="block text-[11px] text-[color:var(--theme-fg-muted,#888888)]">{t("settings.onlineArtworkDescription")}</span>
              </span>
            </label>
          </>}
          plex={<>
            {/* Conexão Plex */}
            <div className="space-y-3 pt-3 border-t border-[color:var(--theme-border-soft,#242424)]">
              <div className="flex items-center justify-between">
                <h3 className="text-[11px] font-bold text-[color:var(--theme-fg-accent,#E5A00D)] uppercase tracking-wider flex items-center space-x-1.5">
                  <Server size={14} />
                  <span>{t("settings.plexConnection")}</span>
                </h3>

                {isPlexConnected && (
                  <button
                    type="button"
                    onClick={handleDisconnectPlex}
                    className="flex items-center space-x-1 text-[11px] text-[color:var(--theme-fg-error,#FF4D4D)] hover:text-[color:var(--theme-fg-error,#FF6666)] font-semibold transition-colors cursor-pointer"
                  >
                    <LogOut size={13} />
                    <span>{t("settings.disconnect")}</span>
                  </button>
                )}
              </div>

              {!isPlexConnected ? (
                <div className="bg-[var(--theme-surface-base,#121212)] border border-[color:var(--theme-border-soft,#2B2B2B)] rounded-xl p-5 flex flex-col items-center justify-center text-center space-y-3">
                  {isPollingPin ? (
                    <div className="flex flex-col items-center space-y-2.5">
                      <div className="flex items-center space-x-2 text-[color:var(--theme-fg-accent,#E5A00D)]">
                        <RefreshCw size={18} className="animate-spin" />
                        <span className="font-bold text-[color:var(--theme-fg-primary,#ffffff)]">{t("settings.waitingBrowser")}</span>
                      </div>

                      {pinCode && (
                        <div className="font-mono text-xl font-black text-[color:var(--theme-fg-accent,#E5A00D)] bg-[var(--theme-surface-panel,#1C1810)] border border-[color:var(--theme-border-accent,#E5A00D)]/40 px-4 py-1.5 rounded-lg tracking-widest">
                          {pinCode}
                        </div>
                      )}

                      <button
                        type="button"
                        onClick={handleCancelPlexAuth}
                        className="text-[11px] text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] underline cursor-pointer pt-1"
                      >
                        {t("settings.cancel")}
                      </button>
                    </div>
                  ) : (
                    <>
                      <span className="text-[color:var(--theme-fg-muted,#888888)]">{t("settings.noPlexConnected")}</span>
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
                  <div className="flex items-center justify-between p-2.5 rounded-lg bg-[var(--theme-surface-panel,#141F14)] border border-[color:var(--theme-border-success,#4BB543)]/40 text-xs">
                    <div className="flex items-center space-x-2 text-[color:var(--theme-fg-success,#4BB543)]">
                      <CheckCircle2 size={16} />
                      <span className="font-bold">{t("settings.sessionActive")}</span>
                    </div>
                  </div>

                  <div className="space-y-2">
                    <label className="block text-[color:var(--theme-fg-secondary,#CCCCCC)] font-semibold">{t("settings.activeServer")}</label>
                    {loadingServers ? (
                      <div className="text-[color:var(--theme-fg-muted,#888888)] flex items-center space-x-2 py-2">
                        <RefreshCw size={13} className="animate-spin text-[color:var(--theme-fg-accent,#E5A00D)]" />
                        <span>{t("settings.searchingServers")}</span>
                      </div>
                    ) : discoveredServers.length === 0 ? (
                      <div className="text-[color:var(--theme-fg-muted,#888888)] p-2 bg-[var(--theme-surface-base,#121212)] rounded-lg border border-[color:var(--theme-border-soft,#262626)]">
                        {config.plex_url || t("settings.noServerSelected")}
                      </div>
                    ) : (
                      <div className="space-y-1.5 max-h-36 overflow-y-auto">
                        {discoveredServers.map((srv) => {
                          const isSelected = config.plex_server_id
                            ? config.plex_server_id === srv.client_identifier
                            : config.plex_url === srv.chosen_uri;
                          const localConn = srv.connections.find((c) => c.local);
                          const selectServer = () =>
                            setConfig({
                              ...config,
                              plex_url: srv.chosen_uri,
                              plex_server_id: srv.client_identifier,
                              plex_server_name: srv.name,
                            });
                          return (
                            <div
                              key={srv.client_identifier}
                              onClick={selectServer}
                              className={`p-2.5 rounded-lg border transition-all cursor-pointer flex items-center justify-between ${
                                isSelected
                                  ? "border-[color:var(--theme-border-accent,#E5A00D)] bg-[var(--theme-surface-accent,#221B0E)]"
                                  : "border-[color:var(--theme-border-soft,#262626)] bg-[var(--theme-surface-base,#121212)] hover:bg-[var(--theme-surface-panel,#181818)]"
                              }`}
                            >
                              <div>
                                <span className="font-bold text-[color:var(--theme-fg-primary,#ffffff)] block">{srv.name}</span>
                                <span className="text-[10px] text-[color:var(--theme-fg-muted,#777777)] font-mono">
                                  {localConn ? `${localConn.address}:${localConn.port} (LAN)` : srv.chosen_uri}
                                </span>
                              </div>
                              <input
                                type="radio"
                                checked={isSelected}
                                onChange={selectServer}
                                className="accent-[var(--theme-control-accent,#E5A00D)] cursor-pointer"
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
                  className="flex items-center space-x-1.5 text-[11px] text-[color:var(--theme-fg-muted,#777777)] hover:text-[color:var(--theme-fg-secondary,#CCCCCC)] transition-colors cursor-pointer"
                >
                  {showManualPlex ? <ChevronUp size={13} /> : <ChevronDown size={13} />}
                  <span>{t("settings.manualConfig")}</span>
                </button>

                {showManualPlex && (
                  <div className="grid grid-cols-2 gap-3 pt-2 animate-in fade-in duration-100">
                    <div>
                      <label className="block text-[11px] text-[color:var(--theme-fg-secondary,#999999)] mb-1">{t("settings.serverUrl")}</label>
                      <input
                        type="text"
                        value={config.plex_url}
                        onChange={(e) =>
                          setConfig({
                            ...config,
                            plex_url: e.target.value,
                            plex_server_id: null,
                            plex_server_name: null,
                          })
                        }
                        placeholder="http://192.168.1.100:32400"
                        className="w-full bg-[var(--theme-surface-base,#121212)] border border-[color:var(--theme-border-soft,#333333)] rounded-lg px-3 py-1.5 text-[color:var(--theme-fg-primary,#ffffff)] outline-none focus:border-[color:var(--theme-border-accent,#E5A00D)]"
                      />
                    </div>
                    <div>
                      <label className="block text-[11px] text-[color:var(--theme-fg-secondary,#999999)] mb-1">{t("settings.plexToken")}</label>
                      <input
                        type="password"
                        value={config.plex_token}
                        onChange={(e) => setConfig({ ...config, plex_token: e.target.value })}
                        placeholder={t("settings.manualTokenPlaceholder")}
                        className="w-full bg-[var(--theme-surface-base,#121212)] border border-[color:var(--theme-border-soft,#333333)] rounded-lg px-3 py-1.5 text-[color:var(--theme-fg-primary,#ffffff)] outline-none focus:border-[color:var(--theme-border-accent,#E5A00D)]"
                      />
                    </div>
                  </div>
                )}
              </div>
            </div>
          </>}
        />

        {/* Rodapé */}
        <div className="px-6 py-4 border-t border-[color:var(--theme-border-soft,#262626)] bg-[var(--theme-surface-panel,#1D1D1D)] flex items-center gap-3 shrink-0">
          <button
            onClick={onClose}
            disabled={saving}
            className="px-4 py-2 rounded-lg text-xs font-semibold text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] hover:bg-[var(--theme-surface-hover,#262626)] transition-colors cursor-pointer disabled:cursor-not-allowed disabled:opacity-50"
          >
            {t("settings.cancel")}
          </button>

          <button
            onClick={handleSave}
            disabled={saving || (newPhysicalOutputChoice(originalConfigRef.current, config) && availability.checking)}
            className="flex shrink-0 items-center space-x-2 px-5 py-2 rounded-lg bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-md transition-transform active:scale-95 cursor-pointer disabled:cursor-not-allowed disabled:opacity-50"
          >
            {saving ? <RefreshCw size={15} className="animate-spin" /> : <Check size={15} />}
            <span>
              {saving ? t("settings.savingSettings") : t("settings.save")}
            </span>
          </button>
        </div>
      </div>
    </div>
  );
};

export function QueueClearConfirmationDialog({ onCancel, onConfirm }: {
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation();
  const titleId = useId();
  const descriptionId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    cancelRef.current?.focus();
    return () => { if (previousFocus?.isConnected) previousFocus.focus(); };
  }, []);

  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/70 p-4">
      <div
        ref={dialogRef}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
        className="w-full max-w-md rounded-xl border border-[color:var(--theme-border-soft,#383838)] bg-[var(--theme-surface-panel,#181818)] p-6 text-[color:var(--theme-fg-primary,#ffffff)] shadow-2xl"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            onCancel();
          } else if (event.key === "Tab") {
            const buttons = dialogRef.current?.querySelectorAll<HTMLButtonElement>("button");
            if (!buttons?.length) return;
            const first = buttons[0];
            const last = buttons[buttons.length - 1];
            if (event.shiftKey && document.activeElement === first) {
              event.preventDefault(); last.focus();
            } else if (!event.shiftKey && document.activeElement === last) {
              event.preventDefault(); first.focus();
            }
          }
        }}
      >
        <h2 id={titleId} className="text-base font-bold">{t("settings.queueClearTitle")}</h2>
        <p id={descriptionId} className="mt-3 text-sm text-[color:var(--theme-fg-secondary,#CCCCCC)]">{t("settings.queueClearDescription")}</p>
        <div className="mt-6 flex flex-wrap justify-end gap-3">
          <button ref={cancelRef} type="button" onClick={onCancel} className="rounded-lg px-4 py-2 text-sm hover:bg-[var(--theme-surface-hover,#333333)] focus-visible:outline-2 focus-visible:outline-[color:var(--theme-border-accent,#E5A00D)]">
            {t("settings.queueClearCancel")}
          </button>
          <button type="button" onClick={onConfirm} className="rounded-lg bg-[#E5A00D] px-4 py-2 text-sm font-bold text-black hover:bg-[#F5B01D] focus-visible:outline-2 focus-visible:outline-[color:var(--theme-border-soft,#ffffff)]">
            {t("settings.queueClearConfirm")}
          </button>
        </div>
      </div>
    </div>
  );
}

export default SettingsModal;
