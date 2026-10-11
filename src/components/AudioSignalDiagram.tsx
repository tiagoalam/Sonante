import { useId, useState } from "react";
import { ArrowDown, AudioLines, Cpu, Music2, Network, Speaker } from "lucide-react";
import { useTranslation } from "react-i18next";

/** Explains the draft route only. It does not measure the effective DAC output. */
export function AudioSignalDiagram({ mode, device }: { mode: "shared" | "direct"; device: string }) {
  const { t } = useTranslation();
  const titleId = useId();
  const explanationId = useId();
  const [selected, setSelected] = useState<string | null>(null);
  const steps = [
    { id: "source", icon: Music2, label: t("settings.signal.source"), description: t("settings.signal.sourceDescription") },
    { id: "mpd", icon: Cpu, label: t("settings.signal.mpd"), description: t("settings.signal.mpdDescription") },
    { id: "alsa", icon: AudioLines, label: t("settings.signal.alsa"), endpoint: mode === "shared" ? "default" : device,
      description: mode === "shared" ? t("settings.signal.alsaShared") : t("settings.signal.alsaDirect") },
    ...(mode === "shared" ? [{ id: "system", icon: Network, label: t("settings.signal.system"), description: t("settings.signal.systemDescription") }] : []),
    { id: "dac", icon: Speaker, label: t("settings.signal.dac"), description: t("settings.signal.dacDescription") },
  ];
  const active = steps.find(step => step.id === selected);
  return (
    <figure aria-labelledby={titleId} className="p-4 rounded-xl border border-[#343029] bg-[#141414] space-y-3">
      <figcaption id={titleId} className="text-white font-semibold">{t("settings.signal.title")}</figcaption>
      <p className="text-[11px] text-[#AAAAAA]">{t("settings.signal.previewLabel")}</p>
      <ol className="flex flex-col md:flex-row items-stretch gap-2 md:gap-1">
        {steps.map((step, index) => (
          <li key={step.id} className="flex flex-col md:flex-row items-center gap-2 md:gap-1 flex-1 min-w-0">
            {index > 0 && <ArrowDown size={16} aria-hidden="true" className="text-[#999999] shrink-0 md:-rotate-90" />}
            <button type="button" aria-pressed={active?.id === step.id}
              aria-describedby={active?.id === step.id ? explanationId : undefined}
              onClick={() => setSelected(step.id)}
              className={`w-full h-full flex flex-col items-center justify-center gap-2 rounded-lg border p-3 min-w-0 cursor-pointer outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D] ${active?.id === step.id
                ? "border-[#E5A00D] bg-[#221B0E]" : mode === "shared" ? "border-[#304350] bg-[#172027] hover:border-[#607787]" : "border-[#483A22] bg-[#211B12] hover:border-[#8D7446]"}`}>
              <step.icon size={24} aria-hidden="true" className={mode === "shared" ? "text-[#96BDCF]" : "text-[#E5A00D]"} />
              <span className="text-white text-[11px] font-semibold text-center">{step.label}</span>
              {step.endpoint !== undefined && <code className="text-[10px] text-[#AAAAAA] break-all">{step.endpoint}</code>}
            </button>
          </li>
        ))}
      </ol>
      <p id={explanationId} role="status" aria-live="polite" className="text-[11px] text-[#BBBBBB] leading-relaxed">
        {active ? active.description : t("settings.signal.inspect")}
      </p>
      <details className="text-[11px] text-[#999999] leading-relaxed space-y-2">
        <summary className="cursor-pointer text-[#BBBBBB]">{t("settings.signal.details")}</summary>
        <p>{t("settings.signal.notice")}</p>
        <p>{mode === "shared" ? t("settings.pipewireNotice") : t("settings.directNotice")}</p>
        <p>{t("settings.deviceAvailability.notice")}</p>
      </details>
    </figure>
  );
}
