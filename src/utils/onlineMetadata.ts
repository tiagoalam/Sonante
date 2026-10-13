import type { LocalAlbumTagInspection } from "../types/localTags";
import type { OnlineMetadataDetails, OnlineMetadataError, OnlineMetadataQuery, OnlineMetadataResult } from "../types/onlineMetadata";

import type { LocalAlbumTagEditorDraft } from "./localAlbumTagEditor";
import { suggestionValue, transferReason } from "./onlineMetadataTransfer.ts";
import type { DateSource, MetadataTransferRequest, TransferField, TransferValues } from "./onlineMetadataTransfer.ts";

const validMbid = (id: string) => /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id);

export function onlineMetadataSeed(inspection: LocalAlbumTagInspection | null): OnlineMetadataQuery {
  const common = (field: "album" | "album_artist") => inspection?.fields[field].state === "common"
    ? inspection.fields[field].value ?? "" : "";
  return { title: common("album"), artist: common("album_artist") };
}

export function onlineMetadataError(cause: unknown): OnlineMetadataError {
  return typeof cause === "string" && ["invalid_query", "service_unavailable", "rate_limited", "timeout",
    "http_error", "invalid_response", "response_too_large", "invalid_mbid", "release_not_found", "identity_mismatch"].includes(cause) ? cause as OnlineMetadataError : "unknown";
}

export interface OnlineMetadataContext {
  albumId: string;
  catalogVersion: unknown;
  inspection: LocalAlbumTagInspection | null;
  available: boolean;
  draft?: LocalAlbumTagEditorDraft;
  transferAvailable?: boolean;
  editorRevision?: string;
}
interface State {
  query: OnlineMetadataQuery;
  phase: "idle" | "loading" | "ready" | "error";
  result: OnlineMetadataResult | null;
  error: OnlineMetadataError | null;
  inFlight: boolean;
  candidateId: string | null;
  details: OnlineMetadataDetails | null;
  detailPhase: "idle" | "loading" | "ready" | "error";
  detailError: OnlineMetadataError | null;
  choices: TransferField[];
  dateSource: DateSource | null;
}

