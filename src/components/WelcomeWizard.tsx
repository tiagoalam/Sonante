import React, { useState } from "react";
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
} from "lucide-react";
import { AppConfig } from "../types/config";
import { AudioDevice } from "../types/audio";
import { audioService } from "../services/audio";
import { configService } from "../services/config";

interface Props {
  initialConfig: AppConfig;
  devices: AudioDevice[];
  onFinish: (newConfig: AppConfig) => void;
}

export const WelcomeWizard: React.FC<Props> = ({ initialConfig, devices, onFinish }) => {
  const [step, setStep] = useState<number>(1);
  const [config, setConfig] = useState<AppConfig>({ ...initialConfig });

  // Controle das fontes escolhidas
  const [useLocal, setUseLocal] = useState<boolean>(true);
  const [usePlex, setUsePlex] = useState<boolean>(false);
  const [isFinishing, setIsFinishing] = useState<boolean>(false);

  // Seleção nativa de pastas via janela do sistema
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

  const handleComplete = async () => {
    setIsFinishing(true);
    try {
      const finalConfig: AppConfig = {
        ...config,
        first_run: false,
      };
      await configService.saveConfig(finalConfig);
      await audioService.rescanLibrary();
      onFinish(finalConfig);
    } catch (err) {
      console.error("Falha ao salvar configuração inicial:", err);
      alert("Houve um erro ao salvar as preferências iniciais.");
    } finally {
      setIsFinishing(false);
    }
  };

  // Cálculo das etapas dinâmicas
  const totalSteps = 2 + (useLocal ? 1 : 0) + (usePlex ? 1 : 0) + 1;

  const nextStep = () => {
    if (step === 2 && !useLocal && !usePlex) {
      alert("Selecione pelo menos uma fonte de música (Armazenamento Local ou Servidor Plex).");
      return;
    }
    setStep((s) => s + 1);
  };

  const prevStep = () => {
    setStep((s) => Math.max(1, s - 1));
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-[#0C0C0C] select-none p-6 text-white font-sans">
      <div className="bg-[#161616] border border-[#2B2B2B] rounded-2xl w-full max-w-2xl min-h-[520px] flex flex-col shadow-2xl overflow-hidden animate-in fade-in duration-200">
        {/* Topo do Wizard */}
        <div className="px-8 py-5 border-b border-[#242424] flex items-center justify-between bg-[#191919]">
          <div className="flex items-center space-x-3">
            <div className="w-9 h-9 rounded-xl bg-[#E5A00D]/10 border border-[#E5A00D]/30 flex items-center justify-center text-[#E5A00D]">
              <Sparkles size={20} />
            </div>
            <div>
              <h1 className="text-base font-black tracking-wider text-[#E5A00D]">SONANTE</h1>
              <p className="text-[11px] text-[#888888]">Assistente de Configuração Inicial</p>
            </div>
          </div>

          <div className="flex items-center space-x-1.5">
            <span className="text-xs font-mono text-[#888888]">Etapa {step}</span>
          </div>
        </div>

        {/* Conteúdo Dinâmico */}
        <div className="p-8 flex-1 flex flex-col justify-between">
          {/* ETAPA 1: DAC / SAÍDA DE ÁUDIO */}
          {step === 1 && (
            <div className="space-y-5 animate-in fade-in duration-150">
              <div className="space-y-1.5">
                <h2 className="text-xl font-bold text-white flex items-center space-x-2">
                  <Sliders size={20} className="text-[#E5A00D]" />
                  <span>Escolha sua Saída de Áudio</span>
                </h2>
                <p className="text-xs text-[#999999] leading-relaxed">
                  Para garantir reprodução <strong>bit-perfect</strong> nativa, selecione o seu DAC USB
                  dedicado. Dispositivos diretos operam sem reamostragem do sistema.
                </p>
              </div>

              <div className="space-y-2 pt-2">
                <label className="block text-xs font-bold text-[#CCCCCC] uppercase tracking-wider">
                  Dispositivo ALSA Detectado
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
                <span>
                  O modo exclusivo garante que o fluxo digital seja enviado bit a bit diretamente ao seu
                  hardware em taxas até DSD128 e PCM 192/384 kHz.
                </span>
              </div>
            </div>
          )}

          {/* ETAPA 2: ESCOLHA DE FONTES DE MÍDIA */}
          {step === 2 && (
            <div className="space-y-5 animate-in fade-in duration-150">
              <div className="space-y-1.5">
                <h2 className="text-xl font-bold text-white">De onde virão suas músicas?</h2>
                <p className="text-xs text-[#999999]">
                  Você pode usar apenas arquivos locais, seu servidor Plex, ou ambos simultaneamente.
                </p>
              </div>

              <div className="grid grid-cols-2 gap-4 pt-2">
                {/* Cartão Local */}
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
                    <h3 className="font-bold text-sm text-white">Armazenamento Local</h3>
                    <p className="text-[11px] text-[#888888] leading-relaxed">
                      Músicas no seu SSD, HD interno, pendrive ou compartilhamentos de rede NFS/SMB.
                    </p>
                  </div>
                  <div className="pt-4 flex items-center justify-between text-xs">
                    <span className={useLocal ? "text-[#E5A00D] font-bold" : "text-[#666666]"}>
                      {useLocal ? "Ativado" : "Desativado"}
                    </span>
                    <input
                      type="checkbox"
                      checked={useLocal}
                      onChange={() => {}}
                      className="w-4 h-4 accent-[#E5A00D] cursor-pointer"
                    />
                  </div>
                </div>

                {/* Cartão Plex */}
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
                    <h3 className="font-bold text-sm text-white">Servidor Plex</h3>
                    <p className="text-[11px] text-[#888888] leading-relaxed">
                      Streaming de alta fidelidade das bibliotecas do seu Plex Media Server.
                    </p>
                  </div>
                  <div className="pt-4 flex items-center justify-between text-xs">
                    <span className={usePlex ? "text-[#E5A00D] font-bold" : "text-[#666666]"}>
                      {usePlex ? "Ativado" : "Desativado"}
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

          {/* ETAPA 3: GERENCIAR PASTAS LOCAIS (Se useLocal estiver ativo) */}
          {step === 3 && useLocal && (
            <div className="space-y-4 animate-in fade-in duration-150">
              <div className="flex items-center justify-between">
                <div>
                  <h2 className="text-xl font-bold text-white">Pastas Locais de Músicas</h2>
                  <p className="text-xs text-[#999999] mt-0.5">
                    Adicione uma ou mais pastas para indexação unificada.
                  </p>
                </div>

                <button
                  type="button"
                  onClick={handleAddFolder}
                  className="flex items-center space-x-2 px-4 py-2 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-md transition-transform active:scale-95 cursor-pointer"
                >
                  <FolderPlus size={16} />
                  <span>Adicionar Pasta</span>
                </button>
              </div>

              {/* Lista de Pastas Adicionadas */}
              <div className="bg-[#111111] border border-[#262626] rounded-xl p-3 min-h-[160px] max-h-[220px] overflow-y-auto space-y-2">
                {config.local_folders.length === 0 ? (
                  <div className="h-32 flex flex-col items-center justify-center text-[#666666] space-y-2">
                    <Disc3 size={32} className="opacity-40" />
                    <span className="text-xs">Nenhuma pasta adicionada ainda. Clique em "Adicionar Pasta".</span>
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
                        title="Remover Pasta"
                      >
                        <Trash2 size={15} />
                      </button>
                    </div>
                  ))
                )}
              </div>
            </div>
          )}

          {/* ETAPA 4: CONFIGURAÇÃO PLEX (Se usePlex estiver ativo) */}
          {((step === 3 && !useLocal && usePlex) || (step === 4 && useLocal && usePlex)) && (
            <div className="space-y-4 animate-in fade-in duration-150">
              <div>
                <h2 className="text-xl font-bold text-white">Conexão Plex Media Server</h2>
                <p className="text-xs text-[#999999] mt-0.5">
                  Informe o endereço local e a credencial do seu servidor Plex.
                </p>
              </div>

              <div className="grid grid-cols-2 gap-3 pt-1">
                <div>
                  <label className="block text-xs font-bold text-[#CCCCCC] mb-1">
                    URL do Servidor
                  </label>
                  <input
                    type="text"
                    value={config.plex_url}
                    onChange={(e) => setConfig({ ...config, plex_url: e.target.value })}
                    placeholder="http://192.168.10.244:32400"
                    className="w-full bg-[#111111] border border-[#333333] rounded-lg px-3 py-2 text-xs text-white outline-none focus:border-[#E5A00D]"
                  />
                </div>

                <div>
                  <label className="block text-xs font-bold text-[#CCCCCC] mb-1">
                    X-Plex-Token
                  </label>
                  <input
                    type="password"
                    value={config.plex_token}
                    onChange={(e) => setConfig({ ...config, plex_token: e.target.value })}
                    placeholder="Token de autenticação"
                    className="w-full bg-[#111111] border border-[#333333] rounded-lg px-3 py-2 text-xs text-white outline-none focus:border-[#E5A00D]"
                  />
                </div>
              </div>

              <div className="space-y-1.5 pt-2">
                <label className="block text-xs font-bold text-[#CCCCCC]">Modo de Transporte</label>
                <div className="grid grid-cols-2 gap-3">
                  <button
                    type="button"
                    onClick={() => setConfig({ ...config, playback_mode: "http" })}
                    className={`p-3 rounded-lg border text-left cursor-pointer ${
                      config.playback_mode === "http"
                        ? "border-[#E5A00D] bg-[#221B0E]"
                        : "border-[#262626] bg-[#141414]"
                    }`}
                  >
                    <span className="font-bold text-xs text-white block">Streaming HTTP Direto</span>
                    <span className="text-[11px] text-[#777777]">
                      Mais simples, busca o binário diretamente via rede sem montagem física.
                    </span>
                  </button>

                  <button
                    type="button"
                    onClick={() => setConfig({ ...config, playback_mode: "local" })}
                    className={`p-3 rounded-lg border text-left cursor-pointer ${
                      config.playback_mode === "local"
                        ? "border-[#E5A00D] bg-[#221B0E]"
                        : "border-[#262626] bg-[#141414]"
                    }`}
                  >
                    <span className="font-bold text-xs text-white block">Acesso Local / NFS</span>
                    <span className="text-[11px] text-[#777777]">
                      Lê os arquivos a partir do ponto de montagem NFS mapeado no Manjaro.
                    </span>
                  </button>
                </div>
              </div>
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
                  <span>Tudo pronto para começar!</span>
                </h2>
                <p className="text-xs text-[#999999] leading-relaxed">
                  O Sonante inicializará o motor de áudio em background e fará a verificação diferencial
                  das suas faixas. Você poderá alterar qualquer preferência a qualquer momento no menu lateral.
                </p>
              </div>

              <div className="bg-[#121212] border border-[#242424] rounded-xl p-4 space-y-2 text-xs">
                <div className="flex justify-between py-1 border-b border-[#1F1F1F]">
                  <span className="text-[#888888]">Saída ALSA:</span>
                  <span className="font-mono text-white truncate max-w-xs">{config.alsa_device}</span>
                </div>
                <div className="flex justify-between py-1 border-b border-[#1F1F1F]">
                  <span className="text-[#888888]">Pastas Locais:</span>
                  <span className="text-white">{config.local_folders.length} diretório(s)</span>
                </div>
                <div className="flex justify-between py-1">
                  <span className="text-[#888888]">Plex Media Server:</span>
                  <span className="text-white">{usePlex ? "Configurado" : "Desativado"}</span>
                </div>
              </div>
            </div>
          )}

          {/* Rodapé com Navegação */}
          <div className="pt-6 border-t border-[#242424] flex items-center justify-between">
            {step > 1 ? (
              <button
                type="button"
                onClick={prevStep}
                className="flex items-center space-x-2 px-4 py-2 rounded-xl bg-[#222222] hover:bg-[#2A2A2A] text-xs font-bold text-[#CCCCCC] hover:text-white transition-colors cursor-pointer"
              >
                <ArrowLeft size={15} />
                <span>Voltar</span>
              </button>
            ) : (
              <div />
            )}

            {/* Próximo ou Concluir */}
            {step < totalSteps - 1 ? (
              <button
                type="button"
                onClick={nextStep}
                className="flex items-center space-x-2 px-6 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-md transition-transform active:scale-95 cursor-pointer"
              >
                <span>Avançar</span>
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
                <span>{isFinishing ? "Inicializando..." : "Iniciar Sonante"}</span>
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
