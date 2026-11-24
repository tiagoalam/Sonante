import { useTranslation } from "react-i18next";

/** Presentation-only shortcut reference embedded in the About / Help dialog. */
export function KeyboardShortcutsHelp() {
  const { t } = useTranslation();
  const groups = [
    { title: t("shortcuts.navigation"), rows: [["Ctrl+K", t("shortcuts.search")], ["Ctrl+F", t("shortcuts.filter")], ["Esc", t("shortcuts.back")]] },
    { title: t("shortcuts.searchResults"), rows: [["↑ ↓", t("shortcuts.results")], ["Home / End", t("shortcuts.edges")], ["Enter", t("shortcuts.activate")]] },
    { title: t("shortcuts.playback"), rows: [[t("shortcuts.space"), t("shortcuts.playPause")], ["← / →", t("shortcuts.previousNext")], ["↑ / ↓", t("shortcuts.volume")], ["M", t("shortcuts.mute")]] },
  ];

  return (
    <div className="space-y-5">
      <h3 className="text-base font-semibold">{t("shortcuts.title")}</h3>
      {groups.map(group => (
        <section key={group.title} className="space-y-2">
          <h4 className="text-sm font-semibold text-[color:var(--theme-fg-accent,#E5A00D)]">{group.title}</h4>
          <dl className="space-y-2">
            {group.rows.map(([keys, action]) => (
              <div key={action} className="flex items-start justify-between gap-5 text-sm">
                <dt className="text-[color:var(--theme-fg-secondary,#bbbbbb)]">{action}</dt>
                <dd className="shrink-0">
                  <kbd className="rounded border border-[color:var(--theme-border-soft,#444444)] bg-[var(--theme-surface-raised,#282828)] px-2 py-0.5 text-xs">{keys}</kbd>
                </dd>
              </div>
            ))}
          </dl>
        </section>
      ))}
      <p className="text-xs leading-relaxed text-[color:var(--theme-fg-muted,#999999)]">{t("shortcuts.contextHint")}</p>
    </div>
  );
}
