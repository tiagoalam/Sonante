import { fullModeEnabled } from './usageMode.ts';
import type { AppConfig } from '../types/config';
import type { PlaybackStatus } from '../types/audio';
export const experimentalFeaturesEnabled = (config: Pick<AppConfig, 'experimental_features' | 'usage_mode'> | null | undefined): boolean => config?.experimental_features === true && fullModeEnabled(config);
export const isExperimentalSection = (section: unknown): boolean => section === 'sessions' || section === 'editions';
type ProgressModule = Pick<typeof import('./listeningSessionProgress'), 'observeSessionProgress' | 'suspendSessionProgress'>;
// The ordinary player imports this small controller, not the session engine or
// store. Disabling invalidates pending imports and stops progress work only.
export function createExperimentalSessionObserver(load: () => Promise<ProgressModule>) {
  let enabled = false, revision = 0, failed = false;
  let loaded: ProgressModule | null = null, pending: Promise<ProgressModule> | null = null;
  return {
    async update(next: boolean, status: PlaybackStatus) {
      if (next !== enabled) { enabled = next; failed = false; }
      const own = ++revision;
      if (!enabled) { loaded?.suspendSessionProgress(); return; }
      if (failed) return;
      try {
        const module = loaded ?? await (pending ??= load()); loaded = module;
        if (own === revision && enabled) await module.observeSessionProgress(status);
        else if (!enabled) module.suspendSessionProgress();
      } catch { pending = null; if (own === revision) failed = true; }
    },
    dispose() { enabled = false; revision++; loaded?.suspendSessionProgress(); },
  };
}
