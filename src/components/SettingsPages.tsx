import React, { useId, useRef, useState } from "react";
import { HardDrive, Languages, Radio, Server } from "lucide-react";
import { useTranslation } from "react-i18next";

type Page = "general" | "audio" | "local" | "plex";

// All panels stay mounted: changing pages must not reset drafts or async work.
export function SettingsPages({ general, audio, local, plex }: Record<Page, React.ReactNode>) {
  const { t } = useTranslation();
  const [active, setActive] = useState<Page>("general");
  const id = useId();
  const tabs = useRef<(HTMLButtonElement | null)[]>([]);
  const scroll = useRef<HTMLDivElement>(null);
  const pages = [
    { id: "general" as const, label: t("settings.pages.general"), icon: Languages, content: general },
    { id: "audio" as const, label: t("settings.pages.audio"), icon: Radio, content: audio },
    { id: "local" as const, label: t("settings.pages.local"), icon: HardDrive, content: local },
    { id: "plex" as const, label: t("settings.pages.plex"), icon: Server, content: plex },
  ];
  function select(index: number, focus = false) {
    setActive(pages[index].id);
    if (scroll.current) scroll.current.scrollTop = 0;
    if (focus) tabs.current[index]?.focus();
  }
  return (
    <div className="flex flex-col sm:flex-row flex-1 min-h-0 overflow-hidden text-xs">
      <div role="tablist" aria-label={t("settings.pages.navigation")}
        className="shrink-0 flex flex-wrap sm:flex-col gap-1 p-3 sm:w-52 bg-[#141414] border-b sm:border-b-0 sm:border-r border-[#262626]">
        {pages.map((page, index) => (
          <button key={page.id} type="button" role="tab" id={`${id}-tab-${page.id}`}
            aria-selected={active === page.id} aria-controls={`${id}-panel-${page.id}`}
            tabIndex={active === page.id ? 0 : -1}
            ref={element => { tabs.current[index] = element; }}
            onClick={() => select(index)}
            onKeyDown={event => {
              let next: number;
              if (event.key === "Home") next = 0;
              else if (event.key === "End") next = pages.length - 1;
              else if (event.key === "ArrowRight" || event.key === "ArrowDown") next = (index + 1) % pages.length;
              else if (event.key === "ArrowLeft" || event.key === "ArrowUp") next = (index + pages.length - 1) % pages.length;
              else return;
              event.preventDefault();
              select(next, true);
            }}
            className={`flex items-center gap-2 px-3 py-3 rounded-lg text-left font-semibold cursor-pointer outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D] ${active === page.id
              ? "bg-[#221B0E] text-[#E5A00D]" : "text-[#999999] hover:bg-[#222222] hover:text-white"}`}>
            <page.icon size={16} aria-hidden="true" className="shrink-0" />
            {page.label}
          </button>
        ))}
      </div>
      <div ref={scroll} className="min-w-0 flex-1 overflow-y-auto p-4 sm:p-6">
        {pages.map(page => (
          <section key={page.id} role="tabpanel" hidden={active !== page.id}
            id={`${id}-panel-${page.id}`} aria-labelledby={`${id}-tab-${page.id}`}
            tabIndex={0} className="space-y-6 outline-none focus-visible:ring-2 focus-visible:ring-[#E5A00D] rounded-lg">
            <h3 className="text-lg font-bold text-white">{page.label}</h3>
            {page.content}
          </section>
        ))}
      </div>
    </div>
  );
}
