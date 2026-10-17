import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import type { PlexTrack } from "../types/plex";

// Display only the safe metadata already returned with the album. Opening the
// dialog never fetches a stream, inspects a local path or changes playback.
export function PlexFileInfoModal({ track, onClose }: { track: PlexTrack; onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const element = dialog.current;
    if (element && !element.open) element.showModal();
    return () => { if (element?.open) element.close(); previous?.focus(); };
  }, []);
  const info = track.audio_info;
  const unknown = t("fileInfo.unknown");
  const dsd = info?.sample_type === "dsd";
  const duration = track.duration_ms;
  const rate = info?.sample_rate == null ? unknown : dsd
    ? t("fileInfo.dsdRate", { hz: info.sample_rate, mhz: (info.sample_rate / 1_000_000).toLocaleString(i18n.resolvedLanguage, { maximumFractionDigits: 6 }) })
    : `${info.sample_rate} Hz`;
  const depth = info?.sample_type === "not_applicable" ? t("fileInfo.notApplicable")
    : info?.bit_depth == null ? unknown : t("fileInfo.bitDepthValue", { bits: info.bit_depth });
  return <dialog ref={dialog} aria-labelledby="plex-file-info-title" aria-modal="true" tabIndex={-1}
    className="m-auto max-h-[85dvh] w-11/12 max-w-md overflow-y-auto rounded-xl border border-[#333] bg-[#181818] p-5 text-[#ddd] shadow-2xl backdrop:bg-black/70"
    onKeyDown={event => event.stopPropagation()}
    onCancel={event => { event.preventDefault(); onClose(); }}
    // StrictMode closes/reopens during effect replay. Its queued close event
    // must not dismiss a dialog that is already open again.
    onClose={event => { if (!event.currentTarget.open) onClose(); }}
    onClick={event => { event.stopPropagation(); if (event.target === event.currentTarget) {
      const box = event.currentTarget.getBoundingClientRect();
      if (event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom) onClose();
    } }}>
    <header className="flex items-center justify-between gap-4">
      <h2 id="plex-file-info-title" className="text-lg font-semibold">{t("fileInfo.title")}</h2>
      <button type="button" autoFocus onClick={onClose} aria-label={t("fileInfo.close")}
        className="rounded p-2 hover:bg-[#333] focus-visible:outline focus-visible:outline-[#E5A00D]"><X size={18} /></button>
    </header>
    <p className="my-3 break-words text-sm">{track.title}</p>
    <p className="mb-3 text-xs text-[#aaa]">{t("fileInfo.plexProvenance")}</p>
    <dl className="grid grid-cols-2 gap-2 text-sm">
      <dt>{t("fileInfo.format")}</dt><dd>{info?.container ?? unknown}</dd>
      <dt>{t("fileInfo.codec")}</dt><dd>{info?.codec ?? unknown}</dd>
      <dt>{t("fileInfo.compression")}</dt><dd>{info?.compression ? t(`fileInfo.compressionValues.${info.compression}`) : unknown}</dd>
      <dt>{t("fileInfo.sampleRate")}</dt><dd>{rate}</dd>
      {dsd ? <><dt>{t("fileInfo.dsdLevel")}</dt><dd>{info?.dsd_level == null ? t("fileInfo.unknownLevel") : `DSD${info.dsd_level}`}</dd></>
        : <><dt>{t(info?.sample_type === "float" ? "fileInfo.floatDepth" : "fileInfo.bitDepth")}</dt><dd>{depth}</dd></>}
      {(info?.sample_type === "integer" || info?.sample_type === "float") && <><dt>{t("fileInfo.sampleType")}</dt><dd>{t(`fileInfo.sampleTypes.${info.sample_type}`)}</dd></>}
      <dt>{t("fileInfo.channels")}</dt><dd>{info?.channels ?? unknown}</dd>
      <dt>{t("fileInfo.plexBitrate")}</dt><dd>{info?.bitrate_kbps == null ? unknown : t("fileInfo.bitrateValue", { kbps: info.bitrate_kbps })}</dd>
      <dt>{t("fileInfo.duration")}</dt><dd>{duration != null && Number.isSafeInteger(duration) && duration > 0
        ? t("fileInfo.durationValue", { seconds: (duration / 1000).toLocaleString(i18n.resolvedLanguage, { maximumFractionDigits: 3 }) }) : unknown}</dd>
    </dl>
    <p className="mt-4 text-xs text-[#aaa]">{t("fileInfo.plexLimits")}</p>
  </dialog>;
}
