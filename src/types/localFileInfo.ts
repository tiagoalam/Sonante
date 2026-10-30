export interface LocalFileInfo {
  provenance: "original_file_streaminfo" | "original_file_dsd_header" | "original_file_mpeg_headers" | "original_file_container_headers";
  /** Original file container, independent of the encoded signal. */
  format: "FLAC" | "DSF" | "DSDIFF" | "MPEG Audio" | "WAV" | "AIFF" | "AIFF-C" | "ADTS" | "MP4/M4A" | "Ogg" | "WavPack" | "Musepack SV7" | "Musepack SV8";
  codec: "FLAC" | "DSD" | "MP3" | "PCM" | "PCM float" | "AAC" | "ALAC" | "Vorbis" | "Opus" | "WavPack" | "Musepack";
  lossy: boolean;
  compression: "lossy" | "lossless" | "uncompressed" | "hybrid" | "unknown";
  sample_type: "integer" | "float" | "dsd" | "not_applicable";
  sample_rate_basis: "encoded_header" | "aac_core_header" | "opus_decode_clock";
  /** Opus informational input rate; never substitutes its 48 kHz decoding clock. */
  original_sample_rate: number | null;
  /** First verified frame only; not an average or CBR/VBR declaration. */
  frame_bitrate_kbps: number | null;
  dsd_level: 64 | 128 | 256 | null;
  sample_rate: number | null;
  bit_depth: number | null;
  channels: number;
  duration_seconds: number | null;
  revision: string;
}
