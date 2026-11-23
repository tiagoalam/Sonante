import type { AudioRouteProjection } from "./audio";

export type EqFilterType = "peak" | "low_shelf" | "high_shelf";
export interface EqBand { id: string; enabled: boolean; filter_type: EqFilterType; frequency_hz: number; gain_db: number; q: number }
export interface EqPreset { id: string; name: string; preamp_db: number; bands: EqBand[] }
export type PresetRef = { kind: "built_in" | "user"; id: string };
export type EquipmentCategory = "headphones" | "speakers" | "other";
export interface EquipmentProfile { id:string; name:string; category:EquipmentCategory; manufacturer:string|null; opra_vendor_id:string|null; opra_product_id:string|null; presets:PresetRef[]; preferred_preset:PresetRef|null }
export interface EqualizerConfig { schema_version: number; enabled: boolean; active_preset: PresetRef; user_presets: EqPreset[]; equipment_profiles: EquipmentProfile[] }
export type DspPhase = "off" | "preparing" | "ready" | "active" | "updating" | "stopping" | "error";
export interface Protection { requested_preamp_db:number; effective_preamp_db:number; attenuation_db:number; filter_bound_db:number; resampler_bound_db:number; ceiling_db:number }
export interface EqComparison {token:string;side:"reference"|"equalized";source:EqPreset;effective_gain_db:number;interrupted:boolean}
export interface DspSnapshot { comparison?:EqComparison|null; protection?:Protection|null; transition?:string|null; in_path: boolean | null; revision: number; phase: DspPhase; applied: EqPreset | null; error: string | null; pcm_rate: number; pcm_channels: number; verified: boolean }
export interface EqualizerApplyResult { runtime: DspSnapshot; audioRoute: AudioRouteProjection | null }
export interface EqCatalog { builtin_groups?:{id:string;preset_ids:string[]}[]; built_ins:EqPreset[]; config: EqualizerConfig; revision: string; runtime: DspSnapshot; shared: boolean; runtime_available: boolean; runtime_diagnostic?: string | null; pulse_audio: boolean }
export type EqEdit = EqDailyEdit | { action: "import_autoeq"; document: string; name: string } | { action: "create"; name: string; source: PresetRef; preset: EqPreset } | { action: "rename"; id: string; name: string } | { action: "update"; id: string; preset: EqPreset } | { action: "delete"; reference: PresetRef } | { action: "select"; reference: PresetRef };

export interface AutoEqImportPreview { document: string; preset: EqPreset; source_filter_count: number; source_name: string }
export interface OpraBandHit { filter_type:string; frequency:number; gain_db:number|null; q:number|null; slope:number|null }
export interface OpraProfileHit { profile_id:string; author:string; details:string|null; source_link:string|null; overall_gain_db:number; bands:OpraBandHit[]; compatibility:"compatible"|"incompatible"|"unverified"; compatibility_reasons:string[] }
export interface OpraProductHit { product_id:string; vendor_id:string|null; product_name:string; vendor_name:string|null; product_type:string; subtype:string; profile_count:number; profiles:OpraProfileHit[] }
export interface OpraCatalogSearch { snapshot_sha256:string; dataset_license:string; products:OpraProductHit[]; truncated:boolean }

export type EqDailyEdit = {action:"import";document:string;name:string}|{action:"link_equipment";id:string|null;name:string;reference:PresetRef}|{action:"add_equipment";id:string|null;name:string;category:EquipmentCategory;manufacturer:string|null;opra_vendor_id:string|null;opra_product_id:string|null}|{action:"add_equipment_preset";id:string;reference:PresetRef;preferred:boolean}|{action:"remove_equipment_preset";id:string;reference:PresetRef}|{action:"set_preferred_equipment_preset";id:string;reference:PresetRef|null}|{action:"unlink_equipment";id:string};
