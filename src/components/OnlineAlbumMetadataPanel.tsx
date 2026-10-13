import React, { useEffect, useId, useMemo, useRef, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import type { LocalAlbumTagInspection } from "../types/localTags";
import { onlineMetadataService } from "../services/onlineMetadata";
import { createOnlineMetadataSession } from "../utils/onlineMetadata";
import { OnlineMetadataComparison } from "./OnlineMetadataComparison";
import type { LocalAlbumTagEditorDraft } from "../utils/localAlbumTagEditor";
import type { MetadataTransferRequest } from "../utils/onlineMetadataTransfer";
import type { OnlineMetadataResult } from "../types/onlineMetadata";

interface Props {
  albumId: string;
  catalogVersion: unknown;
  inspection: LocalAlbumTagInspection;
  available: boolean;
  isAvailable: () => boolean;
  draft?: LocalAlbumTagEditorDraft;
  editorRevision?: string;
  transferAvailable?: boolean;
  onTransfer?: (request: MetadataTransferRequest) => boolean;
}

export const OnlineMetadataResults: React.FC<{ result: OnlineMetadataResult; onCompare?: (id: string) => void; disabled?: boolean }> = ({ result, onCompare, disabled }) => {
  const { t } = useTranslation();
  const value = (text: string | number | null) => text === null ? t("onlineMetadata.missing")
    : text === "" ? t("onlineMetadata.empty") : String(text);
  return <div className="mt-3 space-y-3">
    <p role="status" className="text-xs text-[#BBBBBB]">{t(result.candidates.length === 0 ? "onlineMetadata.noResults"
      : result.limited ? "onlineMetadata.limited" : "onlineMetadata.resultCount", { count: result.candidates.length, total: result.total })}</p>
    <ol className="space-y-3">{result.candidates.map(candidate => <li key={candidate.id} className="rounded-lg border border-[#333333] p-3 text-xs">
      <h4 className="whitespace-pre-wrap break-words font-bold text-white">{value(candidate.title)}</h4>
      <dl className="mt-2 grid gap-2 sm:grid-cols-2">
        {[["artistCredit", value(candidate.artist_credit)], ["date", value(candidate.date)], ["country", value(candidate.country)],
          ["format", candidate.formats.length ? candidate.formats.map(value).join(" · ") : value(null)],
          ["score", value(candidate.score)]].map(([field, text]) => <div key={field}>
            <dt className="text-[#888888]">{t(`onlineMetadata.${field}`)}</dt><dd className="whitespace-pre-wrap break-words">{text}</dd>
          </div>)}
        <div><dt className="text-[#888888]">{t("onlineMetadata.labelCatalog")}</dt>
          <dd>{candidate.labels.length ? <ul>{candidate.labels.map((label, index) =>
            <li key={index} className="whitespace-pre-wrap break-words">{value(label.name)} / {value(label.catalog_number)}</li>)}</ul> : value(null)}</dd>
        </div>
      </dl>
      <p className="mt-2 break-all font-mono text-[10px] text-[#777777]">{candidate.id}</p>
      {onCompare && <button type="button" disabled={disabled} onClick={() => onCompare(candidate.id)}
        className="mt-2 rounded-lg border border-[#555555] px-3 py-2 text-white hover:bg-[#303030] disabled:opacity-40">
        {t("onlineMetadata.comparison.compare")}
      </button>}
    </li>)}</ol>
  </div>;
};

export const OnlineAlbumMetadataPanel: React.FC<Props> = ({ albumId, catalogVersion, inspection, available, isAvailable, draft, editorRevision, transferAvailable, onTransfer }) => {
  const { t } = useTranslation();
  const id = useId();
  const liveAvailability = useRef(isAvailable);
  liveAvailability.current = isAvailable;
  const session = useMemo(() => createOnlineMetadataSession(onlineMetadataService.search, () => liveAvailability.current(), onlineMetadataService.details), []);
  session.syncContext({ albumId, catalogVersion, inspection, available, draft, editorRevision, transferAvailable }, false);
  const state = useSyncExternalStore(session.subscribe, session.getSnapshot, session.getSnapshot);
  useEffect(() => { session.activate(); return () => session.close(); }, [session]);

  return <section aria-labelledby={`${id}-title`} className="mt-5 rounded-xl border border-[#333333] bg-[#202020] p-4"
    onKeyDown={event => {
      // These query inputs are inside TAG-2's form, but Enter must not submit a tag preview.
      if (event.key === "Enter" && event.target instanceof HTMLInputElement) { event.preventDefault(); event.stopPropagation(); }
    }}>
    <h3 id={`${id}-title`} className="text-sm font-bold text-white">{t("onlineMetadata.title")}</h3>
    <p className="mt-2 text-xs text-[#BBBBBB]">{t("onlineMetadata.explanation")}</p>
    <div className="mt-3 grid gap-3 sm:grid-cols-2">{(["title", "artist"] as const).map(field => <div key={field}>
      <label htmlFor={`${id}-${field}`} className="text-xs text-[#BBBBBB]">{t(`onlineMetadata.query.${field}`)}</label>
      <input id={`${id}-${field}`} type="text" value={state.query[field]} maxLength={256} disabled={!available}
        onChange={event => session.setQuery(field, event.target.value)}
        className="mt-1 w-full rounded-lg border border-[#444444] bg-[#111111] px-3 py-2 text-sm text-white focus:border-[#E5A00D] disabled:opacity-50" />
    </div>)}</div>
    <button type="button" onClick={() => void session.search()} disabled={!available || state.inFlight || !state.query.title.trim()}
      className="mt-3 rounded-lg border border-[#555555] px-3 py-2 text-xs text-white hover:bg-[#303030] disabled:cursor-not-allowed disabled:opacity-40">
      {t(state.inFlight ? "onlineMetadata.searching" : "onlineMetadata.search")}
    </button>
    {!available && <p role="status" className="mt-2 text-xs text-amber-200">{t("onlineMetadata.unavailable")}</p>}
    {state.inFlight && state.phase !== "loading" && state.detailPhase !== "loading" && <p role="status" className="mt-2 text-xs text-[#BBBBBB]">{t("onlineMetadata.pendingDiscard")}</p>}
    {state.error && <p role="alert" className="mt-3 text-xs text-red-200">{t(`onlineMetadata.errors.${state.error}`)}</p>}
    {state.detailPhase === "loading" && <p role="status" className="mt-3 text-xs">{t("onlineMetadata.comparison.loading")}</p>}
    {state.detailError && <p role="alert" className="mt-3 text-xs text-red-200">{t(`onlineMetadata.errors.${state.detailError}`)}</p>}
    {state.details && draft && <OnlineMetadataComparison details={state.details} draft={draft}
      available={!!transferAvailable && available && !state.inFlight} choices={state.choices} dateSource={state.dateSource}
      onChoose={session.choose} onDateSource={session.chooseDateSource}
      onTransfer={() => { if (onTransfer) session.transfer(onTransfer); }} />}
    {state.result && <OnlineMetadataResults result={state.result} disabled={!available || state.inFlight} onCompare={id => void session.compare(id)} />}
    <p className="mt-3 text-xs text-[#999999]">{t("onlineMetadata.limits")}</p>
  </section>;
};
