import { useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { createPortal } from "react-dom";
import { MoreHorizontal, ListEnd, ListStart, Disc3 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { audioService } from "../services/audio";
import { beginPlaybackIntent, isPlaybackSuperseded, playbackIntentToken, type PlaybackIntent } from "../utils/playbackIntent";
import { logError } from "../utils/frontendDiagnostics";
import type { QueueMenuRequest } from "../utils/queueActions";

type Action = string;
interface Target { request: QueueMenuRequest; x: number; y: number; origin: HTMLElement; visit: object }
function QueueMenu({ target, close, choose }: { target: Target; close: () => void; choose: (action: Action) => void }) {
  const { t } = useTranslation();
  const menu = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ x: target.x, y: target.y });
  useEffect(() => {
    const element = menu.current;
    if (!element) return;
    const bounds = element.getBoundingClientRect();
    setPosition({ x: Math.max(8, Math.min(target.x, window.innerWidth - bounds.width - 8)), y: Math.max(8, Math.min(target.y, window.innerHeight - bounds.height - 8)) });
    element.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
    const scroll = (event: Event) => { if (!element.contains(event.target as Node)) close(); };
    window.addEventListener("resize", close); window.addEventListener("scroll", scroll, true);
    return () => { window.removeEventListener("resize", close); window.removeEventListener("scroll", scroll, true); if (target.origin.isConnected) target.origin.focus({ preventScroll: true }); };
  }, [target]);
  const choices: { action: string; label: string; Icon: typeof ListStart; disabled?: boolean }[] = [{ action: "next" as const, label: t("queueActions.next"), Icon: ListStart }, { action: "end" as const, label: t("queueActions.end"), Icon: ListEnd }, { action: "album" as const, label: t("queueActions.album"), Icon: Disc3 }];
  if (target.request.scope) choices.pop();
  for (const action of target.request.actions ?? []) choices.push({ action: action.id, label: action.label, Icon: ListStart, disabled: action.disabled });
  return createPortal(<div data-queue-actions-menu className="fixed inset-0 z-[80]" onMouseDown={event => { event.stopPropagation(); if (event.target === event.currentTarget) { event.preventDefault(); close(); } }} onClick={event => event.stopPropagation()} onContextMenu={event => { event.preventDefault(); event.stopPropagation(); close(); }}>
    <div ref={menu} role="menu" aria-label={t("queueActions.title")} style={{ position: "fixed", left: position.x, top: position.y }}
      className="w-72 max-w-[calc(100vw-16px)] rounded-xl border border-[color:var(--theme-border-soft,#444444)] bg-[var(--theme-surface-panel,#181818)] p-1.5 text-[color:var(--theme-fg-primary,#e0e0e0)] shadow-2xl"
      onKeyDown={event => {
        event.stopPropagation();
        if (event.key === "Escape" || event.key === "Tab") { if (event.key === "Escape") event.preventDefault(); close(); return; }
        const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
        const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
        const next = event.key === "ArrowDown" ? (index + 1) % buttons.length : event.key === "ArrowUp" ? (index + buttons.length - 1) % buttons.length
          : event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : null;
        if (next !== null) { event.preventDefault(); buttons[next]?.focus(); }
      }}>
      <p className="truncate px-3 py-2 text-xs text-[color:var(--theme-fg-muted,#999999)]" title={target.request.track.title}>{target.request.track.title}</p>
      {choices.map(({ action, label, Icon, disabled }) => <button key={action} role="menuitem" type="button" disabled={disabled || (action === "album" && !target.request.album)}
        title={action === "album" && !target.request.album ? t("queueActions.albumUnavailable") : undefined}
        onClick={event => { event.stopPropagation(); choose(action); }}
        className="flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left text-sm hover:bg-[var(--theme-surface-raised,#282828)] focus:bg-[var(--theme-surface-accent,#332b15)] focus:outline focus:outline-[color:var(--theme-border-accent,#e5a00d)] disabled:opacity-40"><Icon size={16} aria-hidden="true" />{label}</button>)}
    </div>
  </div>, document.body);
}

