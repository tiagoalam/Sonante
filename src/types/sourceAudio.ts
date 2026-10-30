import type { CurrentMedia } from "./audio";
export interface SourceAudioFacts {
  provenance: "local" | "plex";
  codec: string;
  compression: "lossy" | "lossless" | "uncompressed" | "hybrid" | "unknown";
  sample_type: "integer" | "float" | "dsd" | "not_applicable";
  sample_rate: number | null;
  bit_depth: number | null;
  dsd_level: number | null;
  revision: string | null;
}
export interface PlayerSourceInfo { media: CurrentMedia; info: SourceAudioFacts; }
