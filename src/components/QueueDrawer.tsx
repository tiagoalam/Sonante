import React, { useEffect, useRef } from "react";
import { X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { PlaybackStatus, QueueArtworkContext } from "../types/audio";
import { NowPlayingQueue } from "./NowPlayingQueue";
import "./nowPlaying.css";

export const QueueDrawer: React.FC<{ isOpen: boolean; onClose: () => void; status: PlaybackStatus; isPlaybackAvailable: boolean; artworkContext?: QueueArtworkContext }> = ({ isOpen, onClose, status, isPlaybackAvailable, artworkContext }) => {
  const { t } = useTranslation();
  const closeRef = useRef<HTMLButtonElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  useEffect(() => {
    if (!isOpen) return;
    const previousFocus = document.activeElement;
    closeRef.current?.focus();
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onCloseRef.current(); } };
    window.addEventListener("keydown", escape);
    return () => { window.removeEventListener("keydown", escape); if (previousFocus instanceof HTMLElement && previousFocus.isConnected && !previousFocus.closest("[hidden]")) previousFocus.focus({ preventScroll: true }); };
  }, [isOpen]);
  if (!isOpen) return null;
  return <div data-now-playing-queue-content className="np-queue-drawer" role="region" aria-label={t("queue.title")}>
    <button ref={closeRef} type="button" className="np-drawer-close" onClick={onClose} aria-label={t("nowPlaying.hideQueue")}><X size={18} /></button>
    <NowPlayingQueue open available={isPlaybackAvailable} status={status} artworkContext={artworkContext} />
  </div>;
};
export default QueueDrawer;