export function useQueueInsertion(context: unknown, available: boolean) {
  const { t } = useTranslation();
  const visit = useMemo(() => ({}), [context, available]);
  const live = useRef<object | null>(visit); live.current = visit;
  const owner = useRef<{ visit: object; intent: PlaybackIntent } | null>(null);
  const [target, setTarget] = useState<Target | null>(null);
  const [result, setResult] = useState<{ visit: object; count?: number; error?: "failed" | "uncertain" | "persistence" | "receiver" } | null>(null);
  const [busy, setBusy] = useState<object | null>(null);
  useEffect(() => {
    live.current = visit;
    return () => { if (live.current === visit) live.current = null; if (owner.current?.visit === visit) { owner.current.intent.cancel(); owner.current = null; } };
  }, [visit]);
  const currentTarget = target?.visit === visit && available ? target : null;
  const close = () => setTarget(null);
  const open = (event: MouseEvent<HTMLElement>, request: QueueMenuRequest) => {
    event.preventDefault(); event.stopPropagation();
    if (!available || busy === visit || request.current?.() === false) return;
    const source = event.currentTarget;
    const origin = source.querySelector<HTMLElement>("[data-queue-trigger]") ?? source;
    const bounds = origin.getBoundingClientRect();
    setTarget({ request, origin, visit, x: event.clientX || bounds.left, y: event.clientY || bounds.bottom + 4 });
  };
  const perform = async (action: Action) => {
    if (!currentTarget || busy === visit || owner.current?.visit === visit) return;
    const request = currentTarget.request;
    if (action === "album" && !request.album) return;
    close(); setResult(null); setBusy(visit);
    const intent = beginPlaybackIntent(); owner.current = { visit, intent };
    const current = () => live.current === visit && intent.current() && request.current?.() !== false;
    try {
      await playbackIntentToken(intent);
      if (!current()) return;
      const custom = request.actions?.find(item => item.id === action);
      if (custom) { if (!custom.disabled) await custom.run(intent); return; }
      const tracks = action === "album" ? await request.album!() : request.load ? await request.load() : [request.track];
      if (!current()) return;
      const count = await audioService.enqueueTracks(tracks, action === "next" ? "next" : "end", intent);
      if (await intent.confirmCurrent() && current()) setResult({ visit, count });
    } catch (error) {
      if (!isPlaybackSuperseded(error) && await intent.confirmCurrent() && current()) {
        logError("player.request", error);
        setResult({ visit, error: error === "QUEUE_INSERT_UPNP_ACTIVE" ? "receiver" : error === "PLAYBACK_QUEUE_UNCERTAIN" ? "uncertain" : error === "PLAYBACK_PERSISTENCE_FAILED" ? "persistence" : "failed" });
      }
    } finally { if (owner.current?.intent === intent) owner.current = null; if (live.current === visit) setBusy(null); }
  };
  const shown = result?.visit === visit ? result : null;
  return {
    open,
    trigger(request: QueueMenuRequest) {
      return <button type="button" data-queue-trigger aria-haspopup="menu" disabled={!available || busy === visit}
        aria-label={t("queueActions.forTrack", { title: request.track.title })} title={t("queueActions.title")}
        onClick={event => open(event, request)} onContextMenu={event => open(event, request)} onKeyDown={event => event.stopPropagation()}
        className="shrink-0 rounded-md p-1.5 text-[color:var(--theme-fg-muted,#999999)] hover:bg-[var(--theme-surface-hover,#333333)] hover:text-[color:var(--theme-fg-accent,#e5a00d)] focus-visible:outline focus-visible:outline-[color:var(--theme-border-accent,#e5a00d)] disabled:opacity-40"><MoreHorizontal size={16} /></button>;
    },
    menu: currentTarget ? <QueueMenu key="queue-menu" target={currentTarget} close={close} choose={action => void perform(action)} /> : null,
    notice: shown ? <p role={shown.error ? "alert" : "status"} className="mx-4 my-2 rounded-lg bg-[var(--theme-surface-subtle,#222222)] px-3 py-2 text-xs text-[color:var(--theme-fg-secondary,#bbbbbb)]">{shown.error === "receiver" ? t("queueActions.receiver") : shown.error === "uncertain" ? t("queueActions.uncertain") : shown.error === "persistence" ? t("queueActions.persistence") : shown.error ? t("queueActions.failed") : t("queueActions.added", { count: shown.count })}</p> : null,
  };
}
