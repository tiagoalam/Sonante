import React from "react";
import { Folder, Server } from "lucide-react";

export type LibrarySource = "local" | "plex";
interface Props {
  source: LibrarySource;
  onChange: (source: LibrarySource) => void;
  idPrefix: string;
  label: string;
  labels: Record<LibrarySource, string>;
}

export const LibrarySourceTabs: React.FC<Props> = ({ source, onChange, idPrefix, label, labels }) => {
  const sources = ["local", "plex"] as const;
  return <div className="shrink-0 border-b border-[color:var(--theme-border-soft,#262626)] px-6 py-4 sm:px-8">
    <div data-source-navigation role="tablist" aria-label={label}
      className="grid w-full max-w-lg grid-cols-2 gap-2 rounded-xl border border-[color:var(--theme-border-soft,#333333)] bg-[var(--theme-surface-panel,#181818)] p-1.5">
    {sources.map((item, index) => {
      const Icon = item === "local" ? Folder : Server;
      return <button key={item} id={`${idPrefix}-${item}`} type="button" role="tab"
        aria-selected={source === item} aria-controls={`${idPrefix}-panel`} tabIndex={source === item ? 0 : -1}
        onClick={() => { if (source !== item) onChange(item); }}
        onKeyDown={event => {
          let target: LibrarySource | undefined;
          if (event.key === "ArrowRight" || event.key === "ArrowLeft") target = sources[1 - index];
          else if (event.key === "Home") target = "local";
          else if (event.key === "End") target = "plex";
          if (!target) return;
          event.preventDefault(); event.stopPropagation();
          const button = event.currentTarget.parentElement?.querySelector<HTMLButtonElement>(`#${idPrefix}-${target}`);
          button?.focus();
          if (target !== source) onChange(target);
        }}
        className={`flex min-w-0 items-center justify-center gap-3 rounded-lg border px-5 py-3 text-base font-semibold transition-colors cursor-pointer focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] ${source === item
          ? "border-[color:var(--theme-border-accent,#E5A00D)] bg-[#E5A00D] text-black shadow"
          : "border-[color:var(--theme-border-soft,#333333)] text-[color:var(--theme-fg-secondary,#AAAAAA)] hover:bg-[var(--theme-surface-raised,#262626)] hover:text-[color:var(--theme-fg-primary,#ffffff)]"}`}>
        <Icon size={20} aria-hidden="true" />{labels[item]}
      </button>;
    })}
    </div>
  </div>;
};
