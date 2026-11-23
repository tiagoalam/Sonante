import React from "react";
import { traceInteractionAction, type InteractionAction } from "../utils/frontendDiagnostics";
import type { LucideIcon } from "lucide-react";

interface SidebarNavItemProps {
  icon: LucideIcon;
  label: string;
  active?: boolean;
  // Source readiness is not the same as whether this navigation action is disabled.
  sourceEnabled?: boolean;
  disabled?: boolean;
  onClick: () => void;
  trailing?: React.ReactNode;
  title?: string;
  diagnosticAction?: InteractionAction;
}

export const SidebarNavItem: React.FC<SidebarNavItemProps> = ({
  icon: Icon,
  label,
  active = false,
  sourceEnabled = true,
  disabled = false,
  onClick,
  trailing,
  title,
  diagnosticAction,
}) => (
  <button
    type="button"
    disabled={disabled}
    aria-disabled={disabled || undefined}
    onClick={() => traceInteractionAction(diagnosticAction, onClick)}
    aria-current={active ? "page" : undefined}
    title={title}
    className={`group relative flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-left text-[13px] font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)] ${
      disabled
        ? "cursor-not-allowed opacity-55 disabled:hover:bg-transparent"
        : sourceEnabled
          ? "cursor-pointer text-[color:var(--theme-fg-primary,#ffffff)] hover:bg-[var(--theme-surface-raised,#202020)] hover:text-[color:var(--theme-fg-primary,#ffffff)]"
          : "cursor-pointer text-[color:var(--theme-fg-muted,#8b8b8b)] hover:bg-[var(--theme-surface-raised,#202020)] hover:text-[color:var(--theme-fg-muted,#8b8b8b)]"
    } ${
      active
        ? "bg-[var(--theme-surface-raised,#242424)] font-semibold"
        : ""
    }`}
  >
    {active && <span aria-hidden="true" className="absolute inset-y-2 left-0 w-0.5 rounded-r bg-[var(--theme-fg-accent,#E5A00D)]" />}
    <Icon
      size={17}
      aria-hidden="true"
      className={`shrink-0 transition-colors ${active && sourceEnabled ? "text-[color:var(--theme-fg-accent,#E5A00D)]" : sourceEnabled ? "text-[color:var(--theme-fg-primary,#ffffff)]" : "text-[color:var(--theme-fg-muted,#8b8b8b)]"}`}
    />
    <span className="min-w-0 flex-1 truncate">{label}</span>
    {trailing && <span className="shrink-0">{trailing}</span>}
  </button>
);
