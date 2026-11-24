import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { ArrowLeft, Database, Disc3, Folder, Layers3, LoaderCircle, Pencil, Plus, Server, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { sqliteLibrary, type PersonalCollection, type PersonalCollectionMember, type PersonalCollectionResolution } from "../services/sqliteLibrary";
import { AlbumCatalog } from "./AlbumCatalog";
import { AlbumViewControls } from "./AlbumViewControls";
import { readAlbumPresentation, saveAlbumPresentation, type AlbumPresentation } from "../utils/albumPresentation";
import { PersonalCollectionArtwork } from "./PersonalCollectionArtwork";
import { createVisibleResolutionQueue } from "../utils/personalCollectionOperations.mjs";

const sourceIcons = { local: Folder, plex: Server, subsonic: Database } as const;
type Props = {
  onOpenAlbum: (member: PersonalCollectionMember, target: NonNullable<PersonalCollectionResolution["target"]>) => void;
  onContextChange?: () => void;
  requestedCollectionId?: number | null;
  onRequestHandled?: () => void;
  sourceContext?: object;
};
type ResolutionState = { result: PersonalCollectionResolution | null; error: boolean };

export function PersonalCollectionsView({ onOpenAlbum, onContextChange, requestedCollectionId, onRequestHandled, sourceContext }: Props) {
  const { t } = useTranslation();
  const [collections, setCollections] = useState<PersonalCollection[]>([]);
  const [counts, setCounts] = useState<Record<number, number | null>>({});
  const [selected, setSelected] = useState<PersonalCollection | null>(null);
  const [members, setMembers] = useState<PersonalCollectionMember[]>([]);
  const [resolutions, setResolutions] = useState<Record<number, ResolutionState>>({});
  const [name, setName] = useState("");
  const [loading, setLoading] = useState(true);
  const [membersLoading, setMembersLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);
  const gridRef = useRef<HTMLDivElement>(null);
  const [scrollContainer, setScrollContainer] = useState<HTMLElement | null>(null);
  const [presentation, setPresentation] = useState<AlbumPresentation>(() => {
    const saved = readAlbumPresentation("collections");
    return saved.layout === "carousel" || saved.layout === "shelf" ? saved : { ...saved, layout: "grid" };
  });
  const changePresentation = (value: AlbumPresentation) => { setPresentation(value); saveAlbumPresentation("collections", value); };

  const loadCollections = useCallback(async () => {
    const own = ++generation.current;
    setLoading(true); setError(null);
    try {
      const rows = await sqliteLibrary.personalCollections();
      if (generation.current !== own) return;
      setCollections(rows);
      const values = await Promise.all(rows.map(async collection => {
        try { return [collection.id, (await sqliteLibrary.personalCollectionMembers(collection.id)).length] as const; }
        catch { return [collection.id, null] as const; }
      }));
      if (generation.current === own) setCounts(Object.fromEntries(values));
    } catch {
      if (generation.current === own) setError(t("collections.errors.loadFailed"));
    } finally {
      if (generation.current === own) setLoading(false);
    }
  }, [t]);

  useEffect(() => { void loadCollections(); return () => { generation.current++; }; }, [loadCollections]);

  const openCollection = async (collection: PersonalCollection) => {
    onContextChange?.();
    const own = ++generation.current;
    setSelected(collection); setMembers([]); setResolutions({}); setError(null); setMembersLoading(true);
    try {
      const rows = await sqliteLibrary.personalCollectionMembers(collection.id);
      if (generation.current !== own) return;
      setMembers(rows);

    } catch {
      if (generation.current === own) setError(t("collections.errors.membersFailed"));
    } finally {
      if (generation.current === own) setMembersLoading(false);
    }
  };

  const previousSourceContext = useRef(sourceContext);
  useLayoutEffect(() => {
    if (previousSourceContext.current === sourceContext) return;
    previousSourceContext.current = sourceContext;
    if (selected) void openCollection(selected);
  }, [sourceContext]);

  useEffect(() => {
    if (!selected || !members.length || !gridRef.current) return;
    const own = generation.current;
    const visible = new Map<number, HTMLElement>();
    const resolved = new Set<number>(Object.entries(resolutions).filter(([, state]) => !state.error).map(([id]) => Number(id)));
    const byId = new Map(members.map(member => [member.id, member]));
    const queue = createVisibleResolutionQueue<PersonalCollectionMember>(
      member => sqliteLibrary.personalCollectionResolve(selected.id, member.id),
      (memberId, result, failure) => {
        if (!failure) resolved.add(memberId);
        setResolutions(state => ({ ...state, [memberId]: { result: result as PersonalCollectionResolution | null, error: Boolean(failure) } }));
      }, 4);
    const grid = gridRef.current;
    const change = (node: HTMLElement, shown: boolean) => {
      const id = Number(node.dataset.collectionMember), member = byId.get(id);
      if (!member) return;
      if (!shown) { if (visible.get(id) === node) visible.delete(id); return; }
      visible.set(id, node);
      if (!resolved.has(id)) queue.enqueue(member, () => generation.current === own && Boolean(visible.get(id)?.isConnected));
    };
    const observer = typeof IntersectionObserver === "undefined" ? null : new IntersectionObserver(entries => entries.forEach(entry => change(entry.target as HTMLElement, entry.isIntersecting)));
    const observe = () => grid.querySelectorAll<HTMLElement>("[data-collection-member]").forEach(node => {
      if (observer) observer.observe(node); else change(node, true);
    });
    observe();
    const mutations = typeof MutationObserver === "undefined" ? null : new MutationObserver(observe);
    mutations?.observe(grid, { childList: true, subtree: true });
    return () => { mutations?.disconnect(); observer?.disconnect(); queue.dispose(); };
  }, [selected?.id, members, presentation.layout]);

  useEffect(() => {
    if (requestedCollectionId == null || loading) return;
    const requested = collections.find(item => item.id === requestedCollectionId);
    if (requested) void openCollection(requested);
    else setError(t("collections.errors.collectionMissing"));
    onRequestHandled?.();
  }, [requestedCollectionId, loading, collections]);

  const back = () => { onContextChange?.(); generation.current++; setSelected(null); setMembers([]); setResolutions({}); setError(null); void loadCollections(); };
  const create = async (event: React.FormEvent) => {
    event.preventDefault();
    const value = name.trim();
    if (!value || busy) return;
    setBusy(true); setError(null);
    try { await sqliteLibrary.personalCollectionCreate(value); setName(""); await loadCollections(); }
    catch { setError(t("collections.errors.createFailed")); }
    finally { setBusy(false); }
  };
  const rename = async () => {
    if (!selected || busy) return;
    const value = window.prompt(t("collections.renamePrompt"), selected.name)?.trim();
    if (!value || value === selected.name) return;
    setBusy(true); setError(null);
    try {
      await sqliteLibrary.personalCollectionRename(selected.id, value);
      const updated = { ...selected, name: value };
      setSelected(updated); setCollections(items => items.map(item => item.id === updated.id ? updated : item));
    } catch { setError(t("collections.errors.renameFailed")); }
    finally { setBusy(false); }
  };
  const removeCollection = async () => {
    if (!selected || busy || !window.confirm(t("collections.deleteConfirm", { name: selected.name }))) return;
    onContextChange?.();
    const id = selected.id;
    setBusy(true); setError(null);
    try { await sqliteLibrary.personalCollectionDelete(id); setSelected(null); setMembers([]); setResolutions({}); await loadCollections(); }
    catch { setError(t("collections.errors.deleteFailed")); }
    finally { setBusy(false); }
  };
  const removeMember = async (member: PersonalCollectionMember) => {
    if (!selected || busy) return;
    setBusy(true); setError(null);
    try {
      await sqliteLibrary.personalCollectionRemove(selected.id, member.id);
      setMembers(items => items.filter(item => item.id !== member.id));
      setResolutions(items => { const next = { ...items }; delete next[member.id]; return next; });
      setCounts(items => ({ ...items, [selected.id]: Math.max(0, (items[selected.id] ?? members.length) - 1) }));
    } catch { setError(t("collections.errors.removeFailed")); }
    finally { setBusy(false); }
  };

  const renderMember = (member: PersonalCollectionMember, carousel?: { focused: boolean }) => {
          const state = resolutions[member.id];
          const status = state?.result?.status === "available" && !state.result.target
            ? "unresolved"
            : state?.result?.status;
          const Icon = sourceIcons[member.source];
          const canOpen = status === "available" && Boolean(state.result?.target);
          return <article key={member.id} data-collection-member={member.id} className="group min-w-0 rounded-xl border border-[color:var(--theme-border-soft,#303030)] bg-[var(--theme-surface-panel,#191919)] p-3">
            <button type="button" data-album-open disabled={!canOpen} onClick={() => { if (canOpen && state.result?.target) onOpenAlbum(member, state.result.target); }} className="w-full text-left disabled:cursor-default">
              <span className="mb-3 flex aspect-square items-center justify-center rounded-lg bg-[var(--theme-surface-raised,#242424)] text-[color:var(--theme-fg-muted,#777)]"><PersonalCollectionArtwork preferFullArtwork={carousel?.focused} source={member.source} key={JSON.stringify(canOpen ? [state.result?.target, state.result?.artwork] : null)} target={canOpen ? state.result?.target ?? null : null} artwork={canOpen ? state.result?.artwork ?? null : null} /></span>
              <span className="album-card-caption block truncate text-sm font-semibold" title={member.title}>{member.title}</span>
              {member.artist && <span className="album-card-caption mt-0.5 block truncate text-xs text-[color:var(--theme-fg-secondary,#999)]" title={member.artist}>{member.artist}</span>}
            </button>
            <div className="mt-3 flex items-center justify-between gap-2 border-t border-[color:var(--theme-border-soft,#303030)] pt-2">
              <span className="inline-flex min-w-0 items-center gap-1.5 text-[11px] text-[color:var(--theme-fg-secondary,#aaa)]"><Icon size={12} /><span className="truncate">{t(`collections.sources.${member.source}`)}</span></span>
              <span className={`shrink-0 text-[10px] font-semibold ${status === "available" ? "text-emerald-400" : status === "unavailable" ? "text-amber-300" : status === "unresolved" || state?.error ? "text-[color:var(--theme-fg-muted,#888)]" : "text-[color:var(--theme-fg-secondary,#aaa)]"}`} role="status">{state?.error ? t("collections.status.checkFailed") : status ? t(`collections.status.${status}`) : <LoaderCircle size={12} className="animate-spin" />}</span>
            </div>
            {status === "unresolved" && <p className="mt-2 text-[10px] leading-relaxed text-[color:var(--theme-fg-muted,#888)]">{t("collections.status.unresolvedHint")}</p>}
            <button type="button" disabled={busy} onClick={() => void removeMember(member)} className="mt-2 w-full rounded-md px-2 py-1.5 text-left text-[11px] text-[color:var(--theme-fg-secondary,#aaa)] hover:bg-[var(--theme-surface-raised,#242424)] hover:text-[color:var(--theme-fg-error,#f99)] disabled:opacity-40">{t("collections.removeAlbum")}</button>
          </article>;
  };

  const collectionHeader = <header className="flex flex-wrap items-start justify-between gap-4 border-b border-[color:var(--theme-border-soft,#262626)] px-6 py-6 sm:px-8">
    <div className="flex min-w-0 items-start gap-3">
      {selected ? <button type="button" onClick={back} aria-label={t("collections.back")} className="mt-1 rounded-lg p-2 text-[color:var(--theme-fg-secondary,#aaa)] hover:bg-[var(--theme-surface-raised,#252525)]"><ArrowLeft size={17} /></button> : <span className="mt-1 rounded-xl bg-[var(--theme-surface-raised,#242424)] p-2.5 text-[color:var(--theme-fg-accent,#E5A00D)]"><Layers3 size={21} /></span>}
      <div className="min-w-0"><h1 className="truncate text-2xl font-bold tracking-tight">{selected?.name ?? t("collections.title")}</h1><p className="mt-1 text-sm text-[color:var(--theme-fg-secondary,#aaa)]">{selected ? t("collections.albumCount", { count: members.length }) : t("collections.description")}</p></div>
    </div>
    {selected ? <div className="flex flex-wrap shrink-0 gap-2"><AlbumViewControls collectionsOnly value={presentation} onChange={changePresentation} /><button type="button" disabled={busy} onClick={() => void rename()} className="inline-flex items-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#383838)] px-3 py-2 text-xs font-semibold hover:bg-[var(--theme-surface-raised,#252525)] disabled:opacity-40"><Pencil size={14} />{t("collections.rename")}</button><button type="button" disabled={busy} onClick={() => void removeCollection()} className="inline-flex items-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#383838)] px-3 py-2 text-xs font-semibold text-[color:var(--theme-fg-error,#f99)] hover:bg-[var(--theme-surface-raised,#252525)] disabled:opacity-40"><Trash2 size={14} />{t("collections.delete")}</button></div> : <form onSubmit={event => void create(event)} className="flex w-full flex-wrap gap-2 sm:w-auto"><input value={name} maxLength={256} onChange={event => setName(event.target.value)} aria-label={t("collections.namePlaceholder")} placeholder={t("collections.namePlaceholder")} className="min-w-48 flex-1 rounded-lg border border-[color:var(--theme-border-soft,#383838)] bg-[var(--theme-surface-panel,#191919)] px-3 py-2 text-sm outline-none focus:border-[color:var(--theme-border-accent,#E5A00D)] sm:w-56" /><button type="submit" disabled={busy || !name.trim()} className="inline-flex items-center gap-2 rounded-lg bg-[var(--theme-control-accent,#E5A00D)] px-3 py-2 text-xs font-bold text-[color:var(--theme-control-on-accent,#000)] disabled:opacity-40">{busy ? <LoaderCircle size={15} className="animate-spin" /> : <Plus size={15} />}{t("collections.create")}</button></form>}
  </header>;

  return <main className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-[var(--theme-surface-base,#121212)]">
    {collectionHeader}
    {error && <p role="alert" className="mx-6 mt-4 rounded-lg border border-[color:var(--theme-border-soft,#444)] px-4 py-3 text-sm text-[color:var(--theme-fg-error,#f99)] sm:mx-8">{error}</p>}
    <div ref={setScrollContainer} className="min-h-0 flex-1 overflow-y-auto p-6 sm:p-8">
      {loading && !selected && <p role="status" className="flex items-center gap-2 py-10 text-sm text-[color:var(--theme-fg-secondary,#aaa)]"><LoaderCircle size={16} className="animate-spin" />{t("collections.loading")}</p>}
      {!selected && !loading && !error && collections.length === 0 && <div className="flex min-h-64 flex-col items-center justify-center text-center"><Layers3 size={38} className="mb-3 text-[color:var(--theme-fg-muted,#777)]" /><p className="text-sm font-semibold">{t("collections.empty")}</p><p className="mt-1 text-xs text-[color:var(--theme-fg-secondary,#999)]">{t("collections.createHint")}</p></div>}
      {!selected && collections.length > 0 && <div className="grid grid-cols-[repeat(auto-fill,minmax(190px,1fr))] gap-4">{collections.map(collection => <button key={collection.id} type="button" onClick={() => void openCollection(collection)} className="group rounded-xl border border-[color:var(--theme-border-soft,#303030)] bg-[var(--theme-surface-panel,#191919)] p-4 text-left transition-colors hover:border-[color:var(--theme-border-accent,#E5A00D)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]"><span className="mb-4 flex aspect-[1.6] items-center justify-center rounded-lg bg-[var(--theme-surface-raised,#242424)] text-[color:var(--theme-fg-accent,#E5A00D)]"><Layers3 size={34} className="transition-transform group-hover:scale-105" /></span><span className="block truncate text-sm font-bold">{collection.name}</span><span className="mt-1 block text-xs text-[color:var(--theme-fg-secondary,#999)]">{counts[collection.id] == null ? "—" : t("collections.albumCount", { count: counts[collection.id] })}</span></button>)}</div>}
      {selected && <>
        {membersLoading && <p role="status" className="flex items-center gap-2 py-6 text-sm text-[color:var(--theme-fg-secondary,#aaa)]"><LoaderCircle size={16} className="animate-spin" />{t("collections.loading")}</p>}
        {!membersLoading && !error && members.length === 0 && <div className="flex min-h-48 flex-col items-center justify-center text-center"><Disc3 size={34} className="mb-3 text-[color:var(--theme-fg-muted,#777)]" /><p className="text-sm">{t("collections.emptyMembers")}</p></div>}
        <div ref={gridRef}>
          {presentation.layout === "grid" ? <div className="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-4">{members.map(member => renderMember(member))}</div> :
            <AlbumCatalog items={members} getItemKey={member => member.id} renderItem={renderMember} presentation={presentation} scrollContainer={scrollContainer} cardExtraHeight={190} resetKey={String(selected.id)} />}
        </div>
      </>}
    </div>
  </main>;
}
