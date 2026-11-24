import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";

export function SourceHeader({
  icon: Icon,
  title,
  description,
  meta,
  accentClassName = "text-[color:var(--theme-fg-accent,#E5A00D)]",
}: {
  icon: LucideIcon;
  title: string;
  description: string;
  meta?: ReactNode;
  accentClassName?: string;
}) {
  return <div className="flex min-w-0 items-start gap-3">
    <Icon size={26} className={`mt-0.5 shrink-0 ${accentClassName}`} aria-hidden="true" />
    <div className="min-w-0">
      <h1 className="break-words text-2xl font-bold leading-tight text-[color:var(--theme-fg-primary,#ffffff)]">{title}</h1>
      <p className="mt-2 max-w-3xl break-words text-xs leading-relaxed text-[color:var(--theme-fg-muted,#999)]">{description}</p>
      {meta && <div className="mt-1 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-[11px] leading-relaxed text-[color:var(--theme-fg-secondary,#aaa)]">{meta}</div>}
    </div>
  </div>;
}
