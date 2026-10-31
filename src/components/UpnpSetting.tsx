import { useEffect, useRef, useState, useId } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Radio, LoaderCircle } from "lucide-react";
import { useTranslation } from "react-i18next";

interface ReceiverStatus { enabled: boolean; requested_enabled?: boolean; name: string; port: number | null; error: string | null }
export function UpnpSetting() {
  const { t } = useTranslation();
  const id = useId();
  const [status, setStatus] = useState<ReceiverStatus | null>(null);
  const [enabled, setEnabled] = useState(false);
  const [name, setName] = useState("Sonante");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const generation = useRef(0);
  const read = () => {
    const own = ++generation.current; setBusy(true); setError(false);
    void invoke<ReceiverStatus>("get_upnp_receiver").then(next => {
      if (own !== generation.current) return;
      setStatus(next); setEnabled(next.requested_enabled ?? next.enabled); setName(next.name); setError(Boolean(next.error));
    }).catch(() => { if (own === generation.current) setError(true); })
      .finally(() => { if (own === generation.current) setBusy(false); });
  };
  useEffect(() => { read(); return () => { generation.current++; }; }, []);
  const apply = () => {
    if (busy || !status || !name.trim()) return;
    const own = ++generation.current; setBusy(true); setError(false);
    void invoke<ReceiverStatus>("set_upnp_receiver", { enabled, name: name.trim() }).then(next => {
      if (own !== generation.current) return;
      setStatus(next); setEnabled(next.requested_enabled ?? next.enabled); setName(next.name); setError(Boolean(next.error));
    }).catch(() => { if (own === generation.current) setError(true); })
      .finally(() => { if (own === generation.current) setBusy(false); });
  };
  return <fieldset className="space-y-3 border-t border-[color:var(--theme-border-soft,#333333)] pt-4" aria-describedby={`${id}-help`}>
    <legend className="flex items-center gap-2 text-sm font-semibold"><Radio size={16}/>{t("upnp.title")}</legend>
    <p id={`${id}-help`} className="text-xs leading-relaxed text-[color:var(--theme-fg-muted,#888888)]">{t("upnp.help")}</p>
    <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={enabled} disabled={busy || !status} onChange={event => setEnabled(event.target.checked)} className="accent-[var(--theme-control-accent,#E5A00D)]"/>{t("upnp.enable")}</label>
    <label htmlFor={`${id}-name`} className="block text-xs">{t("upnp.name")}</label>
    <div className="flex flex-wrap items-center gap-2">
      <input id={`${id}-name`} value={name} maxLength={64} disabled={busy || !status} onChange={event => setName(event.target.value)} className="min-w-0 flex-1 rounded-lg border border-[color:var(--theme-border-soft,#333333)] bg-[var(--theme-surface-base,#121212)] px-3 py-2 text-sm"/>
      <button type="button" onClick={status ? apply : read} disabled={busy || Boolean(status && (!name.trim() || !error && enabled === (status.requested_enabled ?? status.enabled) && name.trim() === status.name))} className="rounded-lg border border-[color:var(--theme-border-accent,#E5A00D)] px-4 py-2 text-xs font-semibold text-[color:var(--theme-fg-accent,#E5A00D)] disabled:opacity-50">{busy ? <LoaderCircle size={15} className="animate-spin" aria-label={t("upnp.applying")}/> : t(status ? "upnp.apply" : "upnp.retry")}</button>
    </div>
    {error ? <p role="alert" className="text-xs text-[color:var(--theme-fg-error,#ff9999)]">{t("upnp.error")}</p>
      : status && <p role="status" className="text-xs text-[color:var(--theme-fg-muted,#888888)]">{t(status.enabled ? "upnp.active" : "upnp.inactive")}</p>}
  </fieldset>;
}
