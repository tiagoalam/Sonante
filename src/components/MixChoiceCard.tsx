import { CalendarDays, Music2 } from "lucide-react";

const colors = ["#754347", "#385C50", "#4E4A79", "#6C552F", "#3F5770", "#714A64"];
export function mixChoiceColor(identity: string) {
  let hash = 0;
  for (const character of identity) hash = (Math.imul(hash, 31) + character.codePointAt(0)!) >>> 0;
  return colors[hash % colors.length];
}

export function MixChoiceCard({ identity, label, subtitle, kind, disabled, onClick, ...data }: {
  identity: string; label: string; subtitle?: string; kind: "decade" | "style";
  disabled: boolean; onClick: () => void; "data-decade"?: number; "data-style"?: string;
}) {
  const Icon = kind === "decade" ? CalendarDays : Music2;
  return <button type="button" {...data} disabled={disabled} onClick={onClick}
    style={{ backgroundColor: mixChoiceColor(identity), backgroundImage: "linear-gradient(135deg, rgba(255,255,255,.07), transparent 75%)" }}
    className="flex min-h-36 min-w-0 flex-col items-start gap-4 rounded-2xl border border-white/10 p-5 text-left text-white transition hover:-translate-y-0.5 hover:brightness-110 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white focus-visible:ring-offset-2 focus-visible:ring-offset-[#121212] disabled:opacity-50 disabled:hover:translate-y-0">
    <Icon size={26} aria-hidden="true" className="shrink-0 text-white/80" />
    <span className="min-w-0 text-xl font-bold leading-snug [overflow-wrap:anywhere]">{label}</span>
    {subtitle && <span className="-mt-2 text-xs font-medium text-white/80">{subtitle}</span>}
  </button>;
}
