import { invoke } from "@tauri-apps/api/core";
import type { RadioFilters, RadioPage, RadioQuery, RadioStation } from "../types/radio";
import { ensureDirectOutputChoice } from "./config";
import { playbackIntentToken, type PlaybackIntent } from "../utils/playbackIntent";
import { localSourceReadQueue } from "../utils/localSourceReadQueue";
export const radioService = {
  search: (search: RadioQuery) => invoke<RadioPage>("search_radio", { search }),
  filters: () => invoke<RadioFilters>("radio_filters"),
  favorites: () => invoke<RadioStation[]>("radio_favorites"),
  favorite: (id: string, enabled: boolean) => invoke<RadioStation[]>("radio_favorites", { id, enabled }),
  manual: (name: string, url: string) => invoke<RadioStation[]>("radio_favorites", { name, url }),
  edit: (id: string, name: string, url: string) => invoke<RadioStation[]>("radio_favorites", { id, name, url }),
  async stop(id: string, intent: PlaybackIntent): Promise<void> {
    const token = await playbackIntentToken(intent);
    return localSourceReadQueue.control(() => {
      if (!intent.current()) throw "PLAYBACK_SUPERSEDED";
      return invoke<void>("stop_radio", { id, intent: token });
    });
  },
  async play(id: string, intent: PlaybackIntent): Promise<void> {
    const token = await playbackIntentToken(intent);
    return localSourceReadQueue.control(async () => {
      if (!intent.current()) throw "PLAYBACK_SUPERSEDED";
      await ensureDirectOutputChoice(intent);
      if (!intent.current()) throw "PLAYBACK_SUPERSEDED";
      return invoke<void>("play_radio", { id, intent: token });
    });
  },
};
