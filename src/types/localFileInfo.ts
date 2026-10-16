export interface LocalFileInfo {
  provenance: "original_file_streaminfo";
  format: "FLAC";
  sample_rate: number | null;
  bit_depth: number;
  channels: number;
  duration_seconds: number | null;
  revision: string;
}
