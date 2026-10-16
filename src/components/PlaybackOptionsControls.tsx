import React, { useRef, useState } from "react";
import { Repeat, Shuffle } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { PlaybackOptions, PlaybackOptionAction } from "../types/audio";
import { audioService } from "../services/audio";
import { logError } from "../utils/frontendDiagnostics";

export const repeatMode = (options: PlaybackOptions | null | undefined) => {
  if (!options) return "unavailable";
  if (!options.repeat && options.single === "off") return "off";
  if (options.repeat && options.single === "off") return "all";
  if (options.repeat && options.single === "on") return "one";
  return "custom";
};

export const PlaybackOptionsControls: React.FC<{ options: PlaybackOptions | null; disabled?: boolean }> = ({ options, disabled }) => {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const pending = useRef(false);
  const mode = repeatMode(options);
  const change = async (action: PlaybackOptionAction) => {
    if (disabled || !options || pending.current) return;
    pending.current = true; setBusy(true);
    try {
      await audioService.changePlaybackOption(action, options);
      setFailed(false);
      // Display only the shared MPD status, never the requested/optimistic mode.
    } catch (error) {
      setFailed(true); logError("player.options", error);
    } finally {
      pending.current = false; setBusy(false);
    }
  };
  const repeatActive = mode !== "off" && mode !== "unavailable";
  const repeatLabel = t(`player.repeat.${mode}`);
  const randomLabel = t(!options ? "player.random.unavailable" : options.random ? "player.random.on" : "player.random.off");
  const iconStyle = (active: boolean): React.CSSProperties => ({ color: active ? "#E5A00D" : "#888888", padding: 6, position: "relative", display: "inline-flex" });
  return <span className="relative inline-flex items-center gap-2" aria-busy={busy}>
    <button type="button" disabled={disabled || !options || busy} onClick={() => void change("cycle_repeat")}
      aria-label={repeatLabel} title={repeatLabel} aria-pressed={repeatActive} style={iconStyle(repeatActive)}>
      <Repeat size={18} aria-hidden="true" />
      {mode === "one" && <span aria-hidden="true" className="absolute right-0 bottom-0 text-[9px] font-bold">1</span>}
    </button>
    <button type="button" disabled={disabled || !options || busy} onClick={() => void change("toggle_random")}
      aria-label={randomLabel} title={randomLabel} aria-pressed={Boolean(options?.random)} style={iconStyle(Boolean(options?.random))}>
      <Shuffle size={18} aria-hidden="true" />
    </button>
    {failed && <span role="alert" className="absolute top-full right-0 text-[10px] text-red-300 whitespace-nowrap">{t("player.optionsFailed")}</span>}
  </span>;
};