export function createOnlineMetadataSession(
  load: (query: OnlineMetadataQuery) => Promise<OnlineMetadataResult>,
  isAvailable: () => boolean,
  loadDetails?: (mbid: string) => Promise<OnlineMetadataDetails>,
) {
  let generation = 0, active = true, context: OnlineMetadataContext | null = null;
  const emptyComparison = { candidateId: null, details: null, detailPhase: "idle" as const,
    detailError: null, choices: [] as TransferField[], dateSource: null };
  let state: State = { query: { title: "", artist: "" }, phase: "idle", result: null, error: null, inFlight: false, ...emptyComparison };
  const listeners = new Set<() => void>();
  const update = (patch: Partial<State>, notify = true) => {
    state = { ...state, ...patch };
    if (active && notify) listeners.forEach(listener => listener());
  };
  const invalidate = (notify = true) => {
    generation++;
    update({ result: null, error: null, phase: "idle", ...emptyComparison }, notify);
  };
  const currentDetails = () => active && !state.inFlight && context?.available && isAvailable()
    && state.detailPhase === "ready" && state.details && state.details.id.toLowerCase() === state.candidateId?.toLowerCase()
    && state.result?.candidates.some(candidate => candidate.id === state.candidateId);
  return {
    getSnapshot: () => state,
    subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener); }; },
    syncContext(next: OnlineMetadataContext, notify = true) {
      if (context && next.albumId === context.albumId && next.catalogVersion === context.catalogVersion
        && next.inspection === context.inspection && next.available === context.available) {
        const changed = next.draft !== context.draft || next.editorRevision !== context.editorRevision
          || next.transferAvailable !== context.transferAvailable;
        context = next;
        if (changed) {
          generation++;
          update({ choices: [], dateSource: null, ...(state.detailPhase === "loading" ? emptyComparison : {}) }, notify);
        }
        return;
      }
      const seed = !context || next.albumId !== context.albumId || next.inspection !== context.inspection;
      context = next;
      invalidate(false);
      update(seed ? { query: onlineMetadataSeed(next.inspection?.album_id === next.albumId ? next.inspection : null) } : {}, notify);
    },
    setQuery(field: keyof OnlineMetadataQuery, value: string) {
      if (state.query[field] === value) return;
      invalidate(false);
      update({ query: { ...state.query, [field]: value } });
    },
    activate() { active = true; },
    close() { invalidate(); active = false; },
    async compare(id: string) {
      if (!active || state.inFlight || !context?.available || !isAvailable() || !loadDetails
        || !state.result?.candidates.some(candidate => candidate.id === id)) return;
      if (!validMbid(id)) { update({ ...emptyComparison, detailPhase: "error", detailError: "invalid_mbid" }); return; }
      const request = ++generation;
      update({ ...emptyComparison, candidateId: id, detailPhase: "loading", inFlight: true });
      const current = () => active && request === generation && context?.available && isAvailable();
      try {
        const details = await loadDetails(id);
        if (!current()) return;
        if (!details || typeof details.id !== "string" || !validMbid(details.id)
          || [details.title, details.artist_credit, details.edition_date, details.release_group_id, details.first_release_date]
            .some(value => value !== null && typeof value !== "string")
          || (details.release_group_id !== null && !validMbid(details.release_group_id))
          || (details.first_release_date !== null && details.release_group_id === null)) {
          update({ detailPhase: "error", detailError: "invalid_response" }); return;
        }
        if (details.id.toLowerCase() !== id.toLowerCase()) {
          update({ detailPhase: "error", detailError: "identity_mismatch" }); return;
        }
        update({ details, detailPhase: "ready" });
      } catch (cause) {
        if (current()) update({ detailError: onlineMetadataError(cause), detailPhase: "error" });
      } finally {
        update({ inFlight: false, ...(state.detailPhase === "loading" ? { detailPhase: "idle" as const } : {}) });
      }
    },
    chooseDateSource(source: DateSource) {
      if (!currentDetails() || !context?.transferAvailable || transferReason(suggestionValue(state.details!, "date", source), true)) return;
      update({ dateSource: source, choices: state.choices.filter(field => field !== "date") });
    },
    choose(field: TransferField, checked: boolean) {
      if (!currentDetails() || !context?.transferAvailable || (field === "date" && !state.dateSource)
        || transferReason(suggestionValue(state.details!, field, state.dateSource), field === "date")) return;
      update({ choices: checked ? [...state.choices.filter(value => value !== field), field] : state.choices.filter(value => value !== field) });
    },
    transfer(accept: (request: MetadataTransferRequest) => boolean) {
      if (!currentDetails() || !context?.transferAvailable || !context.draft || context.inspection !== context.draft.inspection
        || context.inspection.album_id !== context.albumId || !state.choices.length) return false;
      const values: TransferValues = {};
      for (const field of state.choices) {
        if (field === "date" && !state.dateSource) return false;
        const value = suggestionValue(state.details!, field, state.dateSource);
        if (transferReason(value, field === "date")) return false;
        values[field] = value!;
      }
      const requestGeneration = generation, requestContext = context, details = state.details!;
      const accepted = accept({ albumId: context.albumId, catalogVersion: context.catalogVersion, inspection: context.inspection,
        draft: context.draft, editorRevision: context.editorRevision, details, values,
        isCurrent: () => generation === requestGeneration && context === requestContext && state.details === details
          && !!currentDetails() && !!context?.transferAvailable });
      if (accepted) { generation++; update({ choices: [], dateSource: null }); }
      return accepted;
    },
    async search() {
      if (!active || state.inFlight || !context?.available || !isAvailable()) return;
      const query = { ...state.query };
      if (!query.title.trim() || [query.title, query.artist].some(value => value.length > 256 || /[\u0000-\u001f\u007f]/.test(value))) {
        invalidate(false); update({ phase: "error", error: "invalid_query" }); return;
      }
      const request = ++generation;
      update({ inFlight: true, phase: "loading", result: null, error: null, ...emptyComparison });
      const current = () => active && request === generation && context?.available && isAvailable();
      try {
        const result = await load(query);
        if (!current()) return;
        if (!result || !Array.isArray(result.candidates) || result.candidates.length > 10
          || !Number.isSafeInteger(result.total) || result.total < result.candidates.length || typeof result.limited !== "boolean") {
          update({ phase: "error", error: "invalid_response" }); return;
        }
        update({ result, phase: "ready" });
      } catch (cause) {
        if (current()) update({ error: onlineMetadataError(cause), phase: "error" });
      } finally {
        // Invalidation discards replies; it does not cancel the backend HTTP read.
        update({ inFlight: false, ...(state.phase === "loading" ? { phase: "idle" as const } : {}) });
      }
    },
  };
}
