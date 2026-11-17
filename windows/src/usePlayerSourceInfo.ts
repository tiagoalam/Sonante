import {useMemo, useSyncExternalStore} from "react";
import {usePlayerSourceInfo as useOriginalPlayerSourceInfo} from "../../src/utils/usePlayerSourceInfo";
import type {CurrentMedia} from "../../src/types/audio";
import {getSourceContext, subscribeSourceContext} from "./sourceContext";

// Keep the original reader and media identity. A native session transition must
// invalidate even when a logical restore and subsequent Play share URI/index.
export function usePlayerSourceInfo(media: CurrentMedia | null, context: object, enabled: boolean) {
  const native = useSyncExternalStore(subscribeSourceContext, getSourceContext);
  const scoped = useMemo(() => ({context, generation: native.generation, live: native.live}), [context, native]);
  return useOriginalPlayerSourceInfo(media, scoped, enabled && native.live);
}
