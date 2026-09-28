import React from "react";
import {
  X,
  Sparkles,
  ShieldCheck,
  Cpu,
  Layers,
  Keyboard,
  HardDrive,
  Radio,
  Github,
  Heart,
} from "lucide-react";

interface Props {
  onClose: () => void;
}

export const AboutModal: React.FC<Props> = ({ onClose }) => {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/75 backdrop-blur-xs select-none p-4">
      <div className="bg-[#181818] border border-[#2B2B2B] rounded-2xl w-full max-w-xl flex flex-col shadow-2xl overflow-hidden animate-in fade-in zoom-in-95 duration-150">
        {/* Cabeçalho */}
        <div className="px-6 py-5 border-b border-[#262626] flex items-center justify-between bg-gradient-to-r from-[#1E1A14] to-[#181818]">
          <div className="flex items-center space-x-3">
            <div className="w-10 h-10 rounded-xl bg-[#E5A00D]/10 border border-[#E5A00D]/30 flex items-center justify-center text-[#E5A00D]">
              <Sparkles size={22} />
            </div>
            <div>
              <div className="flex items-center space-x-2">
                <h2 className="text-lg font-black tracking-wider text-white">SONANTE</h2>
                <span className="text-[10px] font-mono font-bold bg-[#E5A00D] text-black px-1.5 py-0.5 rounded-sm">
                  v0.3.0
                </span>
              </div>
              <p className="text-xs text-[#888888] mt-0.5">
                Player de Áudio Hi-Res & DSD Bit-Perfect
              </p>
            </div>
          </div>

          <button
            onClick={onClose}
            className="p-1.5 text-[#888888] hover:text-white rounded-lg hover:bg-[#252525] transition-colors cursor-pointer"
          >
            <X size={18} />
          </button>
        </div>

        {/* Corpo com Scroll */}
        <div className="p-6 overflow-y-auto space-y-6 max-h-[75vh] text-xs">
          {/* Proposta de Valor */}
          <div className="bg-[#141414] border border-[#242424] p-4 rounded-xl space-y-2">
            <div className="flex items-center space-x-2 text-[#E5A00D] font-bold">
              <ShieldCheck size={16} />
              <span>Pureza Sonora sem Intermediários</span>
            </div>
            <p className="text-[#A0A0A0] leading-relaxed">
              O Sonante foi projetado para entregar uma experiência de reprodução crítica e sem colorações.
              Comunicação direta com o subsistema ALSA via descritor de dispositivo exclusivo (<code className="text-[#E5A00D]">hw:</code>),
              isolando totalmente o fluxo de áudio de mixers compartilhados, conversões de taxa de amostragem ou controle de ganho destrutivo.
            </p>
          </div>

          {/* Destaques Técnicos */}
          <div>
            <h3 className="text-[11px] font-bold text-[#888888] uppercase tracking-wider mb-3">
              Arquitetura de Alta Fidelidade
            </h3>
            <div className="grid grid-cols-2 gap-2.5">
              <div className="bg-[#141414] border border-[#222222] p-3 rounded-lg flex items-start space-x-3">
                <Cpu size={16} className="text-[#E5A00D] shrink-0 mt-0.5" />
                <div>
                  <span className="font-bold text-white block">Motor MPD Dedicado</span>
                  <span className="text-[#666666] text-[11px]">
                    Instância isolada sob socket UNIX local com resposta em sub-milissegundos.
                  </span>
                </div>
              </div>

              <div className="bg-[#141414] border border-[#222222] p-3 rounded-lg flex items-start space-x-3">
                <Radio size={16} className="text-[#E5A00D] shrink-0 mt-0.5" />
                <div>
                  <span className="font-bold text-white block">DSD over PCM (DoP)</span>
                  <span className="text-[#666666] text-[11px]">
                    Encapsulamento nativo de DSD64/128 sem conversões PCM intermediárias.
                  </span>
                </div>
              </div>

              <div className="bg-[#141414] border border-[#222222] p-3 rounded-lg flex items-start space-x-3">
                <Layers size={16} className="text-[#E5A00D] shrink-0 mt-0.5" />
                <div>
                  <span className="font-bold text-white block">Buffer em Memória RAM</span>
                  <span className="text-[#666666] text-[11px]">
                    Pré-carregamento ajustável até 32 MB para imunidade contra jitter de rede.
                  </span>
                </div>
              </div>

              <div className="bg-[#141414] border border-[#222222] p-3 rounded-lg flex items-start space-x-3">
                <HardDrive size={16} className="text-[#E5A00D] shrink-0 mt-0.5" />
                <div>
                  <span className="font-bold text-white block">Plex & Disco Local</span>
                  <span className="text-[#666666] text-[11px]">
                    Streaming direto sem transcodificação ou leitura nativa do SSD/NFS.
                  </span>
                </div>
              </div>
            </div>
          </div>

          {/* Tabela de Atalhos de Teclado */}
          <div>
            <h3 className="text-[11px] font-bold text-[#888888] uppercase tracking-wider mb-3 flex items-center space-x-1.5">
              <Keyboard size={14} />
              <span>Atalhos Rápidos de Teclado</span>
            </h3>

            <div className="bg-[#141414] border border-[#222222] rounded-xl overflow-hidden divide-y divide-[#1F1F1F]">
              <div className="grid grid-cols-2 px-3.5 py-2 items-center">
                <span className="text-[#A0A0A0]">Reproduzir / Pausar</span>
                <span className="text-right">
                  <kbd className="bg-[#242424] border border-[#333333] text-white px-2 py-0.5 rounded text-[10px] font-mono shadow-xs">
                    Espaço
                  </kbd>
                </span>
              </div>

              <div className="grid grid-cols-2 px-3.5 py-2 items-center">
                <span className="text-[#A0A0A0]">Próxima / Faixa Anterior</span>
                <span className="text-right space-x-1">
                  <kbd className="bg-[#242424] border border-[#333333] text-white px-2 py-0.5 rounded text-[10px] font-mono shadow-xs">
                    ←
                  </kbd>
                  <kbd className="bg-[#242424] border border-[#333333] text-white px-2 py-0.5 rounded text-[10px] font-mono shadow-xs">
                    →
                  </kbd>
                </span>
              </div>

              <div className="grid grid-cols-2 px-3.5 py-2 items-center">
                <span className="text-[#A0A0A0]">Ajuste de Volume (+/- 5%)</span>
                <span className="text-right space-x-1">
                  <kbd className="bg-[#242424] border border-[#333333] text-white px-2 py-0.5 rounded text-[10px] font-mono shadow-xs">
                    ↑
                  </kbd>
                  <kbd className="bg-[#242424] border border-[#333333] text-white px-2 py-0.5 rounded text-[10px] font-mono shadow-xs">
                    ↓
                  </kbd>
                </span>
              </div>

              <div className="grid grid-cols-2 px-3.5 py-2 items-center">
                <span className="text-[#A0A0A0]">Mutar / Desmutar</span>
                <span className="text-right">
                  <kbd className="bg-[#242424] border border-[#333333] text-white px-2 py-0.5 rounded text-[10px] font-mono shadow-xs">
                    M
                  </kbd>
                </span>
              </div>

              <div className="grid grid-cols-2 px-3.5 py-2 items-center">
                <span className="text-[#A0A0A0]">Focar na Barra de Busca</span>
                <span className="text-right">
                  <kbd className="bg-[#242424] border border-[#333333] text-white px-2 py-0.5 rounded text-[10px] font-mono shadow-xs">
                    Ctrl + F
                  </kbd>
                </span>
              </div>

              <div className="grid grid-cols-2 px-3.5 py-2 items-center">
                <span className="text-[#A0A0A0]">Fechar Modais / Gavetas</span>
                <span className="text-right">
                  <kbd className="bg-[#242424] border border-[#333333] text-white px-2 py-0.5 rounded text-[10px] font-mono shadow-xs">
                    Esc
                  </kbd>
                </span>
              </div>
            </div>
          </div>
        </div>

        {/* Rodapé */}
        <div className="px-6 py-4 border-t border-[#262626] bg-[#141414] flex items-center justify-between text-[#666666] text-[11px]">
          <div className="flex items-center space-x-1.5">
            <span>Desenvolvido com</span>
            <Heart size={12} className="text-[#E5A00D] fill-[#E5A00D]" />
            <span>em Rust + Tauri v2</span>
          </div>

          <button
            onClick={onClose}
            className="px-4 py-1.5 rounded-lg bg-[#242424] hover:bg-[#2E2E2E] text-white font-semibold transition-colors cursor-pointer"
          >
            Fechar
          </button>
        </div>
      </div>
    </div>
  );
};
