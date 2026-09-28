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
} from "lucide-react";
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
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [scanning, setScanning] = useState(false);

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

  // Iniciar autenticação Plex via PIN
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
      alert("Não foi possível conectar aos servidores do Plex. Verifique sua conexão à internet.");
    }
  };

  const handleCancelPlexAuth = () => {
    setIsPollingPin(false);
    setPinId(null);
    setPinCode(null);
    if (pollTimerRef.current) clearInterval(pollTimerRef.current);
  };

  // Desconectar / Fazer Logout da conta Plex
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

  const isSelectedInList = devices.some((d) => d.id === config.alsa_device);
  const isPlexConnected = config.plex_token && config.plex_token.trim().length > 0;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-xs select-none p-4">
      <div className="bg-[#181818] border border-[#2B2B2B] rounded-xl w-full max-w-2xl max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
        {/* Cabeçalho */}
        <div className="px-6 py-4 border-b border-[#262626] flex items-center justify-between bg-[#1D1D1D]">
          <div className="flex items-center space-x-2.5">
            <Sliders size={20} className="text-[#E5A00D]" />
            <h2 className="text-base font-bold text-white tracking-wide">Preferências do Sistema</h2>
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
          {/* Seção 1: Saída de Áudio ALSA */}
          <div className="space-y-3">
            <h3 className="text-[11px] font-bold text-[#E5A00D] uppercase tracking-wider">
              Dispositivo de Saída de Áudio
            </h3>
            <div>
              <label className="block text-[#CCCCCC] font-semibold mb-1.5">
                Placa de Som ALSA (Bit-Perfect)
              </label>
              <select
                value={config.alsa_device}
                onChange={(e) => setConfig({ ...config, alsa_device: e.target.value })}
                className="w-full bg-[#121212] border border-[#333333] rounded-lg px-3 py-2 text-white outline-none focus:border-[#E5A00D] cursor-pointer"
              >
                {!isSelectedInList && config.alsa_device && (
                  <option value={config.alsa_device} className="bg-[#1A1A1A] text-white py-1">
                    Dispositivo Atual ({config.alsa_device})
                  </option>
                )}
                {devices.map((dev) => (
                  <option key={dev.id} value={dev.id} className="bg-[#1A1A1A] text-white py-1">
                    {dev.name}
                  </option>
                ))}
              </select>
            </div>
          </div>

          {/* Seção 2: Gerenciamento de Pastas Locais */}
          <div className="space-y-3 pt-3 border-t border-[#242424]">
            <div className="flex items-center justify-between">
              <h3 className="text-[11px] font-bold text-[#E5A00D] uppercase tracking-wider flex items-center space-x-1.5">
                <HardDrive size={14} />
                <span>Pastas de Armazenamento Local</span>
              </h3>

              <div className="flex items-center space-x-2">
                <button
                  type="button"
                  onClick={handleForceRescan}
                  disabled={scanning}
                  className="flex items-center space-x-1.5 px-2.5 py-1 rounded bg-[#242424] hover:bg-[#2C2C2C] text-[#CCCCCC] hover:text-white transition-colors cursor-pointer"
                  title="Verificar novas músicas"
                >
                  <RefreshCw size={12} className={scanning ? "animate-spin text-[#E5A00D]" : ""} />
                  <span>Verificar Novidades</span>
                </button>

                <button
                  type="button"
                  onClick={handleAddFolder}
                  className="flex items-center space-x-1.5 px-3 py-1 rounded bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold transition-transform active:scale-95 cursor-pointer"
                >
                  <FolderPlus size={13} />
                  <span>Adicionar</span>
                </button>
              </div>
            </div>

            <div className="bg-[#121212] border border-[#2B2B2B] rounded-lg p-2.5 max-h-36 overflow-y-auto space-y-1.5">
              {config.local_folders.length === 0 ? (
                <span className="text-[11px] text-[#666666] block text-center py-2">
                  Nenhuma pasta local configurada.
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

          {/* Seção 3: Motor de Áudio & Bit-Perfect */}
          <div className="space-y-3 pt-3 border-t border-[#242424]">
            <h3 className="text-[11px] font-bold text-[#E5A00D] uppercase tracking-wider flex items-center space-x-1.5">
              <ShieldCheck size={14} />
              <span>Motor de Áudio & Fidelidade Bit-Perfect</span>
            </h3>

            <div className="flex items-center justify-between bg-[#141414] p-3 rounded-lg border border-[#262626]">
              <div className="flex flex-col pr-4">
                <span className="text-white font-semibold">DSD over PCM (DoP)</span>
                <span className="text-[11px] text-[#777777] mt-0.5">
                  Encapsula fluxos DSD nativos em pacotes PCM para envio direto a DACs compatíveis sem conversão.
                </span>
              </div>
              <input
                type="checkbox"
                checked={config.dop_enabled}
                onChange={(e) => setConfig({ ...config, dop_enabled: e.target.checked })}
                className="w-4 h-4 accent-[#E5A00D] cursor-pointer shrink-0"
              />
            </div>

            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-[#CCCCCC] font-semibold mb-1">Buffer RAM</label>
                <select
                  value={config.audio_buffer_size_kb}
                  onChange={(e) =>
                    setConfig({ ...config, audio_buffer_size_kb: parseInt(e.target.value, 10) })
                  }
                  className="w-full bg-[#121212] border border-[#333333] rounded-lg px-3 py-2 text-white outline-none focus:border-[#E5A00D] cursor-pointer"
                >
                  <option value={4096}>4 MB (Padrão)</option>
                  <option value={8192}>8 MB</option>
                  <option value={16384}>16 MB (Recomendado Hi-Res)</option>
                  <option value={32768}>32 MB (Ultra Buffer)</option>
                </select>
              </div>

              <div>
                <label className="block text-[#CCCCCC] font-semibold mb-1">ReplayGain</label>
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
                  <option value="off">Desativado (Bit-Perfect Puro)</option>
                  <option value="album">Por Álbum (Preserva Dinâmica)</option>
                  <option value="track">Por Faixa</option>
                </select>
              </div>
            </div>
          </div>

          {/* Seção 4: Conexão Plex Media Server com Login OAuth & Logout */}
          <div className="space-y-3 pt-3 border-t border-[#242424]">
            <div className="flex items-center justify-between">
              <h3 className="text-[11px] font-bold text-[#E5A00D] uppercase tracking-wider flex items-center space-x-1.5">
                <Server size={14} />
                <span>Conexão Plex Media Server</span>
              </h3>

              {isPlexConnected && (
                <button
                  type="button"
                  onClick={handleDisconnectPlex}
                  className="flex items-center space-x-1 text-[11px] text-[#FF4D4D] hover:text-[#FF6666] font-semibold transition-colors cursor-pointer"
                  title="Encerrar sessão Plex"
                >
                  <LogOut size={13} />
                  <span>Desconectar Conta</span>
                </button>
              )}
            </div>

            {!isPlexConnected ? (
              <div className="bg-[#121212] border border-[#2B2B2B] rounded-xl p-5 flex flex-col items-center justify-center text-center space-y-3">
                {isPollingPin ? (
                  <div className="flex flex-col items-center space-y-2.5">
                    <div className="flex items-center space-x-2 text-[#E5A00D]">
                      <RefreshCw size={18} className="animate-spin" />
                      <span className="font-bold text-white">Aguardando autorização no navegador...</span>
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
                      Cancelar
                    </button>
                  </div>
                ) : (
                  <>
                    <span className="text-[#888888]">Nenhuma conta Plex conectada no momento.</span>
                    <button
                      type="button"
                      onClick={handleStartPlexAuth}
                      className="flex items-center space-x-2 px-5 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs transition-transform active:scale-95 cursor-pointer shadow"
                    >
                      <ExternalLink size={14} />
                      <span>Conectar Conta Plex</span>
                    </button>
                  </>
                )}
              </div>
            ) : (
              <div className="space-y-3">
                <div className="flex items-center justify-between p-2.5 rounded-lg bg-[#141F14] border border-[#4BB543]/40 text-xs">
                  <div className="flex items-center space-x-2 text-[#4BB543]">
                    <CheckCircle2 size={16} />
                    <span className="font-bold">Sessão ativa e autenticada</span>
                  </div>
                </div>

                <div className="space-y-2">
                  <label className="block text-[#CCCCCC] font-semibold">Servidor de Áudio Ativo:</label>
                  {loadingServers ? (
                    <div className="text-[#888888] flex items-center space-x-2 py-2">
                      <RefreshCw size={13} className="animate-spin text-[#E5A00D]" />
                      <span>Buscando servidores na rede...</span>
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

            {/* Configuração Manual Recolhível */}
            <div className="pt-2">
              <button
                type="button"
                onClick={() => setShowManualPlex(!showManualPlex)}
                className="flex items-center space-x-1.5 text-[11px] text-[#777777] hover:text-[#CCCCCC] transition-colors cursor-pointer"
              >
                {showManualPlex ? <ChevronUp size={13} /> : <ChevronDown size={13} />}
                <span>Configuração Manual (Avançado)</span>
              </button>

              {showManualPlex && (
                <div className="grid grid-cols-2 gap-3 pt-2 animate-in fade-in duration-100">
                  <div>
                    <label className="block text-[11px] text-[#999999] mb-1">URL do Servidor</label>
                    <input
                      type="text"
                      value={config.plex_url}
                      onChange={(e) => setConfig({ ...config, plex_url: e.target.value })}
                      placeholder="http://192.168.1.100:32400"
                      className="w-full bg-[#121212] border border-[#333333] rounded-lg px-3 py-1.5 text-white outline-none focus:border-[#E5A00D]"
                    />
                  </div>
                  <div>
                    <label className="block text-[11px] text-[#999999] mb-1">X-Plex-Token</label>
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
            Cancelar
          </button>

          <button
            onClick={handleSave}
            disabled={saving}
            className="flex items-center space-x-2 px-5 py-2 rounded-lg bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-md transition-transform active:scale-95 cursor-pointer disabled:opacity-50"
          >
            <Check size={15} />
            <span>{saving ? "Gravando..." : "Gravar Preferências"}</span>
          </button>
        </div>
      </div>
    </div>
  );
};

export default SettingsModal;
