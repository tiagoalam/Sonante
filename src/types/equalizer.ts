export type EqFilterType = "peak" | "low_shelf" | "high_shelf";
export interface EqBand { id: string; enabled: boolean; filter_type: EqFilterType; frequency_hz: number; gain_db: number; q: number }
export interface EqPreset { id: string; name: string; preamp_db: number; bands: EqBand[] }
export type PresetRef = { kind: "built_in" | "user"; id: string };
export interface EquipmentProfile { id:string; name:string; preset:PresetRef }
export interface EqualizerConfig { schema_version: number; enabled: boolean; active_preset: PresetRef; user_presets: EqPreset[]; equipment_profiles: EquipmentProfile[] }
export type DspPhase = "off" | "preparing" | "ready" | "active" | "updating" | "stopping" | "error";
export interface Protection { requested_preamp_db:number; effective_preamp_db:number; attenuation_db:number; filter_bound_db:number; resampler_bound_db:number; ceiling_db:number }
export interface EqComparison {token:string;side:"reference"|"equalized";source:EqPreset;effective_gain_db:number;interrupted:boolean}
export interface DspSnapshot { comparison?:EqComparison|null; protection?:Protection|null; transition?:string|null; in_path: boolean | null; revision: number; phase: DspPhase; applied: EqPreset | null; error: string | null; pcm_rate: number; pcm_channels: number; verified: boolean }
export interface EqCatalog { builtin_groups?:{id:string;preset_ids:string[]}[]; built_ins:EqPreset[]; config: EqualizerConfig; revision: string; runtime: DspSnapshot; shared: boolean; runtime_available: boolean }
export type EqEdit = EqDailyEdit | { action: "create"; name: string; source: PresetRef; preset: EqPreset } | { action: "rename"; id: string; name: string } | { action: "update"; id: string; preset: EqPreset } | { action: "delete"; reference: PresetRef } | { action: "select"; reference: PresetRef };

export type EqDailyEdit = {action:"import";document:string;name:string}|{action:"link_equipment";id:string|null;name:string;reference:PresetRef}|{action:"unlink_equipment";id:string};
