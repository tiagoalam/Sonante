import type { LocalAlbumCatalogState } from './localAlbumCatalog';
export type SourceReadiness = 'disabled' | 'unconfigured' | 'checking' | 'busy' | 'ready' | 'error';
// Presentation of an observed catalog boundary, never native read admission.
export function localSourceReadiness(enabled: boolean, configured: boolean, available: boolean, unavailable: boolean, updating: boolean, catalog: LocalAlbumCatalogState): SourceReadiness {
  if (!enabled) return 'disabled';
  if (!configured) return 'unconfigured';
  if (unavailable) return 'error';
  if (!available) return 'checking';
  if (updating || catalog.loading || catalog.mpdBusy || catalog.saveBlocked || catalog.manualCheckPlayback) return 'busy';
  if (catalog.error) return 'error';
  return catalog.loaded ? 'ready' : 'checking';
}
export function remoteSourceReadiness(enabled: boolean, configured: boolean, current: boolean, loading: boolean, failed: boolean): SourceReadiness {
  if (!enabled) return 'disabled';
  if (!configured) return 'unconfigured';
  if (!current || loading) return 'checking';
  return failed ? 'error' : 'ready';
}
