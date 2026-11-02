import { useCallback, useEffect, useMemo, useState } from 'react';
import type { AppConfig } from '../types/config';
import type { EditionSource } from './albumEditions';
import type { LocalAlbumCatalogState } from './localAlbumCatalog';
import { localAlbumCatalog } from './localAlbumCatalogSession';
import { sourceEnabled } from './musicSources';
import { navidromeService } from '../services/navidrome';
import { localSourceReadiness, remoteSourceReadiness, type SourceReadiness } from './sourceReadiness';
interface Inputs {
  config: AppConfig | null; available: boolean; unavailable: boolean; updating: boolean; catalog: LocalAlbumCatalogState;
  plexIdentity: object; plexCurrent: boolean; plexLoading: boolean; plexError: boolean; plexAttempt: number; retryPlex: () => void;
  navConfigured: boolean; navRevision: number;
}
export function useSourceReadiness(input: Inputs) {
  const { config, available, unavailable, updating, catalog, plexIdentity, plexCurrent, plexLoading, plexError, plexAttempt, retryPlex, navConfigured, navRevision } = input;
  const local = sourceEnabled(config, 'local') && !config?.first_run;
  const plex = sourceEnabled(config, 'plex') && !config?.first_run;
  const nav = sourceEnabled(config, 'navidrome') && !config?.first_run;
  const [navAttempt, setNavAttempt] = useState(0);
  const navContext = useMemo(() => ({}), [nav, navConfigured, navRevision, navAttempt]);
  const plexContext = useMemo(() => ({}), [plexIdentity, plexAttempt]);
  const [probe, setProbe] = useState<{ context: object; phase: SourceReadiness } | null>(null);
  const [observed, setObserved] = useState<Partial<Record<EditionSource, { context: object; phase: 'ready' | 'error' }>>>({});
  useEffect(() => { if (local && config?.local_folders.length) localAlbumCatalog.ensureLoaded(updating, available); }, [local, config?.local_folders, updating, available, catalog.loaded, catalog.loading]);
  useEffect(() => {
    if (!nav || !navConfigured || !available) return;
    let current = true;
    setProbe({ context: navContext, phase: 'checking' });
    void (async () => {
      const connection = await navidromeService.status();
      if (!current) return;
      if (!connection.connection_id || !connection.account_id) { setProbe({ context: navContext, phase: 'error' }); return; }
      // One bounded page proves catalog availability, including an empty catalog.
      // It does not enumerate the collection or prepare tracks/playback.
      await navidromeService.albums(connection.connection_id, '', 'name', 0, 18);
      if (!current) return;
      const confirmed = await navidromeService.status();
      if (!current) return;
      setProbe({ context: navContext, phase: confirmed.connection_id === connection.connection_id && confirmed.account_id === connection.account_id ? 'ready' : 'error' });
    })().catch(() => { if (current) setProbe({ context: navContext, phase: 'error' }); });
    return () => { current = false; };
  }, [navContext, nav, navConfigured, available]);
  const statuses: Record<EditionSource, SourceReadiness> = {
    local: localSourceReadiness(local, !!config?.local_folders.length, available, unavailable, updating, catalog),
    plex: remoteSourceReadiness(plex, !!config?.plex_server_id && !!config?.plex_token, plexCurrent, plexLoading, plexError),
    navidrome: !nav ? 'disabled' : !navConfigured ? 'unconfigured' : probe?.context === navContext ? probe.phase : 'checking',
  };
  for (const source of ['plex', 'navidrome'] as const) {
    const context = source === 'plex' ? plexContext : navContext;
    if ((statuses[source] === 'ready' || statuses[source] === 'error') && observed[source]?.context === context) statuses[source] = observed[source]!.phase;
  }
  const report = useCallback((source: EditionSource, phase: 'ready' | 'error') => {
    if (source === 'local') return;
    const context = source === 'plex' ? plexContext : navContext;
    setObserved(previous => previous[source]?.context === context && previous[source]?.phase === phase ? previous : { ...previous, [source]: { context, phase } });
  }, [plexContext, navContext]);
  const retry = (source: EditionSource) => {
    if (!sourceEnabled(config, source)) return;
    if (source === 'local') { if (available && !updating && !catalog.saveBlocked && !catalog.mpdBusy) localAlbumCatalog.requestRefresh(); }
    else if (source === 'plex') retryPlex();
    else setNavAttempt(value => value + 1);
  };
  return { statuses, retry, report };
}
