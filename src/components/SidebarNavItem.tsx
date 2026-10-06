import React from "react";
import type { LucideIcon } from "lucide-react";

interface SidebarNavItemProps {
  icon: LucideIcon;
  label: string;
  active?: boolean;
  onClick: () => void;
  trailing?: React.ReactNode;
  title?: string;
}

export const SidebarNavItem: React.FC<SidebarNavItemProps> = ({
  icon: Icon,
  label,
  active = false,
  onClick,
  trailing,
  title,
}) => (
  <button
    type="button"
    onClick={onClick}
    aria-current={active ? "page" : undefined}
    title={title}
    className={`group relative flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left text-sm font-medium transition-colors cursor-pointer focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#E5A00D] ${
      active
        ? "bg-[#242424] text-white"
        : "text-[#8A8A8A] hover:bg-[#202020] hover:text-white"
    }`}
  >
    {active && <span aria-hidden="true" className="absolute inset-y-2 left-0 w-0.5 rounded-r bg-[#E5A00D]" />}
    <Icon
      size={17}
      aria-hidden="true"
      className={`shrink-0 transition-colors ${active ? "text-[#E5A00D]" : "text-[#777777] group-hover:text-[#BBBBBB]"}`}
    />
    <span className="min-w-0 flex-1 truncate">{label}</span>
    {trailing && <span className="shrink-0">{trailing}</span>}
  </button>
);
