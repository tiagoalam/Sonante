import React from "react";
import { ListPlus, LoaderCircle, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { sqliteLibrary, type PersonalCollection } from "../services/sqliteLibrary";
import {
  associateCollectionAlbums,
  collectionSelectionKey,
  mergeCollectionOutcomes,
  toggleCollectionSelection,
  type CollectionAddOutcome,
  type CollectionSelectionAlbum,
} from "../utils/personalCollectionOperations.mjs";

type SelectionContextValue = {
  enabled: boolean;
  albums: readonly CollectionSelectionAlbum[];
  isSelected: (album: CollectionSelectionAlbum) => boolean;
  toggle: (album: CollectionSelectionAlbum) => void;
};
const SelectionContext = React.createContext<SelectionContextValue>({
  enabled: false,
  albums: [],
  isSelected: () => false,
  toggle: () => {},
});

export function usePersonalCollectionSelection() { return React.useContext(SelectionContext); }

function errorTranslation(code: string | null | undefined, t: (key: string) => string): string {
  switch (code) {
    case "COLLECTION_LOCAL_SNAPSHOT_TOO_LARGE": return t("collections.errors.snapshotTooLarge");
    case "COLLECTION_DUPLICATE_MEMBER": return t("collections.errors.duplicate");
    case "COLLECTION_NOT_FOUND": return t("collections.errors.collectionMissing");
    case "COLLECTION_LOCAL_UNAVAILABLE":
    case "COLLECTION_SOURCE_UNAVAILABLE":
    case "COLLECTION_unavailable": return t("collections.errors.sourceUnavailable");
    case "COLLECTION_INVALID_INPUT":
    case "COLLECTION_INVALID_REFERENCE": return t("collections.errors.invalidReference");
    default: return t("collections.errors.addFailed");
  }
}

export function PersonalCollectionSelectionProvider({ enabled, children, onViewCollection }: { enabled: boolean; children: React.ReactNode; onViewCollection?: (id: number) => void }) {
  const { t } = useTranslation();
  const [albums, setAlbums] = React.useState<CollectionSelectionAlbum[]>([]);
  const [dialogOpen, setDialogOpen] = React.useState(false);
  const [collections, setCollections] = React.useState<PersonalCollection[]>([]);
  const [loading, setLoading] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [collectionName, setCollectionName] = React.useState("");
  const [outcomes, setOutcomes] = React.useState<CollectionAddOutcome[] | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const requestGeneration = React.useRef(0);
  const [destination, setDestination] = React.useState<PersonalCollection | null>(null);
  const dialogRef = React.useRef<HTMLElement>(null);
  const busyRef = React.useRef(false);
  React.useEffect(() => {
    if (!dialogOpen) return;
    const previous = document.activeElement as HTMLElement | null;
    dialogRef.current?.focus();
    const keydown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busyRef.current) { event.preventDefault(); requestGeneration.current++; setDialogOpen(false); setOutcomes(null); setError(null); }
      if (event.key === "Tab") {
        const nodes = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), [tabindex="0"]') ?? []);
        if (!nodes.length) { event.preventDefault(); return; }
        if (event.shiftKey && (document.activeElement === nodes[0] || document.activeElement === dialogRef.current)) { event.preventDefault(); nodes[nodes.length - 1].focus(); }
        else if (!event.shiftKey && document.activeElement === nodes[nodes.length - 1]) { event.preventDefault(); nodes[0].focus(); }
      }
    };
    document.addEventListener("keydown", keydown);
    return () => { document.removeEventListener("keydown", keydown); previous?.focus(); };
  }, [dialogOpen]);

  React.useEffect(() => {
    if (!dialogOpen) return;
    const generation = ++requestGeneration.current;
    let live = true;
    setLoading(true);
    setError(null);
    void sqliteLibrary.personalCollections().then(value => {
      if (live && requestGeneration.current === generation) setCollections(value);
    }).catch(() => {
      if (live && requestGeneration.current === generation) setError(t("collections.errors.loadFailed"));
    }).finally(() => {
      if (live && requestGeneration.current === generation) setLoading(false);
    });
    return () => { live = false; };
  }, [dialogOpen, t]);

  const context = React.useMemo<SelectionContextValue>(() => ({
    enabled,
    albums,
    isSelected: album => albums.some(item => collectionSelectionKey(item) === collectionSelectionKey(album)),
    toggle: album => setAlbums(current => toggleCollectionSelection(current, album)),
  }), [enabled, albums]);

  const close = () => {
    if (busyRef.current) return;
    requestGeneration.current++;
    setDialogOpen(false);
    setOutcomes(null);
    setError(null);
  };

  const associate = async (collection: PersonalCollection, allowBusy = false, retry = false) => {
    if ((busyRef.current && !allowBusy) || albums.length === 0) return;
    const collectionId = collection.id;
    setDestination(collection);
    const selected = [...albums];
    const generation = ++requestGeneration.current;
    busyRef.current = true;
    setBusy(true);
    setError(null);
    if (!retry) setOutcomes(null);
    try {
      const result = await associateCollectionAlbums(selected, album => {
        switch (album.source) {
          case "local": return sqliteLibrary.personalCollectionAddLocal(collectionId, album.albumId);
          case "plex": return sqliteLibrary.personalCollectionAddPlex(collectionId, album.serverId, album.libraryId, album.ratingKey);
          case "subsonic": return sqliteLibrary.personalCollectionAddSubsonic(collectionId, album.connectionId, album.accountId, album.albumId);
        }
      }, () => requestGeneration.current === generation, 4);
      if (requestGeneration.current !== generation || !result) return;
      setOutcomes(previous => retry && previous ? mergeCollectionOutcomes(previous, result) : result);
      const failed = new Set(result.filter(item => item.status === "failed").map(item => collectionSelectionKey(item.album)));
      setAlbums(current => current.filter(item => failed.has(collectionSelectionKey(item))));
    } finally {
      if (requestGeneration.current === generation) { busyRef.current = false; setBusy(false); }
    }
  };

  const createAndAssociate = async () => {
    const name = collectionName.trim();
    if (!name || busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError(null);
    try {
      const id = await sqliteLibrary.personalCollectionCreate(name);
      setCollectionName("");
      await associate({ id, name, created_ns: 0, updated_ns: 0 }, true);
      const generation = requestGeneration.current;
      void sqliteLibrary.personalCollections().then(value => {
        if (dialogOpen && requestGeneration.current === generation) setCollections(value);
      }).catch(() => {});
    } catch (cause) {
      setError(typeof cause === "string" && cause.includes("COLLECTION_") ? errorTranslation(cause.match(/COLLECTION_[A-Z0-9_]+/)?.[0], t) : t("collections.errors.createFailed"));
      busyRef.current = false;
      setBusy(false);
    }
  };

  const counts = React.useMemo(() => ({
    added: outcomes?.filter(item => item.status === "added").length ?? 0,
    duplicate: outcomes?.filter(item => item.status === "duplicate").length ?? 0,
    failed: outcomes?.filter(item => item.status === "failed").length ?? 0,
  }), [outcomes]);

  return <SelectionContext.Provider value={context}>
    {children}
    {enabled && albums.length > 0 && <div className="fixed bottom-24 left-1/2 z-[80] flex max-w-[calc(100vw-2rem)] -translate-x-1/2 items-center gap-3 rounded-xl border border-[color:var(--theme-border-accent,#E5A00D)]/50 bg-[var(--theme-surface-panel,#202020)] px-4 py-3 shadow-2xl">
      <span className="whitespace-nowrap text-sm font-semibold">{t("collections.selectedCount", { count: albums.length })}</span>
      <button type="button" className="inline-flex items-center gap-2 rounded-lg bg-[var(--theme-control-accent,#E5A00D)] px-3 py-2 text-xs font-bold text-[color:var(--theme-control-on-accent,#000)]" onClick={() => { setOutcomes(null); setDialogOpen(true); }}>
        <ListPlus size={16} />{t("collections.addToCollection")}
      </button>
      <button type="button" className="rounded-lg p-2 text-[color:var(--theme-fg-secondary,#aaa)] hover:bg-[var(--theme-surface-hover,#303030)]" aria-label={t("collections.clearSelection")} onClick={() => setAlbums([])}><X size={16} /></button>
    </div>}
    {enabled && dialogOpen && <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/65 p-4" role="presentation" onMouseDown={event => { if (event.target === event.currentTarget) close(); }}>
      <section ref={dialogRef} tabIndex={-1} role="dialog" aria-modal="true" aria-labelledby="collection-add-title" className="max-h-[90vh] w-full max-w-lg overflow-y-auto rounded-2xl border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-panel,#1b1b1b)] p-5 shadow-2xl">
        <div className="mb-4 flex items-start justify-between gap-3"><h2 id="collection-add-title" className="text-lg font-bold">{t("collections.addToCollection")}</h2><button type="button" disabled={busy} onClick={close} aria-label={t("collections.close")} className="rounded-lg p-2 hover:bg-[var(--theme-surface-hover,#303030)] disabled:opacity-40"><X size={17} /></button></div>
        {busy ? <p role="status" className="flex items-center gap-2 py-8 text-sm"><LoaderCircle size={18} className="animate-spin" />{t("collections.processing")}</p> : outcomes ? <div role="status" aria-live="polite" className="space-y-3 py-2">
          <p className="text-lg font-bold">{counts.failed ? t("collections.partialResult") : counts.added ? t("collections.addedResult", { count: counts.added }) : t("collections.alreadyAdded")}</p>
          {destination && (counts.added > 0 || counts.duplicate > 0) && <p className="text-sm text-[color:var(--theme-fg-secondary,#aaa)]">{t("collections.savedIn", { name: destination.name })}</p>}
          {counts.failed > 0 && <p className="text-sm">{t("collections.notAdded", { count: counts.failed })}</p>}
          {counts.duplicate > 0 && <p className="text-sm">{t("collections.duplicateCount", { count: counts.duplicate })}</p>}
          {(outcomes.length > 1 || counts.failed > 0) && <details className="text-sm"><summary className="cursor-pointer">{t("collections.details")}</summary><ul className="mt-2 max-h-36 space-y-1 overflow-y-auto">{outcomes.map(item => <li key={collectionSelectionKey(item.album)}>{item.album.title} — {item.status === "added" ? t("collections.addedShort") : item.status === "duplicate" ? t("collections.alreadyAdded") : errorTranslation(item.code, t)}</li>)}</ul></details>}
          <div className="flex flex-wrap justify-end gap-2 pt-3">{counts.failed > 0 && destination && <button type="button" onClick={() => void associate(destination, false, true)} className="rounded-lg border px-3 py-2 text-sm">{t("collections.retryFailed")}</button>}{destination && onViewCollection && <button type="button" onClick={() => { const id = destination.id; close(); onViewCollection(id); }} className="rounded-lg border px-3 py-2 text-sm">{t("collections.viewCollection")}</button>}<button type="button" onClick={close} className="rounded-lg bg-[var(--theme-control-accent,#E5A00D)] px-4 py-2 text-sm font-bold text-black">{t("collections.done")}</button></div>
        </div> : loading ? <p role="status" className="flex items-center gap-2 py-6 text-sm"><LoaderCircle size={16} className="animate-spin" />{t("collections.loading")}</p> : <>
          <p className="mb-3 text-xs text-[color:var(--theme-fg-secondary,#aaa)]">{t("collections.selectedCount", { count: albums.length })}</p>
          {collections.length > 0 ? <div className="max-h-56 space-y-1 overflow-y-auto" aria-label={t("collections.title")}>{collections.map(collection => <button key={collection.id} type="button" onClick={() => void associate(collection)} className="flex w-full items-center justify-between rounded-lg px-3 py-2.5 text-left text-sm hover:bg-[var(--theme-surface-hover,#303030)]"><span className="truncate">{collection.name}</span><ListPlus size={15} /></button>)}</div> : !error && <p className="py-3 text-sm">{t("collections.empty")}</p>}
          <form className="mt-4 flex flex-wrap gap-2 border-t border-[color:var(--theme-border-soft,#333)] pt-4" onSubmit={event => { event.preventDefault(); void createAndAssociate(); }}><input value={collectionName} maxLength={256} onChange={event => setCollectionName(event.target.value)} placeholder={t("collections.namePlaceholder")} aria-label={t("collections.namePlaceholder")} className="min-w-0 flex-1 rounded-lg border border-[color:var(--theme-border-soft,#333)] bg-[var(--theme-surface-base,#121212)] px-3 py-2 text-sm" /><button type="submit" disabled={!collectionName.trim()} className="rounded-lg border px-3 py-2 text-xs font-semibold disabled:opacity-40">{t("collections.createAndAdd")}</button></form>
        </>}
        {error && <p role="alert" className="mt-3 text-sm text-[color:var(--theme-fg-error,#f99)]">{error}</p>}
      </section>
    </div>}
  </SelectionContext.Provider>;
}
