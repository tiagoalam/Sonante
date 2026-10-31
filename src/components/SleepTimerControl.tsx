import React, { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { Clock3, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { sleepTimer } from "../services/sleepTimer";
import { remainingSleepMinutes } from "../utils/sleepTimer";
import "./sleepTimer.css";
export const SleepTimerControl: React.FC<{ compact?: boolean }> = ({ compact = false }) => {
  const { t } = useTranslation();
  const view = useSyncExternalStore(sleepTimer.subscribe, sleepTimer.getSnapshot, sleepTimer.getSnapshot);
  const [now, setNow] = useState(() => performance.now());
  const menu = useRef<HTMLDetailsElement>(null);
  const active = view.timer?.phase === "armed";
  const failed = view.error || view.timer?.phase === "failed";
  useEffect(() => {
    const visible = () => { if (!document.hidden) { setNow(performance.now()); void sleepTimer.refresh(); } };
    const outside = (event: PointerEvent) => { if (menu.current?.open && !menu.current.contains(event.target as Node)) menu.current.open = false; };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("visibilitychange", visible);
    let interval: number | undefined;
    if (active) interval = window.setInterval(() => { if (!document.hidden) setNow(performance.now()); }, 1000);
    return () => { window.clearInterval(interval); document.removeEventListener("visibilitychange", visible); document.removeEventListener("pointerdown", outside); };
  }, [active]);
  const change = (minutes: 15 | 30 | 60 | null) => { if (menu.current) menu.current.open = false; void sleepTimer.change(minutes); };
  const minutes = remainingSleepMinutes(view, Math.max(now, view.receivedAt));
  return <div className={`sleep-timer${compact ? " sleep-timer-compact" : ""}${active ? " sleep-timer-active" : ""}`}>
    <details ref={menu} onKeyDown={event => { if (event.key === "Escape" && menu.current?.open) { event.preventDefault(); event.stopPropagation(); menu.current.open = false; menu.current.querySelector("summary")?.focus(); } }}>
      <summary title={t("sleepTimer.title")} aria-label={active ? t("sleepTimer.remaining", { count: minutes }) : t("sleepTimer.title")}><Clock3 size={17} />{active && <span>{minutes > 0 ? `${minutes} min` : t("sleepTimer.stopping")}</span>}{failed && <span className="sleep-timer-dot" aria-label={t("sleepTimer.error")} />}</summary>
      <div className="sleep-timer-menu"><strong>{t("sleepTimer.title")}</strong><p>{t("sleepTimer.help")}</p>
        {([15, 30, 60] as const).map(minutes => <button type="button" key={minutes} disabled={view.changing || !view.timer} onClick={() => change(minutes)}>{t("sleepTimer.option", { count: minutes })}</button>)}
        {(active || failed) && <button type="button" disabled={view.changing} onClick={() => change(null)}>{t(active ? "sleepTimer.cancel" : "sleepTimer.dismiss")}</button>}
        {failed && <p role="alert">{t("sleepTimer.error")}</p>}
      </div>
    </details>
    {active && <button type="button" className="sleep-timer-cancel" disabled={view.changing} onClick={() => change(null)} aria-label={t("sleepTimer.cancel")} title={t("sleepTimer.cancel")}><X size={13} /></button>}
  </div>;
};
