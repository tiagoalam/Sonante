import { logError } from "./frontendDiagnostics";
import { useEffect, useMemo, useRef, useState } from "react";
import { beginPlaybackIntent, isPlaybackSuperseded, playbackFailure } from "./playbackIntent";
import type { PlaybackFailure, PlaybackIntent } from "./playbackIntent";

// Error and cancellation belong to one visit/attempt, even on A -> B -> A.
export function usePlaybackAttempt(context: unknown, operation: "player.request" | "local.album-play" | "plex.album-play" = "player.request", identity?: unknown) {
  const visit = useMemo(() => ({ context, identity }), [context, identity]);
  const live = useRef(visit); live.current = visit;
  const owner = useRef<{ visit: object; intent: PlaybackIntent } | null>(null);
  const [failure, setFailure] = useState<{ visit: object; value: PlaybackFailure } | null>(null);
  useEffect(() => () => {
    const owned = owner.current;
    if (owned?.visit === visit) { owned.intent.cancel(); owner.current = null; }
  }, [visit]);
  return {
    failure: failure?.visit === visit ? failure.value : null,
    async run(action: (intent: PlaybackIntent) => Promise<unknown>) {
      const intent = beginPlaybackIntent(); owner.current = { visit, intent }; setFailure(null);
      try { await action(intent); }
      catch (error) {
        if (!isPlaybackSuperseded(error) && await intent.confirmCurrent() && live.current === visit) {
          if (operation === "local.album-play") logError("local.album-play", error);
          else if (operation === "plex.album-play") logError("plex.album-play", error);
          else logError("player.request", error);
          setFailure({ visit, value: playbackFailure(error) });
        }
      }
    },
  };
}
