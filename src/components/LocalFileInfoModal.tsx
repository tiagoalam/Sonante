import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import { inspectLocalFile } from "../services/localFileInfo";
import { fileInfoErrorKey } from "../utils/localFileInfo";
import type { LocalFileInfo } from "../types/localFileInfo";

export function LocalFileInfoModal({ path, displayName, available, onClose }: {
  path: string; displayName: string; available: boolean; onClose: () => void;
}) {
  const { t, i18n } = useTranslation();
  const [state, setState] = useState<{ path: string; info?: LocalFileInfo; error?: string } | null>(null);
  const requestContext = useRef({});
  const dialog = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog.current?.focus();
    const keydown = (event: KeyboardEvent) => {
      if (event.key === "Escape") { event.stopPropagation(); close.current(); }
      if (event.key === "Tab") {
        event.preventDefault(); dialog.current?.querySelector<HTMLButtonElement>("button")?.focus();
      }
    };
    document.addEventListener("keydown", keydown, true);
    return () => { document.removeEventListener("keydown", keydown, true); previous?.focus(); };
  }, []);
  useEffect(() => {
    let active = true;
    setState(null);
    if (available) void inspectLocalFile(path, requestContext.current).then(info => {
      if (active) setState({ path, info });
    }).catch(error => {
      if (active) setState({ path, error: fileInfoErrorKey(error) });
    });
    return () => { active = false; }; // Logical cancellation only; worker retains its I/O permit.
  }, [path, available]);
  const info = available && state?.path === path ? state.info : undefined;
  const error = !available ? "stale" : state?.path === path ? state.error : undefined;
  return <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/70 p-4">
    <div ref={dialog} role="dialog" aria-modal="true" aria-labelledby="local-file-info-title" tabIndex={-1}
      className="w-full max-w-md rounded-xl border border-[#333] bg-[#181818] p-5 text-[#ddd] shadow-xl">
      <div className="flex items-center justify-between gap-4">
        <h2 id="local-file-info-title" className="text-lg font-semibold">{t("fileInfo.title")}</h2>
        <button type="button" onClick={onClose} aria-label={t("fileInfo.close")} className="rounded p-2 hover:bg-[#333]"><X size={18} /></button>
      </div>
      <p className="my-3 break-words text-sm">{displayName}</p>
      <div role="status" aria-live="polite" aria-busy={!info && !error}>
        {error ? <p>{t(`fileInfo.errors.${error}`)}</p> : !info ? <p>{t("fileInfo.loading")}</p> : <>
          <p className="mb-3 text-xs text-[#aaa]">{t(info.provenance === "original_file_container_headers" ? "fileInfo.containerProvenance" : info.provenance === "original_file_mpeg_headers" ? "fileInfo.mp3Provenance" : info.provenance === "original_file_dsd_header" ? "fileInfo.dsdProvenance" : "fileInfo.provenance")}</p>
          <dl className="grid grid-cols-2 gap-2 text-sm">
            <dt>{t("fileInfo.format")}</dt><dd>{info.format}</dd>
            <dt>{t("fileInfo.codec")}</dt><dd>{info.codec}</dd>
            <dt>{t("fileInfo.compression")}</dt><dd>{t(`fileInfo.compressionValues.${info.compression}`)}</dd>
            <dt>{t(info.sample_rate_basis === "opus_decode_clock" ? "fileInfo.opusClock" : info.sample_rate_basis === "aac_core_header" ? "fileInfo.aacCoreRate" : "fileInfo.sampleRate")}</dt><dd>{info.sample_rate === null ? t("fileInfo.unknown") : info.codec === "DSD" ? t("fileInfo.dsdRate", { hz: info.sample_rate, mhz: (info.sample_rate / 1_000_000).toLocaleString(i18n.resolvedLanguage, { maximumFractionDigits: 6 }) }) : `${info.sample_rate} Hz`}</dd>
            {info.codec === "Opus" && <><dt>{t("fileInfo.originalRate")}</dt><dd>{info.original_sample_rate == null ? t("fileInfo.unknown") : `${info.original_sample_rate} Hz`}</dd></>}
            {info.codec === "DSD" ? <>
              <dt>{t("fileInfo.dsdLevel")}</dt><dd>{info.dsd_level === null ? t("fileInfo.unknownLevel") : `DSD${info.dsd_level}`}</dd>
            </> : <><dt>{t(info.sample_type === "float" ? "fileInfo.floatDepth" : "fileInfo.bitDepth")}</dt><dd>{info.sample_type === "not_applicable" ? t("fileInfo.notApplicable") : info.bit_depth === null ? t("fileInfo.unknown") : t("fileInfo.bitDepthValue", { bits: info.bit_depth })}</dd></>}
            {info.sample_type === "integer" || info.sample_type === "float" ? <><dt>{t("fileInfo.sampleType")}</dt><dd>{t(`fileInfo.sampleTypes.${info.sample_type}`)}</dd></> : null}
            {info.codec === "MP3" && <><dt>{t("fileInfo.frameBitrate")}</dt><dd>{info.frame_bitrate_kbps === null ? t("fileInfo.unknown") : t("fileInfo.bitrateValue", { kbps: info.frame_bitrate_kbps })}</dd></>}
            <dt>{t("fileInfo.channels")}</dt><dd>{info.channels}</dd>
            <dt>{t("fileInfo.duration")}</dt><dd>{info.duration_seconds === null ? t(info.codec === "MP3" || info.provenance === "original_file_container_headers" ? "fileInfo.mp3DurationUnknown" : "fileInfo.unknown") : t("fileInfo.durationValue", { seconds: info.duration_seconds.toLocaleString(i18n.resolvedLanguage, { maximumFractionDigits: 3 }) })}</dd>
          </dl>
          {info.codec === "MP3" && <p className="mt-3 text-xs text-[#aaa]">{t("fileInfo.frameBitrateHint")}</p>}
          {info.codec === "Opus" && <p className="mt-3 text-xs text-[#aaa]">{t("fileInfo.opusHint")}</p>}
          {info.sample_rate_basis === "aac_core_header" && <p className="mt-3 text-xs text-[#aaa]">{t("fileInfo.aacCoreHint")}</p>}
          {info.sample_type === "float" && <p className="mt-3 text-xs text-[#aaa]">{t("fileInfo.floatHint")}</p>}
          {info.compression === "hybrid" && <p className="mt-3 text-xs text-[#aaa]">{t("fileInfo.hybridHint")}</p>}
          <p className="mt-4 text-xs text-[#aaa]">{t("fileInfo.limits")}</p>
        </>}
      </div>
    </div>
  </div>;
}
