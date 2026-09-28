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
  ChevronDown,
  ChevronUp,
} from "lucide-react";
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
  const [step, setStep] = useState<number>(1);
  const [config, setConfig] = useState<AppConfig>({ ...initialConfig });

  // Garante que o primeiro DAC detectado seja gravado se o campo estiver vazio
  useEffect(() => {
    if (devices.length > 0) {
      const exists = devices.some((d) => d.id === config.alsa_device);
      if (!config.alsa_device || !exists) {
        setConfig((prev) => ({ ...prev, alsa_device: devices[0].id }));
      }
    }
  }, [devices]);

  const [useLocal, setUseLocal] = useState<boolean>(true);
  const [usePlex, setUsePlex] = useState<boolean>(false);
  const [isFinishing, setIsFinishing] = useState<boolean>(false);

  // Estados do Fluxo de Login Plex (PIN / OAuth)
  const [pinCode, setPinCode] = useState<string | null>(null);
  const [pinId, setPinId] = useState<number | null>(null);
  const [isPollingPin, setIsPollingPin] = useState(false);
  const [discoveredServers, setDiscoveredServers] = useState<PlexServerResource[]>([]);
  const [selectedServerUri, setSelectedServerUri] = useState<string>(config.plex_url || "");
  const [loadingServers, setLoadingServers] = useState(false);
  const [showManualPlex, setShowManualPlex] = useState(false);
  const pollTimerRef = useRef<number | null>(null);

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

  // Iniciar Login Oficial do Plex via PIN
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
      alert("Não foi possível conectar aos servidores do Plex. Verifique sua conexão com a internet.");
    }
  };

  // Polling para checar autorização do PIN no navegador
  useEffect(() => {
    if (!pinId || !isPollingPin) return;

    pollTimerRef.current = window.setInterval(async () => {
      try {
        const token = await plexService.checkPin(pinId);
        if (token) {
          setIsPollingPin(false);
          setPinId(null);
          setConfig((prev) => ({ ...prev, plex_token: token }));

          // Busca automaticamente os servidores do usuário
          setLoadingServers(true);
          const servers = await plexService.getServers(token);
          setDiscoveredServers(servers);
          setLoadingServers(false);

          if (servers.length > 0) {
            const defaultUri = servers[0].chosen_uri;
            setSelectedServerUri(defaultUri);
            setConfig((prev) => ({ ...prev, plex_url: defaultUri }));
          }
        }
      } catch (err) {
        console.error("Erro ao checar status do PIN:", err);
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
      <div className="bg-[#161616] border border-[#2B2B2B] rounded-2xl w-full max-w-2xl min-h-[540px] flex flex-col shadow-2xl overflow-hidden animate-in fade-in duration-200">
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

          {/* ETAPA 3: GERENCIAR PASTAS LOCAIS */}
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

          {/* ETAPA 4: AUTENTICAÇÃO PLEX AUTOMATIZADA COM PIN */}
          {((step === 3 && !useLocal && usePlex) || (step === 4 && useLocal && usePlex)) && (
            <div className="space-y-4 animate-in fade-in duration-150">
              <div>
                <h2 className="text-xl font-bold text-white flex items-center space-x-2">
                  <Server size={20} className="text-[#E5A00D]" />
                  <span>Conectar ao Plex Media Server</span>
                </h2>
                <p className="text-xs text-[#999999] mt-0.5">
                  Autentique com a sua conta oficial do Plex para importar seus servidores automaticamente.
                </p>
              </div>

              {!config.plex_token ? (
                /* Estado 1: Aguardando ou iniciando autenticação */
                <div className="bg-[#121212] border border-[#262626] rounded-xl p-6 flex flex-col items-center justify-center text-center space-y-4">
                  {isPollingPin ? (
                    <div className="space-y-3 flex flex-col items-center">
                      <div className="flex items-center space-x-2 text-[#E5A00D]">
                        <RefreshCw size={22} className="animate-spin" />
                        <span className="text-sm font-bold">Aguardando autorização no navegador...</span>
                      </div>
                      {pinCode && (
                        <div className="bg-[#1C1810] border border-[#E5A00D]/40 px-5 py-2.5 rounded-xl">
                          <span className="text-xs text-[#888888] block">Código de Confirmação:</span>
                          <span className="text-2xl font-mono font-black text-[#E5A00D] tracking-widest">
                            {pinCode}
                          </span>
                        </div>
                      )}
                      <p className="text-[11px] text-[#777777] max-w-sm">
                        Uma aba foi aberta no seu navegador padrão. Confirme o acesso da sua conta para conectar o Sonante.
                      </p>
                    </div>
                  ) : (
                    <>
                      <div className="w-12 h-12 rounded-xl bg-[#E5A00D]/10 border border-[#E5A00D]/20 flex items-center justify-center text-[#E5A00D]">
                        <ExternalLink size={24} />
                      </div>
                      <div className="space-y-1 max-w-sm">
                        <h3 className="text-sm font-bold text-white">Login Rápido e Seguro</h3>
                        <p className="text-xs text-[#888888]">
                          Você será redirecionado para a página de autorização oficial do Plex sem expor suas credenciais.
                        </p>
                      </div>
                      <button
                        type="button"
                        onClick={handleStartPlexAuth}
                        className="flex items-center space-x-2 px-6 py-2.5 rounded-xl bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-lg transition-transform active:scale-95 cursor-pointer"
                      >
                        <Server size={16} />
                        <span>Entrar com a conta Plex</span>
                      </button>
                    </>
                  )}
                </div>
              ) : (
                /* Estado 2: Autenticado - Seleção de Servidor */
                <div className="space-y-3">
                  <div className="flex items-center justify-between p-3 rounded-xl bg-[#141F14] border border-[#4BB543]/40 text-xs">
                    <div className="flex items-center space-x-2 text-[#4BB543]">
                      <Check size={16} />
                      <span className="font-bold">Conta Plex conectada com sucesso!</span>
                    </div>
                    <button
                      type="button"
                      onClick={() => {
                        setConfig((prev) => ({ ...prev, plex_token: "", plex_url: "" }));
                        setDiscoveredServers([]);
                      }}
                      className="text-[#888888] hover:text-[#FF4D4D] text-[11px] underline cursor-pointer"
                    >
                      Desconectar
                    </button>
                  </div>

                  <div className="space-y-2">
                    <label className="block text-xs font-bold text-[#CCCCCC]">
                      Selecione o seu Servidor de Músicas:
                    </label>

                    {loadingServers ? (
                      <div className="p-4 text-center text-xs text-[#777777] flex items-center justify-center space-x-2">
                        <RefreshCw size={14} className="animate-spin text-[#E5A00D]" />
                        <span>Localizando servidores...</span>
                      </div>
                    ) : discoveredServers.length === 0 ? (
                      <div className="p-4 text-center text-xs text-[#888888] bg-[#121212] rounded-xl border border-[#242424]">
                        Nenhum servidor foi detectado nesta conta Plex.
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
                                  {localConn ? `${localConn.address}:${localConn.port} (Rede Local)` : srv.chosen_uri}
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

              {/* Opção Recolhível de Ajuste Manual */}
              <div className="pt-2 border-t border-[#222222]">
                <button
                  type="button"
                  onClick={() => setShowManualPlex(!showManualPlex)}
                  className="flex items-center space-x-1.5 text-[11px] text-[#777777] hover:text-[#CCCCCC] transition-colors cursor-pointer"
                >
                  {showManualPlex ? <ChevronUp size={13} /> : <ChevronDown size={13} />}
                  <span>Configuração Manual (Avançado)</span>
                </button>

                {showManualPlex && (
                  <div className="grid grid-cols-2 gap-3 pt-2.5 animate-in fade-in duration-100">
                    <div>
                      <label className="block text-[11px] text-[#999999] mb-1">URL do Servidor</label>
                      <input
                        type="text"
                        value={config.plex_url}
                        onChange={(e) => setConfig({ ...config, plex_url: e.target.value })}
                        placeholder="http://192.168.1.100:32400"
                        className="w-full bg-[#111111] border border-[#333333] rounded-lg px-3 py-1.5 text-xs text-white outline-none focus:border-[#E5A00D]"
                      />
                    </div>
                    <div>
                      <label className="block text-[11px] text-[#999999] mb-1">X-Plex-Token</label>
                      <input
                        type="password"
                        value={config.plex_token}
                        onChange={(e) => setConfig({ ...config, plex_token: e.target.value })}
                        placeholder="Token manual"
                        className="w-full bg-[#111111] border border-[#333333] rounded-lg px-3 py-1.5 text-xs text-white outline-none focus:border-[#E5A00D]"
                      />
                    </div>
                  </div>
                )}
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
                  <span className="text-white">
                    {config.plex_url ? "Conectado" : "Desativado"}
                  </span>
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

export default WelcomeWizard;
