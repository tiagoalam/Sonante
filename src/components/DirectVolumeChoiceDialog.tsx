import { useEffect, useId, useRef, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import {
  getDirectVolumeChoicePromptSnapshot,
  respondToDirectVolumeChoice,
  subscribeDirectVolumeChoicePrompt,
} from "../utils/directVolumeChoicePrompt";

export function DirectVolumeChoiceDialog() {
  const { t } = useTranslation();
  const id = useId();
  const dialog = useRef<HTMLDialogElement>(null);
  const state = useSyncExternalStore(subscribeDirectVolumeChoicePrompt, getDirectVolumeChoicePromptSnapshot, getDirectVolumeChoicePromptSnapshot);

  useEffect(() => {
    const element = dialog.current;
    if (!element) return;
    if (state.open && !element.open) element.showModal();
    else if (!state.open && element.open) element.close();
  }, [state.open]);
  useEffect(() => () => {
    if (getDirectVolumeChoicePromptSnapshot().open) respondToDirectVolumeChoice(false);
  }, []);

  return <dialog ref={dialog} aria-labelledby={`${id}-title`} aria-describedby={`${id}-description`}
    onCancel={event => { event.preventDefault(); respondToDirectVolumeChoice(false); }}
    className="m-auto w-[min(92vw,28rem)] rounded-2xl border border-[color:var(--theme-border-soft,#333333)] bg-[var(--theme-surface-panel,#1B1C1F)] p-5 text-[color:var(--theme-fg-primary,#ffffff)] shadow-2xl backdrop:bg-black/65">
    {state.open && <>
      <h2 id={`${id}-title`} className="text-base font-bold">{t("player.directVolumeChoiceTitle")}</h2>
      <p id={`${id}-description`} className="mt-3 text-sm leading-relaxed text-[color:var(--theme-fg-secondary,#BBBBBB)]">{t("player.directVolumeChoiceDescription")}</p>
      <p role="note" className="mt-3 rounded-lg border border-[color:var(--theme-border-accent,#6E5422)] bg-[var(--theme-surface-base,#121212)] p-3 text-xs leading-relaxed text-[color:var(--theme-fg-accent,#C9A45D)]">{t("player.directVolumeChoiceSafety")}</p>
      <div className="mt-5 flex justify-end gap-2">
        <button type="button" autoFocus onClick={() => respondToDirectVolumeChoice(false)}
          className="rounded-lg border border-[color:var(--theme-border-soft,#444444)] px-4 py-2 text-xs font-semibold hover:bg-[var(--theme-surface-hover,#292929)]">
          {t("player.directVolumeChoiceCancel")}
        </button>
        <button type="button" onClick={() => respondToDirectVolumeChoice(true)}
          className="rounded-lg bg-[var(--theme-control-accent,#E5A00D)] px-4 py-2 text-xs font-bold text-[color:var(--theme-control-on-accent,#000000)] hover:brightness-110">
          {t("player.directVolumeChoiceContinue")}
        </button>
      </div>
    </>}
  </dialog>;
}
