import { frontendStartup } from '../utils/frontendStartup';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { localDiagnosticErrors } from '../utils/frontendDiagnostics';
export function LocalDiagnostics() {
    const alive = useRef(true);
    useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
    const { t } = useTranslation();
    const [report, setReport] = useState<string | null>(null);
    const [copied, setCopied] = useState(false);
    const [failed, setFailed] = useState(false);
    const [busy, setBusy] = useState(false);
    const pending = useRef(false);
    async function copy() { if (pending.current)
        return; pending.current = true; setBusy(true); setFailed(false); setCopied(false); try {
        const native = await invoke('get_local_diagnostics');
        if (!alive.current)
            return;
        const text = JSON.stringify({ application: 'Sonante', native, frontend_startup: frontendStartup.snapshot(), frontend: localDiagnosticErrors() }, null, 2);
        setReport(text);
        try {
            await navigator.clipboard.writeText(text);
            if (alive.current)
                setCopied(true);
        }
        catch { /* The visible read-only report remains selectable when clipboard is unavailable. */ }
    }
    catch {
        if (alive.current)
            setFailed(true);
    }
    finally {
        pending.current = false;
        if (alive.current)
            setBusy(false);
    } }
    return <div className="space-y-2 text-xs"><button type="button" disabled={busy} className="rounded-lg border border-[color:var(--theme-border-soft,#333)] px-3 py-2 disabled:opacity-50" onClick={() => void copy()}>{t(busy ? 'diagnostic.preparing' : 'diagnostic.copy')}</button><p className="text-[color:var(--theme-fg-muted,#999)]">{t('diagnostic.hint')}</p>{copied && <p role="status">{t('diagnostic.copied')}</p>}{failed && <p role="alert">{t('diagnostic.failed')}</p>}{report && !copied && <label className="block space-y-1"><span>{t('diagnostic.select')}</span><textarea readOnly rows={7} value={report} onFocus={event => event.currentTarget.select()} className="w-full rounded border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-base,#121212)] p-2 font-mono text-xs"/></label>}</div>;
}
