import { invoke } from "@tauri-apps/api/core";
import type { CurrentMedia } from "../types/audio";
import type { PlayerSourceInfo } from "../types/sourceAudio";
import { SourceReadGate } from "../utils/sourceReadGate";
const gate = new SourceReadGate();
export const readPlayerSource = (media: CurrentMedia, current: () => boolean) =>
  gate.run("player", () => invoke<PlayerSourceInfo | null>("get_player_source_info", { media }), current);
