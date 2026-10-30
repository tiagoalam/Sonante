import React from "react";
import { traceInteractionAction, type InteractionAction } from "../utils/frontendDiagnostics";
import type { LucideIcon } from "lucide-react";

interface SidebarNavItemProps {
  icon: LucideIcon;
  label: string;
  active?: boolean;
  onClick: () => void;
  trailing?: React.ReactNode;
  title?: string;
  diagnosticAction?: InteractionAction;
}

export const SidebarNavItem: React.FC<SidebarNavItemProps> = ({
  icon: Icon,
  label,
  active = false,
  onClick,
  trailing,
  title,
  diagnosticAction,
}) => (
  <button
    type="button"
    onClick={() => traceInteractionAction(diagnosticAction, onClick)}
    aria-current={active ? "page" : undefined}
    title={title}
    className={`group relative flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left text-sm font-medium transition-colors cursor-pointer focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] ${
      active
        ? "bg-[var(--theme-surface-raised,#242424)] text-[color:var(--theme-fg-primary,#ffffff)]"
        : "text-[color:var(--theme-fg-muted,#8A8A8A)] hover:bg-[var(--theme-surface-raised,#202020)] hover:text-[color:var(--theme-fg-primary,#ffffff)]"
    }`}
  >
    {active && <span aria-hidden="true" className="absolute inset-y-2 left-0 w-0.5 rounded-r bg-[#E5A00D]" />}
    <Icon
      size={17}
      aria-hidden="true"
      className={`shrink-0 transition-colors ${active ? "text-[color:var(--theme-fg-accent,#E5A00D)]" : "text-[color:var(--theme-fg-muted,#777777)] group-hover:text-[color:var(--theme-fg-secondary,#BBBBBB)]"}`}
    />
    <span className="min-w-0 flex-1 truncate">{label}</span>
    {trailing && <span className="shrink-0">{trailing}</span>}
  </button>
);
