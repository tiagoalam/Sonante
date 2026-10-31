import { useEffect, useId, useRef, useState } from "react";
import { Keyboard, X } from "lucide-react";
import { useTranslation } from "react-i18next";

/** Presentation only. Native modal owns keyboard events; it never sends audio commands. */
export function KeyboardShortcutsHelp() {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const dialog = useRef<HTMLDialogElement>(null), trigger = useRef<HTMLButtonElement>(null);
  const title = useId();
  useEffect(() => {
    if (!open || !dialog.current) return;
    const element = dialog.current;
    element.showModal();
    return () => { if (element.open) element.close(); if (trigger.current?.isConnected) trigger.current.focus({ preventScroll: true }); };
  }, [open]);
  const groups = [
    { title: t("shortcuts.navigation"), rows: [["Ctrl+K", t("shortcuts.search")], ["Ctrl+F", t("shortcuts.filter")], ["Esc", t("shortcuts.back")]] },
    { title: t("shortcuts.searchResults"), rows: [["↑ ↓", t("shortcuts.results")], ["Home / End", t("shortcuts.edges")], ["Enter", t("shortcuts.activate")]] },
    { title: t("shortcuts.playback"), rows: [[t("shortcuts.space"), t("shortcuts.playPause")], ["← / →", t("shortcuts.previousNext")], ["↑ / ↓", t("shortcuts.volume")], ["M", t("shortcuts.mute")]] },
  ];
  return <>
    <button ref={trigger} type="button" aria-haspopup="dialog" aria-expanded={open}
      onClick={event => { event.stopPropagation(); setOpen(true); }}
      onKeyDown={event => { if (event.key === " " || event.key === "Enter") event.stopPropagation(); }}
      className="flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left text-sm font-medium text-[color:var(--theme-fg-muted,#8A8A8A)] hover:bg-[var(--theme-surface-raised,#202020)] hover:text-[color:var(--theme-fg-primary,#ffffff)] focus-visible:outline focus-visible:outline-[color:var(--theme-border-accent,#E5A00D)]">
      <Keyboard size={17} aria-hidden="true" /><span>{t("shortcuts.title")}</span>
    </button>
    {open && <dialog ref={dialog} aria-labelledby={title} aria-modal="true" tabIndex={-1}
      onKeyDown={event => event.stopPropagation()} onCancel={event => { event.preventDefault(); setOpen(false); }} onClose={() => setOpen(false)}
      className="m-auto max-h-[85dvh] w-11/12 max-w-lg overflow-y-auto rounded-2xl border border-[color:var(--theme-border-soft,#333333)] bg-[var(--theme-surface-panel,#181818)] p-5 text-[color:var(--theme-fg-primary,#e0e0e0)] shadow-2xl backdrop:bg-black/75">
      <header className="mb-5 flex items-center justify-between gap-4"><h2 id={title} className="text-lg font-semibold">{t("shortcuts.title")}</h2>
        <button autoFocus type="button" onClick={() => setOpen(false)} aria-label={t("shortcuts.close")} className="rounded-lg p-2 hover:bg-[var(--theme-surface-raised,#282828)] focus-visible:outline focus-visible:outline-[color:var(--theme-border-accent,#E5A00D)]"><X size={20} /></button>
      </header>
      {groups.map(group => <section key={group.title} className="mb-5"><h3 className="mb-2 text-sm font-semibold text-[color:var(--theme-fg-accent,#E5A00D)]">{group.title}</h3>
        <dl className="space-y-2">{group.rows.map(([keys, action]) => <div key={action} className="flex items-start justify-between gap-5 text-sm">
          <dt className="text-[color:var(--theme-fg-secondary,#bbbbbb)]">{action}</dt>
          <dd className="shrink-0"><kbd className="rounded border border-[color:var(--theme-border-soft,#444444)] bg-[var(--theme-surface-raised,#282828)] px-2 py-0.5 text-xs">{keys}</kbd></dd>
        </div>)}</dl>
      </section>)}
      <p className="text-xs leading-relaxed text-[color:var(--theme-fg-muted,#999999)]">{t("shortcuts.contextHint")}</p>
    </dialog>}
  </>;
}
