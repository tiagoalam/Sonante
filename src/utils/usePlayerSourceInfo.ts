import { useLayoutEffect, useMemo, useSyncExternalStore } from "react";
import { readPlayerSource } from "../services/sourceAudio";
import type { CurrentMedia } from "../types/audio";
import { currentMediaKey } from "./playerNavigation";
import { PlayerSourceReader } from "./sourceAudio";

// Only App acquires. Both Now Playing hosts receive the same facts in their
// existing atomic snapshot; detaching/reincorporating never starts another read.
export function usePlayerSourceInfo(media: CurrentMedia | null, context: object, enabled: boolean) {
  const reader = useMemo(() => new PlayerSourceReader(readPlayerSource), []);
  const snapshot = useSyncExternalStore(reader.subscribe, reader.getSnapshot);
  const key = currentMediaKey(media);
  useLayoutEffect(() => {
    reader.setContext(enabled ? media : null, context);
    return () => reader.setContext(null, context);
  }, [reader, key, context, enabled]);
  return enabled && reader.matches(media, context) ? snapshot : null;
}
