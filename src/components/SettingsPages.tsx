import React, { useEffect, useId, useRef, useState } from "react";
import { FlaskConical, Languages, Network, Palette, Radio, Server } from "lucide-react";
import { useTranslation } from "react-i18next";
import "./settings.css";

export type SettingsPage = "interface" | "appearance" | "audio" | "sources" | "network" | "experimental" | "local" | "plex" | "navidrome";

interface SettingsPagesProps {
  interfacePage: React.ReactNode;
  appearance: React.ReactNode;
  audio: React.ReactNode;
  sources: React.ReactNode;
  network: React.ReactNode;
  experimental: React.ReactNode;
  initialPage?: SettingsPage;
  readOnly?: boolean;
  disabledPages?: SettingsPage[];
}

// All panels stay mounted: changing pages must not reset drafts or async work.
export function SettingsPages({
  initialPage = "interface", interfacePage, appearance, audio, sources, network, experimental,
  readOnly = false, disabledPages = [],
}: SettingsPagesProps) {
  const { t } = useTranslation();
  const requestedPage = (["local", "plex", "navidrome"].includes(initialPage) ? "sources" : initialPage) as SettingsPage;
  const firstEnabledPage = (["interface", "appearance", "audio", "sources", "network", "experimental"] as SettingsPage[])
    .find(page => !disabledPages.includes(page)) ?? "interface";
  // Consume the destination once on mount; polling must not override user navigation.
  const [active, setActive] = useState<SettingsPage>(disabledPages.includes(requestedPage) ? firstEnabledPage : requestedPage);
  const activePage = disabledPages.includes(active) ? firstEnabledPage : active;
  const disabledPageSignature = disabledPages.join("|");
  useEffect(() => {
    if (disabledPages.includes(active)) setActive(firstEnabledPage);
  }, [active, disabledPageSignature, firstEnabledPage]);
  const id = useId();
  const tabs = useRef<(HTMLButtonElement | null)[]>([]);
  const scroll = useRef<HTMLDivElement>(null);
  const pages = [
    { id: "interface" as const, label: t("settings.pages.interface"), icon: Languages, content: interfacePage },
    { id: "appearance" as const, label: t("settings.pages.appearance"), icon: Palette, content: appearance },
    { id: "audio" as const, label: t("settings.pages.audio"), icon: Radio, content: audio },
    { id: "sources" as const, label: t("settings.pages.sources"), icon: Server, content: sources },
    { id: "network" as const, label: t("settings.pages.network"), icon: Network, content: network },
    { id: "experimental" as const, label: t("settings.pages.experimental"), icon: FlaskConical, content: experimental },
  ];
  function select(index: number, focus = false) {
    if (disabledPages.includes(pages[index].id)) return;
    setActive(pages[index].id);
    if (scroll.current) scroll.current.scrollTop = 0;
    if (focus) tabs.current[index]?.focus();
  }
  function nextEnabled(index: number, direction: number) {
    let next = index;
    for (let attempts = 0; attempts < pages.length; attempts++) {
      next = (next + direction + pages.length) % pages.length;
      if (!disabledPages.includes(pages[next].id)) return next;
    }
    return index;
  }
  return (
    <div className="flex flex-col sm:flex-row flex-1 min-h-0 overflow-hidden text-xs">
      <div role="tablist" aria-label={t("settings.pages.navigation")}
        className="settings-pages-nav shrink-0 flex flex-nowrap sm:flex-col gap-1 p-2 sm:p-3 overflow-x-auto sm:overflow-x-visible sm:w-52 bg-[var(--theme-surface-base,#141414)] border-b sm:border-b-0 sm:border-r border-[color:var(--theme-border-soft,#262626)]">
        {pages.map((page, index) => (
          <button key={page.id} type="button" role="tab" id={`${id}-tab-${page.id}`}
            aria-selected={activePage === page.id} aria-controls={`${id}-panel-${page.id}`}
            disabled={disabledPages.includes(page.id)} aria-disabled={disabledPages.includes(page.id) || undefined}
            tabIndex={activePage === page.id ? 0 : -1}
            ref={element => { tabs.current[index] = element; }}
            onClick={() => select(index)}
            onKeyDown={event => {
              let next: number;
              if (event.key === "Home") next = pages.findIndex(page => !disabledPages.includes(page.id));
              else if (event.key === "End") next = Math.max(0, pages.length - 1 - pages.slice().reverse().findIndex(page => !disabledPages.includes(page.id)));
              else if (event.key === "ArrowRight" || event.key === "ArrowDown") next = nextEnabled(index, 1);
              else if (event.key === "ArrowLeft" || event.key === "ArrowUp") next = nextEnabled(index, -1);
              else return;
              event.preventDefault();
              select(next, true);
            }}
            className={`flex shrink-0 items-center gap-2 px-3 py-3 rounded-lg text-left font-semibold whitespace-nowrap cursor-pointer outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] disabled:cursor-not-allowed disabled:opacity-45 ${activePage === page.id
              ? "bg-[var(--theme-surface-accent,#221B0E)] text-[color:var(--theme-fg-accent,#E5A00D)]" : "text-[color:var(--theme-fg-secondary,#999999)] hover:bg-[var(--theme-surface-raised,#222222)] hover:text-[color:var(--theme-fg-primary,#ffffff)]"}`}>
            <page.icon size={16} aria-hidden="true" className="shrink-0" />
            {page.label}
          </button>
        ))}
      </div>
      <div ref={scroll} className="min-w-0 flex-1 overflow-y-auto p-4 sm:p-6">
        {pages.map(page => (
          <section key={page.id} role="tabpanel" hidden={activePage !== page.id}
            id={`${id}-panel-${page.id}`} aria-labelledby={`${id}-tab-${page.id}`}
            tabIndex={0} className="space-y-6 outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] rounded-lg">
            <h3 className="text-lg font-bold text-[color:var(--theme-fg-primary,#ffffff)]">{page.label}</h3>
            <div inert={readOnly || disabledPages.includes(page.id) || undefined} aria-disabled={readOnly || disabledPages.includes(page.id) || undefined} className="settings-page-content">{page.content}</div>
          </section>
        ))}
      </div>
    </div>
  );
}
