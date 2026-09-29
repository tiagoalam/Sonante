import React from "react";
import { X, Sparkles, Cpu, HardDrive, Server, ShieldCheck, Heart } from "lucide-react";
import { useTranslation } from "react-i18next";

interface Props {
  onClose: () => void;
}

export const AboutModal: React.FC<Props> = ({ onClose }) => {
  const { t } = useTranslation();

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/75 backdrop-blur-xs select-none p-4">
      <div className="bg-[#181818] border border-[#2B2B2B] rounded-2xl w-full max-w-lg shadow-2xl overflow-hidden animate-in fade-in duration-150">
        {/* Cabeçalho */}
        <div className="p-6 border-b border-[#262626] flex items-center justify-between bg-[#1D1D1D]">
          <div className="flex items-center space-x-3">
            <div className="w-10 h-10 rounded-xl bg-[#E5A00D]/10 border border-[#E5A00D]/30 flex items-center justify-center text-[#E5A00D]">
              <Sparkles size={22} />
            </div>
            <div>
              <div className="flex items-center space-x-2">
                <h2 className="text-lg font-black tracking-wider text-[#E5A00D]">SONANTE</h2>
                <span className="text-[10px] font-mono font-bold bg-[#E5A00D] text-black px-1.5 py-0.5 rounded-sm">
                  v0.3.8
                </span>
              </div>
              <p className="text-[11px] text-[#888888]">{t("about.tagline")}</p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="p-1.5 text-[#888888] hover:text-white rounded-lg hover:bg-[#252525] transition-colors cursor-pointer"
          >
            <X size={18} />
          </button>
        </div>

        {/* Conteúdo */}
        <div className="p-6 space-y-4 text-xs">
          <div className="space-y-3">
            <h3 className="text-[11px] font-bold text-[#CCCCCC] uppercase tracking-wider">
              {t("about.techTitle")}
            </h3>

            <div className="grid grid-cols-1 gap-2.5">
              <div className="p-3 rounded-xl bg-[#131313] border border-[#222222] flex items-start space-x-3">
                <ShieldCheck size={18} className="text-[#E5A00D] shrink-0 mt-0.5" />
                <div>
                  <span className="font-bold text-white block">{t("about.audioEngineTitle")}</span>
                  <span className="text-[#888888] text-[11px] leading-relaxed">
                    {t("about.audioEngineDesc")}
                  </span>
                </div>
              </div>

              <div className="p-3 rounded-xl bg-[#131313] border border-[#222222] flex items-start space-x-3">
                <HardDrive size={18} className="text-[#E5A00D] shrink-0 mt-0.5" />
                <div>
                  <span className="font-bold text-white block">{t("about.localTitle")}</span>
                  <span className="text-[#888888] text-[11px] leading-relaxed">
                    {t("about.localDesc")}
                  </span>
                </div>
              </div>

              <div className="p-3 rounded-xl bg-[#131313] border border-[#222222] flex items-start space-x-3">
                <Server size={18} className="text-[#E5A00D] shrink-0 mt-0.5" />
                <div>
                  <span className="font-bold text-white block">{t("about.plexTitle")}</span>
                  <span className="text-[#888888] text-[11px] leading-relaxed">
                    {t("about.plexDesc")}
                  </span>
                </div>
              </div>

              <div className="p-3 rounded-xl bg-[#131313] border border-[#222222] flex items-start space-x-3">
                <Cpu size={18} className="text-[#E5A00D] shrink-0 mt-0.5" />
                <div>
                  <span className="font-bold text-white block">{t("about.devTitle")}</span>
                  <span className="text-[#888888] text-[11px] leading-relaxed">
                    {t("about.devDesc")}
                  </span>
                </div>
              </div>
            </div>
          </div>
        </div>

        {/* Rodapé */}
        <div className="px-6 py-4 border-t border-[#262626] bg-[#161616] flex items-center justify-between text-[11px] text-[#666666]">
          <div className="flex items-center space-x-1">
            <span>{t("about.builtWith")}</span>
            <Heart size={12} className="text-[#E5A00D] fill-[#E5A00D]" />
            <span>{t("about.forEnthusiasts")}</span>
          </div>
          <span className="font-mono">Linux ALSA Exclusive</span>
        </div>
      </div>
    </div>
  );
};

export default AboutModal;
