import type { AppConfig, EnabledSources, MusicSource } from '../types/config';
export const MUSIC_SOURCES: readonly MusicSource[] = ['local', 'plex', 'navidrome'];
type SourceConfig = Pick<AppConfig, 'sources'>;
// Missing preferences preserve pre-switch installations, including existing Navidrome accounts.
export function sourceEnabled(config: SourceConfig | null | undefined, source: MusicSource): boolean {
  return !!config && (config.sources?.[source] ?? true);
}
export function enabledSources(config: SourceConfig): EnabledSources {
  return { local: sourceEnabled(config, 'local'), plex: sourceEnabled(config, 'plex'), navidrome: sourceEnabled(config, 'navidrome') };
}
export function withSourceEnabled(config: AppConfig, source: MusicSource, enabled: boolean): AppConfig {
  return { ...config, sources: { ...enabledSources(config), [source]: enabled } };
}

export function sourceLocatorEnabled(config: Pick<AppConfig,'sources'>, locator: {kind: string} | null | undefined): boolean {
  return !locator || !MUSIC_SOURCES.includes(locator.kind as MusicSource) || sourceEnabled(config, locator.kind as MusicSource);
}
