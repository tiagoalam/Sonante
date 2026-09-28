import React, { useEffect, useState } from "react";
import { X, Check, Server, HardDrive, Sliders, ShieldCheck, FolderPlus, Trash2, RefreshCw } from "lucide-react";
import { AppConfig } from "../types/config";
import { AudioDevice } from "../types/audio";
import { configService } from "../services/config";
import { audioService } from "../services/audio";

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

  useEffect(() => {
    Promise.all([configService.getConfig(), configService.getAudioDevices()])
      .then(([cfg, devs]) => {
        setConfig(cfg);
        setDevices(devs);
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
                  Encapsula fluxos DSD nativos em pacotes PCM para envio direto a DACs compatíveis com DoP sem conversão.
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
                <label className="block text-[#CCCCCC] font-semibold mb-1">
                  Buffer de Memória RAM
                </label>
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
                <label className="block text-[#CCCCCC] font-semibold mb-1">
                  Nivelamento de Volume (ReplayGain)
                </label>
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

          {/* Seção 4: Conexão Plex Media Server */}
          <div className="space-y-3 pt-3 border-t border-[#242424]">
            <h3 className="text-[11px] font-bold text-[#E5A00D] uppercase tracking-wider">
              Conexão Plex Media Server
            </h3>

            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-[#CCCCCC] font-semibold mb-1">URL do Servidor</label>
                <input
                  type="text"
                  value={config.plex_url}
                  onChange={(e) => setConfig({ ...config, plex_url: e.target.value })}
                  placeholder="http://192.168.1.100:32400"
                  className="w-full bg-[#121212] border border-[#333333] rounded-lg px-3 py-2 text-white outline-none focus:border-[#E5A00D]"
                />
              </div>

              <div>
                <label className="block text-[#CCCCCC] font-semibold mb-1">X-Plex-Token</label>
                <input
                  type="password"
                  value={config.plex_token}
                  onChange={(e) => setConfig({ ...config, plex_token: e.target.value })}
                  placeholder="Token de autenticação"
                  className="w-full bg-[#121212] border border-[#333333] rounded-lg px-3 py-2 text-white outline-none focus:border-[#E5A00D]"
                />
              </div>
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
