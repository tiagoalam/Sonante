import type { AppConfig } from '../types/config';
export type UsageMode = 'full' | 'simple';
// Missing preferences retain the complete experience of existing installations.
export const fullModeEnabled = (config: Pick<AppConfig, 'usage_mode'> | null | undefined): boolean => config?.usage_mode !== 'simple';
export const isAdvancedSection = (section: unknown): boolean => ['explore', 'equalizer', 'sessions', 'editions'].includes(String(section));
