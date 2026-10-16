import React, { useMemo, useState } from "react";
import { ArrowLeft, CircleCheck, CircleAlert, CircleHelp, Pencil, Search, Stethoscope } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { LocalAlbumCatalogState } from "../utils/localAlbumCatalog";
import { libraryDoctorAvailable } from "../utils/libraryDoctor";
import { catalogPresenceCategory, catalogPresenceCounts, filterCatalogPresence, metadataPresenceFields, validCatalogTagPresence } from "../utils/localMetadataPresence";
import type { MetadataPresenceCategory, MetadataPresenceField } from "../utils/localMetadataPresence";
import { VirtualList } from "./VirtualList";

interface Props {
  catalog: LocalAlbumCatalogState;
  updating: boolean;
  selectedAlbumId: string | null;
  dialogsOpen: boolean;
  onSelectAlbum: (albumId: string) => void;
  onDiagnose: (albumId: string) => void;
  onReview: (albumId: string) => void;
  onBack: () => void;
}

// Catalog presentation only. Selecting/searching never inspects files or loads artwork/tracks.
export const LocalMetadataView: React.FC<Props> = ({ catalog, updating, selectedAlbumId, dialogsOpen,
  onSelectAlbum, onDiagnose, onReview, onBack }) => {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState<MetadataPresenceCategory>("all");
  const [fieldFilters, setFieldFilters] = useState<MetadataPresenceField[]>([]);
  const [scrollContainer, setScrollContainer] = useState<HTMLDivElement | null>(null);
  const selected = catalog.albums.find(album => album.id === selectedAlbumId) ?? null;
  const counts = useMemo(() => catalogPresenceCounts(catalog.albums), [catalog.albums]);
  const filtered = useMemo(() => filterCatalogPresence(catalog.albums, query, category, fieldFilters), [catalog.albums, query, category, fieldFilters]);
  const selectedEvidence = selected ? validCatalogTagPresence(selected) : null;
  const hasSnapshot = catalog.loaded || catalog.albums.length > 0;
  const staleSnapshot = hasSnapshot && (!catalog.loaded || catalog.loading || !!catalog.error || catalog.mpdBusy || catalog.saveBlocked || updating);
  const categoryLabel = (value: MetadataPresenceCategory) => t(`localMetadata.presence.categories.${value}`);
  const trackCount = catalog.albums.reduce((sum, album) => sum + album.track_count, 0);
  const canAct = !!selected && !dialogsOpen && libraryDoctorAvailable(catalog, updating, selected.id);
  const busy = updating || catalog.mpdBusy;
  return <section aria-label={t("localMetadata.title")} className="flex min-h-0 flex-1 flex-col">
    <header className="max-h-[50vh] shrink-0 space-y-3 overflow-y-auto border-b border-[#282828] px-6 py-5">
      <button type="button" disabled={dialogsOpen} onClick={onBack} className="flex items-center gap-2 text-xs text-[#BBBBBB] hover:text-white disabled:opacity-40">
        <ArrowLeft size={14} />{t("localMetadata.back")}
      </button>
      <h1 className="text-2xl font-bold text-white">{t("localMetadata.title")}</h1>
      <p className="text-xs text-[#999999]">{t("localMetadata.explanation")}</p>
      {hasSnapshot && <p className="text-xs text-[#BBBBBB]">{t("localMetadata.catalogTotals", { albums: catalog.albums.length, tracks: trackCount })}</p>}
      {(busy || catalog.loading || (!catalog.loaded && !catalog.error)) && <p role="status" className="text-xs text-amber-200">
        {t(busy ? "localMetadata.updating" : "localMetadata.loading")}</p>}
      {catalog.saveBlocked && <p role="status" className="text-xs text-amber-200">{t("localMetadata.saveBlocked")}</p>}
      {catalog.error && <p role="alert" className="whitespace-pre-wrap text-xs text-red-200">{t("localMetadata.error")} {catalog.error}</p>}
      {staleSnapshot && <p role="status" className="text-xs text-amber-200">{t("localMetadata.presence.lastSnapshot")}</p>}
      {!hasSnapshot && <p role="status" className="text-xs text-[#BBBBBB]">{t("localMetadata.presence.noSnapshot")}</p>}
      <p className="text-xs text-[#999999]">{t("localMetadata.presence.criteria")}</p>
      <div aria-label={t("localMetadata.presence.categoriesLabel")} className="grid grid-cols-2 gap-2 lg:grid-cols-4">
        {(["all", "filled", "missing", "unevaluated"] as const).map(value => <button key={value} type="button" aria-pressed={category === value}
          onClick={() => setCategory(value)} className={`rounded-lg border p-2 text-left ${category === value ? "border-[#E5A00D] bg-[#292929]" : "border-[#333333] bg-[#181818]"}`}>
          <span className="block text-xs text-[#BBBBBB]">{categoryLabel(value)}</span>
          <span className="text-lg font-bold text-white">{hasSnapshot ? counts.categories[value] : "—"}</span>
        </button>)}
      </div>
      <p className="text-xs text-[#999999]">{hasSnapshot ? t("localMetadata.presence.coverage", { evaluated: counts.evaluated, unknown: counts.categories.unevaluated }) : t("localMetadata.presence.noEvidence")}</p>
      <fieldset className="space-y-1">
        <legend className="text-xs text-[#BBBBBB]">{t("localMetadata.presence.fieldsLabel")}</legend>
        {metadataPresenceFields.map(field => <label key={field} className="grid grid-cols-[auto_1fr_3fr_auto] items-center gap-2 text-[11px] text-[#BBBBBB]">
          <input type="checkbox" checked={fieldFilters.includes(field)} onChange={event => setFieldFilters(current => event.target.checked
            ? [...current, field] : current.filter(value => value !== field))} />
          <span>{t(`localMetadata.presence.fields.${field}`)}</span>
          <span aria-hidden="true" className="h-1.5 rounded bg-[#333333]"><span className="block h-full rounded bg-[#E5A00D]"
            style={{ width: `${counts.evaluated ? counts.fields[field] / counts.evaluated * 100 : 0}%` }} /></span>
          <span>{hasSnapshot && counts.evaluated > 0 ? t("localMetadata.presence.affectedAlbums", { count: counts.fields[field] }) : t("localMetadata.presence.notEvaluated")}</span>
        </label>)}
      </fieldset>
      <p className="text-[11px] text-[#999999]">{t("localMetadata.presence.overlap")}</p>
      <label className="flex items-center gap-2 text-xs text-[#BBBBBB]"><Search size={14} />{t("localMetadata.search")}
        <input type="search" value={query} onChange={event => setQuery(event.target.value)}
          className="min-w-0 flex-1 rounded-lg border border-[#333333] bg-[#181818] px-3 py-2 text-sm text-white outline-none focus:border-[#E5A00D]" />
      </label>
    </header>
    <div className="grid min-h-0 flex-1 grid-cols-1 md:grid-cols-2">
      <div ref={setScrollContainer} className="min-h-0 overflow-y-auto border-b border-[#282828] p-4 md:border-b-0 md:border-r">
        {!filtered.length && catalog.loaded && !catalog.loading && !busy && !catalog.error && <p role="status" className="p-3 text-sm text-[#999999]">
          {t(catalog.albums.length ? "localMetadata.noMatches" : "localMetadata.empty")}</p>}
        <VirtualList items={filtered} rowHeight={100} scrollContainer={scrollContainer} resetKey={JSON.stringify([query, category, fieldFilters])}
          getItemKey={album => album.id} renderItem={album => {
            const status = catalogPresenceCategory(album), evidence = validCatalogTagPresence(album);
            const Icon = status === "filled" ? CircleCheck : status === "missing" ? CircleAlert : CircleHelp;
            return <button type="button" disabled={dialogsOpen}
            aria-pressed={selectedAlbumId === album.id} onClick={() => onSelectAlbum(album.id)}
            className={`flex h-[96px] w-full flex-col justify-center rounded-lg px-3 text-left disabled:opacity-40 ${selectedAlbumId === album.id ? "bg-[#2A2A2A] ring-1 ring-[#E5A00D]/50" : "hover:bg-[#202020]"}`}>
            <span className="w-full truncate text-sm font-semibold text-white" title={album.title}>{album.title}</span>
            <span className="w-full truncate text-xs text-[#AAAAAA]" title={album.artist}>{album.artist}</span>
            <span className="text-[11px] text-[#777777]">{t("localMetadata.catalogTracks", { count: album.track_count })}</span>
            <span className="flex items-center gap-1 text-[11px] text-[#BBBBBB]"><Icon size={12} aria-hidden="true" />{categoryLabel(status)}</span>
            {evidence && evidence.missing.genre > 0 && <span className="text-[10px] text-[#999999]">{t("localMetadata.presence.genreOptional")}</span>}
          </button>;
          }} />
      </div>
      <div className="min-h-0 space-y-4 overflow-y-auto p-6">
        {selected ? <>
          <h2 className="break-words text-xl font-bold text-white">{selected.title}</h2>
          <p className="break-words text-sm text-[#BBBBBB]">{selected.artist}</p>
          <dl className="space-y-2 text-xs text-[#999999]">
            <div><dt>{t("localMetadata.albumId")}</dt><dd className="break-all font-mono select-text">{selected.id}</dd></div>
            <div><dt>{t("localMetadata.year")}</dt><dd>{selected.year || t("localMetadata.notProvided")}</dd></div>
            <div><dt>{t("localMetadata.catalogLabel")}</dt><dd>{t("localMetadata.albumTotals", { tracks: selected.track_count, discs: selected.discs.length })}</dd></div>
          </dl>
          <section aria-label={t("localMetadata.presence.selectedTitle")} className="space-y-2 text-xs text-[#BBBBBB]">
            <h3 className="font-semibold">{t("localMetadata.presence.selectedTitle")}</h3>
            <p>{categoryLabel(catalogPresenceCategory(selected))}</p>
            {selectedEvidence ? <>
              <p>{t("localMetadata.presence.evaluatedTracks", { count: selectedEvidence.evaluated_track_count })}</p>
              <dl className="space-y-1">{metadataPresenceFields.map(field => <div key={field} className="flex flex-wrap justify-between gap-2">
                <dt>{t(`localMetadata.presence.fields.${field}`)}</dt><dd>{t("localMetadata.presence.affectedTracks", { count: selectedEvidence.missing[field] })}</dd>
              </div>)}</dl>
              {selectedEvidence.missing.album_artist > 0 && <p className="text-amber-200">{t("localMetadata.presence.albumArtistAdvice")}</p>}
            </> : <p>{t("localMetadata.presence.noEvidence")}</p>}
            <p className="text-[#999999]">{t("localMetadata.presence.limits")}</p>
          </section>
          <div className="flex flex-wrap gap-3">
            <button type="button" disabled={!canAct} onClick={() => onDiagnose(selected.id)} className="flex items-center gap-2 rounded-lg border border-[#444444] px-3 py-2 text-xs text-white disabled:opacity-40">
              <Stethoscope size={15} />{t("localMetadata.diagnose")}</button>
            <button type="button" disabled={!canAct} onClick={() => onReview(selected.id)} className="flex items-center gap-2 rounded-lg bg-[#E5A00D] px-3 py-2 text-xs font-bold text-black disabled:opacity-40">
              <Pencil size={15} />{t("localMetadata.review")}</button>
          </div>
          {!canAct && <p role="status" className="text-xs text-amber-200">{t("localMetadata.actionsBlocked")}</p>}
        </> : <p role="status" className="text-sm text-[#999999]">{t(selectedAlbumId ? "localMetadata.albumUnavailable" : "localMetadata.selectAlbum")}</p>}
      </div>
    </div>
  </section>;
};
