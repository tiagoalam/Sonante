import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink, LoaderCircle, Music2, RefreshCw } from "lucide-react";
import { scrobblingService, scrobblingError, type ScrobblingAction, type ScrobblingProvider, type ScrobblingStatus } from "../services/scrobbling";

const fieldClass = "w-full min-w-0 rounded-lg border border-[color:var(--theme-border-soft,#333333)] bg-[var(--theme-surface-base,#121212)] px-3 py-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]";
const buttonClass = "rounded-lg border border-[color:var(--theme-border-soft,#333333)] px-3 py-2 text-xs font-semibold cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed hover:bg-[var(--theme-surface-raised,#252525)] focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]";
export function ScrobblingSetting() {
  const { t, i18n } = useTranslation(); const id = useId();
  const [status, setStatus] = useState<ScrobblingStatus | null>(null);
  const [busy, setBusy] = useState(false); const [error, setError] = useState<string | null>(null);
  const [apiKey, setApiKey] = useState(""); const [apiSecret, setApiSecret] = useState(""); const [token, setToken] = useState("");
  const generation = useRef(0); const active = useRef(false); const eventRevision = useRef(0); const acceptedRevision = useRef(-1);
  const accept = (next: ScrobblingStatus) => { if (next.revision < acceptedRevision.current) return false; acceptedRevision.current = next.revision; setStatus(next); return true; };
  useEffect(() => {
    const own = ++generation.current; let cleanup: (() => void) | undefined;
    // Subscribe before reading; a late read cannot replace a newer notification.
    void scrobblingService.subscribe(next => { if (generation.current === own) { eventRevision.current++; accept(next); } }).then(unlisten => {
      if (generation.current !== own) { unlisten(); return; } cleanup = unlisten;
      const revision = eventRevision.current;
      void scrobblingService.status().then(next => { if (generation.current === own && revision === eventRevision.current) accept(next); })
        .catch(() => { if (generation.current === own) setError("unavailable"); });
    }).catch(() => { if (generation.current === own) setError("unavailable"); });
    return () => { generation.current++; cleanup?.(); };
  }, []);
  const run = async (action: ScrobblingAction) => {
    if (active.current) return; active.current = true; const own = generation.current;
    setBusy(true); setError(null);
    try {
      const result = await scrobblingService.action(action);
      if (generation.current !== own) return;
      if (!accept(result.status)) return; setApiKey(""); setApiSecret(""); setToken("");
      if (result.authorization_url) {
        const url = new URL(result.authorization_url);
        if (url.origin !== "https://www.last.fm" || url.pathname !== "/api/auth/") throw Error("Invalid authorization destination");
        try { await openUrl(result.authorization_url); } catch { if (generation.current === own) setError("browser"); }
      }
    } catch (failure) {
      if (generation.current === own) {
        const code = scrobblingError(failure);
        const connecting = ["lastfm_begin", "lastfm_finish", "listenbrainz_connect"].includes(action.kind);
        setError(connecting && code === "protocol" ? "connection_protocol" : code);
      }
    }
    finally { active.current = false; if (generation.current === own) setBusy(false); }
  };
  const refresh = async () => {
    if (active.current) return; active.current = true; const own = generation.current; setBusy(true); setError(null);
    try { const revision = eventRevision.current; const next = await scrobblingService.status(); if (generation.current === own && revision === eventRevision.current) accept(next); }
    catch { if (generation.current === own) setError("unavailable"); }
    finally { active.current = false; if (generation.current === own) setBusy(false); }
  };
  const link = (url: string) => { const own = generation.current; void openUrl(url).catch(() => { if (generation.current === own) setError("browser_link"); }); };
  const disabled = busy || !status?.ready || status.storage_error;
  const service = (provider: ScrobblingProvider) => {
    const current = status?.[provider]; const lastfm = provider === "lastfm";
    return <section key={provider} className="min-w-0 space-y-3 rounded-xl border border-[color:var(--theme-border-soft,#333333)] p-4" aria-labelledby={`${id}-${provider}`}>
      <div className="flex flex-wrap items-center justify-between gap-2"><h4 id={`${id}-${provider}`} className="font-semibold text-sm">{lastfm ? "Last.fm" : "ListenBrainz"}</h4>
        <button type="button" className="inline-flex items-center gap-1 text-xs underline text-[color:var(--theme-fg-muted,#aaaaaa)]" onClick={() => link(lastfm ? "https://www.last.fm/api/account/create" : "https://listenbrainz.org/settings/")}>{t(lastfm ? "scrobbling.appCredentials" : "scrobbling.getToken")}<ExternalLink size={12}/></button>
      </div>
      {current?.connected && <>
        <p className="text-xs break-words">{t("scrobbling.connected", { user: current.username })}</p>
        <label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={current.enabled} disabled={disabled} className="accent-[var(--theme-control-accent,#E5A00D)]" onChange={event => void run({ kind: "set_enabled", provider, enabled: event.target.checked })}/>{t("scrobbling.enable")}</label>
        <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-[color:var(--theme-fg-muted,#aaaaaa)]" role="status"><span>{t("scrobbling.pending", { count: current.queued })}</span>
          {current.last_sent && <span>{t("scrobbling.lastSent", { time: new Date(current.last_sent * 1000).toLocaleTimeString(i18n.language, { hour: "2-digit", minute: "2-digit" }) })}</span>}
        </div>
      </>}
      {lastfm ? status?.lastfm_authorizing ? <div className="space-y-2">
        <p className="text-xs leading-relaxed">{t("scrobbling.authorizeHelp")}</p>
        <div className="flex flex-wrap gap-2"><button type="button" className={buttonClass} disabled={disabled} onClick={() => void run({ kind: "lastfm_finish" })}>{t("scrobbling.authorized")}</button><button type="button" className={buttonClass} disabled={disabled} onClick={() => void run({ kind: "lastfm_cancel" })}>{t("scrobbling.cancel")}</button></div>
      </div> : <>
        {!current?.connected && <>
          <p className="text-xs leading-relaxed text-[color:var(--theme-fg-muted,#aaaaaa)]">{t("scrobbling.lastfmHelp")}</p>
          <label htmlFor={`${id}-key`} className="block text-xs">{t("scrobbling.apiKey")}</label><input id={`${id}-key`} type="password" autoComplete="off" spellCheck={false} maxLength={32} className={fieldClass} value={apiKey} disabled={disabled} onChange={e => setApiKey(e.target.value)}/>
          <label htmlFor={`${id}-secret`} className="block text-xs">{t("scrobbling.apiSecret")}</label><input id={`${id}-secret`} type="password" autoComplete="new-password" spellCheck={false} maxLength={32} className={fieldClass} value={apiSecret} disabled={disabled} onChange={e => setApiSecret(e.target.value)}/>
        </>}
        <button type="button" className={buttonClass} disabled={disabled || (!current?.connected && (!/^[a-f\d]{32}$/i.test(apiKey.trim()) || !/^[a-f\d]{32}$/i.test(apiSecret.trim())))} onClick={() => void run({ kind: "lastfm_begin", api_key: apiKey, api_secret: apiSecret })}>{t(current?.connected ? "scrobbling.reauthorize" : "scrobbling.authorize")}</button>
      </> : <>
        <label htmlFor={`${id}-token`} className="block text-xs">{t("scrobbling.userToken")}</label><input id={`${id}-token`} type="password" autoComplete="new-password" spellCheck={false} maxLength={128} className={fieldClass} value={token} disabled={disabled} onChange={e => setToken(e.target.value)}/>
        <button type="button" className={buttonClass} disabled={disabled || token.trim().length < 8} onClick={() => void run({ kind: "listenbrainz_connect", token })}>{t(current?.connected ? "scrobbling.updateToken" : "scrobbling.connect")}</button>
      </>}
      {current?.connected && <button type="button" className={`${buttonClass} ml-2`} disabled={disabled} onClick={() => void run({ kind: "disconnect", provider })}>{t("scrobbling.disconnect")}</button>}
      {current?.error && <p role="status" className="text-xs leading-relaxed text-[color:var(--theme-fg-accent,#E5A00D)]">{t(`scrobbling.errors.${scrobblingError(current.error)}`)}</p>}
    </section>;
  };
  return <fieldset className="space-y-3 border-t border-[color:var(--theme-border-soft,#333333)] pt-4" aria-describedby={`${id}-help`}>
    <legend className="flex items-center gap-2 text-sm font-semibold"><Music2 size={16}/>{t("scrobbling.title")}</legend>
    <p id={`${id}-help`} className="text-xs leading-relaxed text-[color:var(--theme-fg-muted,#aaaaaa)]">{t("scrobbling.help")}</p>
    <p className="text-xs leading-relaxed text-[color:var(--theme-fg-muted,#aaaaaa)]">{t("scrobbling.threshold")}</p>
    {(!status?.ready || busy) && !error && <p role="status" className="flex items-center gap-2 text-xs"><LoaderCircle size={14} className="animate-spin"/>{t(busy ? "scrobbling.working" : "scrobbling.loading")}</p>}
    {status?.storage_error && <p role="alert" className="text-xs text-[color:var(--theme-fg-error,#ff9999)]">{t("scrobbling.errors.storage_unavailable")}</p>}
    {error && <p role="alert" className="text-xs text-[color:var(--theme-fg-error,#ff9999)]">{t(`scrobbling.errors.${error}`)}</p>}
    <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">{service("lastfm")}{service("listenbrainz")}</div>
    <div className="flex flex-wrap items-center gap-3"><button type="button" className={`${buttonClass} inline-flex items-center gap-1.5`} disabled={busy} onClick={() => void refresh()}><RefreshCw size={12}/>{t("scrobbling.refresh")}</button><span className="text-xs text-[color:var(--theme-fg-muted,#aaaaaa)]">{t("scrobbling.savedImmediately")}</span></div>
  </fieldset>;
}
