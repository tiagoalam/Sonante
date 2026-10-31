import React, { useEffect, useRef, useState } from "react";
import { NowPlayingIdle } from "../utils/nowPlayingIdle";

interface Props extends React.ComponentPropsWithoutRef<"main"> { enabled: boolean; }

// Keep all content mounted and its geometry stable while fading only the controls.
export const NowPlayingImmersive: React.FC<Props> = ({ enabled, className = "", children, ...rest }) => {
  const rootRef = useRef<HTMLElement>(null);
  const [hidden, setHidden] = useState(false);
  useEffect(() => {
    const root = rootRef.current;
    setHidden(false);
    if (!enabled || !root) return;
    let held = false;
    let focusRequest: number | undefined;
    let keyboard = Boolean(document.activeElement?.matches(":focus-visible"));
    const idle = new NowPlayingIdle({
      setTimer: (fn, ms) => window.setTimeout(fn, ms), clearTimer: id => window.clearTimeout(id), changed: next => {
        // Reveal synchronously so the existing Escape focus restoration can see the header.
        if (!next) root.removeAttribute("data-np-controls-hidden");
        setHidden(next);
      },
      blocked: () => {
        if (held || document.hidden) return true;
        const active = document.activeElement;
        if (active && root.contains(active) && (keyboard || active.matches('input[type="text"],input[type="search"],textarea,[contenteditable="true"]'))) return true;
        return Boolean(root.querySelector('details[open],dialog[open],[role="dialog"],[role="menu"],[role="listbox"],[role="alert"]'));
      },
    });
    const activity = () => idle.activity();
    const move = (event: Event) => { if ((event as PointerEvent).buttons === 0) held = false; idle.activity(); };
    const down = () => { keyboard = false; held = true; idle.activity(); };
    const up = () => { held = false; idle.activity(); };
    const blur = () => { held = false; idle.pause(); };
    const visibility = () => { if (document.hidden) blur(); else activity(); };
    const key = (event: KeyboardEvent) => {
      keyboard = true;
      // First Tab from the hidden state reveals and focuses the header, rather than skipping it.
      const focusHeader = idle.hidden && event.key === "Tab" && !event.defaultPrevented;
      idle.activity();
      if (focusHeader) {
        event.preventDefault();
        if (focusRequest !== undefined) window.cancelAnimationFrame(focusRequest);
        focusRequest = window.requestAnimationFrame(() => {
          focusRequest = undefined;
          root.querySelector<HTMLButtonElement>('.np-header button:not(:disabled)')?.focus();
        });
      }
    };
    const globalEvents = [
      ["pointermove", move], ["pointerdown", down], ["pointerup", up], ["pointercancel", up],
      ["wheel", activity], ["focus", activity], ["blur", blur],
    ] as const;
    globalEvents.forEach(([name, fn]) => window.addEventListener(name, fn, { passive: true }));
    window.addEventListener("keydown", key, true);
    root.addEventListener("focusin", activity); root.addEventListener("focusout", activity);
    root.addEventListener("toggle", activity, true);
    document.addEventListener("visibilitychange", visibility);
    if (!document.hidden) idle.activity();
    return () => {
      idle.dispose();
      if (focusRequest !== undefined) window.cancelAnimationFrame(focusRequest);
      globalEvents.forEach(([name, fn]) => window.removeEventListener(name, fn));
      window.removeEventListener("keydown", key, true);
      root.removeEventListener("focusin", activity); root.removeEventListener("focusout", activity);
      root.removeEventListener("toggle", activity, true);
      document.removeEventListener("visibilitychange", visibility);
    };
  }, [enabled]);
  return <main {...rest} ref={rootRef} className={className} data-np-controls-hidden={enabled && hidden ? "true" : undefined}>{children}</main>;
};
