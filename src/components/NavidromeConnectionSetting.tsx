import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { navidromeService, navidromeError } from '../services/navidrome';
import type { NavidromeAuthMethod, NavidromeStatus } from '../types/navidrome';
import { isWindowsPreview } from '../utils/platform';

const control = 'rounded-lg border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-base,#121212)] px-3 py-2 text-sm disabled:opacity-50';

export function NavidromeConnectionSetting({ onBusyChange, onConfiguredChange }: { onBusyChange?: (busy: boolean) => void; onConfiguredChange?: (configured: boolean) => void }) {
  const { t } = useTranslation();
  const windowsPreview = isWindowsPreview();
  const [status, setStatus] = useState<NavidromeStatus | null>(null);
  const [server, setServer] = useState('');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [authMethod, setAuthMethod] = useState<NavidromeAuthMethod>('password');
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const alive = useRef(false);
  const pending = useRef(false);
  const notify = useRef(onBusyChange);
  notify.current = onBusyChange;
  const notifyConfigured = useRef(onConfiguredChange);
  notifyConfigured.current = onConfiguredChange;

  useEffect(() => {
    alive.current = true;
    let live = true;
    setLoading(true);
    void navidromeService.status().then(value => {
      if (!live) return;
      setStatus(value);
      setServer(value.server ?? '');
      setUsername(value.username ?? '');
      setAuthMethod(value.auth_method ?? 'password');
      notifyConfigured.current?.(!!value.server && (value.auth_method === 'api_key' || !!value.username));
      setError(null);
    }).catch(cause => {
      if (live) setError(navidromeError(cause));
    }).finally(() => {
      if (live) setLoading(false);
    });
    return () => {
      live = false;
      alive.current = false;
    };
  }, [retry]);

  async function save(disconnect = false) {
    if (pending.current || (disconnect && !window.confirm(t('navidrome.disconnectConfirm')))) return;
    pending.current = true;
    setBusy(true);
    setError(null);
    notify.current?.(true);
    try {
      const next = disconnect
        ? await navidromeService.disconnect()
        : await navidromeService.connect(server, username, password, authMethod);
      if (alive.current) {
        setStatus(next);
        setAuthMethod(next.auth_method ?? authMethod);
        setUsername(next.username ?? '');
        notifyConfigured.current?.(!!next.server && (next.auth_method === 'api_key' || !!next.username));
        setPassword('');
        if (disconnect) {
          setServer('');
          setUsername('');
          setAuthMethod('password');
        }
      }
    } catch (cause) {
      if (alive.current) setError(navidromeError(cause));
    } finally {
      pending.current = false;
      if (alive.current) setBusy(false);
      notify.current?.(false);
    }
  }

  const httpWithoutTls = server.trim().toLowerCase().startsWith('http://');
  const canConnect = !!server.trim() && !!password
    && (authMethod === 'api_key' || !!username.trim());
  const extensionSummary = status?.extensions?.slice(0, 6).map(extension =>
    `${extension.name}${extension.versions.length ? ` v${extension.versions.join('/')}` : ''}`,
  ).join(', ');
  const hasServerInformation = !!(status?.server_type || status?.api_version || status?.open_subsonic || extensionSummary);

  return <div className="space-y-3">
    {loading ? <p role="status">{t('navidrome.loading')}</p> : <>
      <p className="text-xs text-[color:var(--theme-fg-muted,#999)]">{t('navidrome.connectionHint')}</p>
      {status?.server && <p role="status" className="text-xs">{t('sources.connectionSaved')}</p>}
      {hasServerInformation && <p data-subsonic-server-info className="break-words text-xs text-[color:var(--theme-fg-muted,#999)]">
        {status?.server_type && <span>{t('navidrome.serverInfo', {
          name: status.server_type,
          version: status.server_version ? ` (${status.server_version})` : '',
        })}</span>}
        {status?.api_version && <span>{status.server_type ? ' · ' : ''}{t('navidrome.apiVersion', { version: status.api_version })}</span>}
        {status?.open_subsonic && <span>{status.server_type || status.api_version ? ' · ' : ''}{t('navidrome.openSubsonic')}</span>}
        {extensionSummary && <span>{status?.server_type || status?.api_version || status?.open_subsonic ? ' · ' : ''}{t('navidrome.detectedExtensions', { names: extensionSummary })}</span>}
      </p>}
      <label className="block space-y-1">
        <span>{t('navidrome.server')}</span>
        <input className={control + ' w-full'} type="url" autoComplete="url" value={server} disabled={busy}
          maxLength={2048} onChange={event => setServer(event.target.value)} />
      </label>
      <label className="block space-y-1">
        <span>{t('navidrome.authMethod')}</span>
        <select className={control + ' w-full'} value={authMethod} disabled={busy}
          onChange={event => setAuthMethod(event.target.value as NavidromeAuthMethod)}>
          <option value="password">{t('navidrome.passwordAuth')}</option>
          {!windowsPreview && <option value="api_key">{t('navidrome.apiKeyAuth')}</option>}
        </select>
      </label>
      <label className="block space-y-1">
        <span>{t(authMethod === 'password' ? 'navidrome.username' : 'navidrome.accountLabel')}</span>
        <input className={control + ' w-full'} type="text" autoComplete={authMethod === 'password' ? 'username' : 'off'}
          value={username} disabled={busy} maxLength={256} onChange={event => setUsername(event.target.value)} />
      </label>
      <label className="block space-y-1">
        <span>{t(authMethod === 'password' ? 'navidrome.password' : 'navidrome.apiKey')}</span>
        <input className={control + ' w-full'} type="password" autoComplete="current-password"
          value={password} disabled={busy} maxLength={authMethod === 'password' ? 4096 : 2047}
          onChange={event => setPassword(event.target.value)} />
      </label>
      {httpWithoutTls && <p role="alert" className="text-xs text-amber-300">{t('navidrome.httpWarning')}</p>}
      <p className="text-xs text-[color:var(--theme-fg-muted,#999)]">{t('navidrome.credentialsHint')}</p>
      <p className="text-xs text-[color:var(--theme-fg-muted,#999)]">{t('sources.connectionApplies')}</p>
      <div className="flex flex-wrap gap-2">
        <button type="button" className={control} disabled={busy || !canConnect} onClick={() => void save()}>
          {t(busy ? 'navidrome.connecting' : 'navidrome.connect')}
        </button>
        {status?.server && <button type="button" className={control} disabled={busy} onClick={() => void save(true)}>
          {t('navidrome.disconnect')}
        </button>}
      </div>
    </>}
    {error && <p role="alert">{t(`navidrome.errors.${error}`)} <button type="button" className={control}
      disabled={busy} onClick={() => setRetry(value => value + 1)}>{t('navidrome.retry')}</button></p>}
  </div>;
}
