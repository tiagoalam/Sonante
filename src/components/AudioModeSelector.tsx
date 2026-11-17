import { Check, Headphones, Radio } from "lucide-react";
import { useTranslation } from "react-i18next";

export type AudioExperienceMode = "shared" | "direct" | "equalized";
export type AudioOutputMode = "shared" | "direct";

export function AudioModeSelector({
  selected,
  onSelect,
  disabled = false,
  pendingSave = false,
}: {
  selected: AudioExperienceMode;
  onSelect: (mode: AudioOutputMode) => void;
  disabled?: boolean;
  pendingSave?: boolean;
}) {
  const { t } = useTranslation();
  const selectedOutput: AudioOutputMode = selected === "direct" ? "direct" : "shared";
  const options = [
    { mode: "shared" as const, icon: Radio },
    { mode: "direct" as const, icon: Headphones },
  ];

  return (
    <div data-audio-mode-selector role="group" aria-label={t("audioExperience.modeLabel")}
      className="grid grid-cols-1 gap-2.5 sm:grid-cols-2">
      {options.map(({ mode, icon: Icon }) => {
        const active = selectedOutput === mode;
        return (
          <button key={mode} type="button" data-audio-mode={mode} aria-pressed={active}
            data-pending-save={active && pendingSave ? "true" : undefined}
            disabled={disabled} onClick={() => onSelect(mode)}
            className={`group flex min-h-[76px] items-start gap-3 rounded-xl border px-4 py-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] disabled:cursor-not-allowed disabled:opacity-55 ${active
              ? "border-[color:var(--theme-border-accent,#E5A00D)] bg-[var(--theme-surface-accent,#221B0E)] text-[color:var(--theme-fg-primary,#ffffff)]"
              : "border-[color:var(--theme-border-soft,#2A2A2A)] bg-[var(--theme-surface-base,#141414)] text-[color:var(--theme-fg-secondary,#AAAAAA)] hover:border-[color:var(--theme-border-soft,#444444)] hover:text-[color:var(--theme-fg-primary,#ffffff)]"}`}>
            <Icon size={18} aria-hidden="true" className={`mt-0.5 shrink-0 ${active ? "text-[color:var(--theme-fg-accent,#E5A00D)]" : "text-[color:var(--theme-fg-muted,#888888)]"}`} />
            <span className="min-w-0 flex-1">
              <span className="flex items-center gap-2 text-sm font-semibold">
                {t(`audioExperience.mode.${mode}`)}
                {active && <span aria-label={t("audioExperience.selected")} className="ml-auto inline-flex h-5 w-5 items-center justify-center rounded-full bg-[var(--theme-surface-raised,#302410)] text-[color:var(--theme-fg-accent,#E5A00D)]"><Check size={12} aria-hidden="true" /></span>}
              </span>
              <span className="mt-1 block text-xs leading-relaxed text-[color:var(--theme-fg-muted,#888888)]">{t(`audioExperience.description.${mode}`)}</span>
            </span>
          </button>
        );
      })}
    </div>
  );
}
