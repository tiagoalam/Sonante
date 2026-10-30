import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { logError } from "../utils/frontendDiagnostics";
import { libraryRecoveryService, diagnosticKeyForAnalysisId, extractFiniteOfferAssessment } from "../services/libraryRecovery";
import {
  FAILURE_CLASSES, FAILURE_BOUNDARIES, FAILURE_KNOWLEDGE, POSSIBLE_EFFECTS,
  INHERITED_OBSERVED, FRESHNESS, OUTCOMES, UNAVAILABLE_REASONS,
  ASSESSMENT_STATUSES, ASSESSMENT_REASONS, ASSESSMENT_ROLES,
} from "../types/libraryRecovery";
import type { RecoveryProgress, AttemptDiagnostic, SafeOfferAssessment, LibraryRecoveryObservation, LibraryRecoveryResult } from "../types/libraryRecovery";

type Phase = "idle" | "analyzing" | "ready" | "exporting" | "executing" | "reopening";
type Notice = "failed" | "exportFailed" | "exported" | "cancelFailed" | "executionFailed" | "reopenFailed" | "analysisCancelled" | "executionCancelled" | "executionCancelFailed" | null;
type Offer = { observation: LibraryRecoveryObservation; visit: number };
type AnalysisSnapshot = Offer & { assessment: SafeOfferAssessment | undefined; prior: boolean };
type AttemptContext = { analysisId: number; safeKey: string | null; visit: number; queryIssued: boolean };
type DiagnosticSnapshot = { context: AttemptContext; value: AttemptDiagnostic; prior: boolean; availabilityNotice: "unknown" | "unavailable" | null };
const CHOICES = ["restore_previous", "finish_confirmed"] as const;
const CONFIG_MATCHES = ["previous", "next", "both", "unbound", "unknown"] as const;
const ANALYSIS_REASONS = ["intent_valid", "intent_invalid", "orphan_staging", "config_missing", "config_invalid", "config_divergent", "mirror_missing", "mirror_invalid", "generation_divergent", "database_missing", "database_unproven", "ledger_missing", "ledger_divergent", "unsafe_entry", "filesystem_changed", "ownership_unknown", "no_pending_marker", "recovery_set_unproven"] as const;

export function LibraryRecoveryPanel() {
  const { t } = useTranslation();
  const [phase, setPhase] = useState<Phase>("idle");
  const [progressStage, setProgressStage] = useState<RecoveryProgress>("checking");
  const [result, setResult] = useState<LibraryRecoveryResult | null>(null);
  const completedPlan = useRef<string | null>(null);
  const [offer, setOffer] = useState<Offer | null>(null);
  const offerRef = useRef<Offer | null>(null);
  const [snapshot, setSnapshot] = useState<AnalysisSnapshot | null>(null);
  const [diagnostic, setDiagnostic] = useState<DiagnosticSnapshot | null>(null);
  const attempt = useRef<AttemptContext | null>(null);
  const [sessionUnconfirmed, setSessionUnconfirmed] = useState(false);
  const [notice, setNotice] = useState<Notice>(null);
  const visit = useRef(0);
  const current = useRef<string | null>(null);
  const active = useRef(false);
  const operation = useRef<"analysis" | "execution" | "export" | "reopen" | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    return () => {
      visit.current++;
      offerRef.current = null; attempt.current = null; operation.current = null;
      const id = current.current; current.current = null;
      if (id) void libraryRecoveryService.cancel(id).catch(() => { logError("libraryRecovery.cancel", "not_confirmed"); });
    };
  }, []);

  const clearActions = () => {
    offerRef.current = null; setOffer(null); setResult(null); completedPlan.current = null;
  };
  const keepAsPrior = () => {
    setSnapshot(previous => previous ? { ...previous, prior: true } : null);
    setDiagnostic(previous => previous ? { ...previous, prior: true } : null);
  };
  const queryAttempt = async (context: AttemptContext) => {
    if (context.queryIssued || visit.current !== context.visit || attempt.current !== context) return;
    context.queryIssued = true; // Synchronous once-only gate, before invoking the service.
    const apply = (value: AttemptDiagnostic) => {
      if (visit.current !== context.visit || attempt.current !== context) return;
      const safe = value.kind === "unknown" || value.association === context.safeKey ? value : { kind: "unknown" } as const;
      setDiagnostic(previous => {
        if (visit.current !== context.visit || attempt.current !== context) return previous;
        const same = previous?.context.analysisId === context.analysisId && previous.context.safeKey === context.safeKey;
        if (same && previous.value.kind === "available" && previous.value.outcome === "failed" &&
            (safe.kind !== "available" || safe.outcome !== "failed")) {
          return { ...previous, context, prior: false, availabilityNotice: safe.kind === "unavailable" ? "unavailable" : "unknown" };
        }
        return { context, value: safe, prior: false, availabilityNotice: null };
      });
    };
    try { apply(await libraryRecoveryService.diagnostic(context.analysisId)); }
    catch { apply({ kind: "unknown" }); }
    // Never changes active/phase, result/completedPlan/offer, and never retries.
  };
  const analyze = async () => {
    if (active.current) return;
    active.current = true; operation.current = "analysis";
    const epoch = ++visit.current;
    attempt.current = null; clearActions(); keepAsPrior(); setNotice(null); setPhase("analyzing");
    try {
      const id = crypto.randomUUID(); current.current = id;
      const observation = await libraryRecoveryService.analyze(id);
      if (visit.current !== epoch) return;
      const next: AnalysisSnapshot = { observation, assessment: extractFiniteOfferAssessment(observation), visit: epoch, prior: false };
      setSnapshot(next); setDiagnostic(null); setSessionUnconfirmed(false);
      if (observation.recovery_available && observation.plan) {
        const nextOffer = { observation, visit: epoch }; offerRef.current = nextOffer; setOffer(nextOffer);
      }
      setPhase("ready"); heading.current?.focus();
    } catch {
      if (visit.current === epoch) { clearActions(); setNotice("failed"); setPhase("idle"); }
    } finally {
      if (visit.current === epoch) { active.current = false; operation.current = null; }
    }
  };
  const cancel = async () => {
    const kind = operation.current;
    if (kind !== "analysis" && kind !== "execution") return;
    const previous = kind === "execution" ? attempt.current : null;
    const epoch = ++visit.current; // Fence before cancel IPC or any await.
    const context: AttemptContext | null = previous ? { analysisId: previous.analysisId, safeKey: previous.safeKey, visit: epoch, queryIssued: false } : null;
    attempt.current = context;
    const id = current.current; current.current = null;
    clearActions(); keepAsPrior(); active.current = false; operation.current = null;
    setPhase("idle"); setNotice(kind === "execution" ? "executionCancelled" : "analysisCancelled");
    if (kind === "execution") setSessionUnconfirmed(true);
    let failed = id === null;
    try { if (id) await libraryRecoveryService.cancel(id); }
    catch { failed = true; }
    finally {
      if (visit.current === epoch) {
        if (failed) setNotice(kind === "execution" ? "executionCancelFailed" : "cancelFailed");
        if (context) void queryAttempt(context); // Owns the new visit, even if cancel rejects.
      }
    }
  };
  const exportReport = async () => {
    const selected = snapshot;
    if (active.current || !selected || selected.prior || selected.visit !== visit.current || result) return;
    active.current = true; operation.current = "export";
    const epoch = visit.current; let failed = false;
    setPhase("exporting"); setNotice(null);
    try {
      const saved = await libraryRecoveryService.exportReport(selected.observation.analysis_id);
      if (visit.current === epoch && saved) setNotice("exported");
    } catch {
      if (visit.current === epoch) { failed = true; clearActions(); keepAsPrior(); setNotice("exportFailed"); }
    } finally {
      if (visit.current === epoch) { active.current = false; operation.current = null; setPhase(failed ? "idle" : "ready"); }
    }
  };
  const execute = async () => {
    const selected = offerRef.current;
    const plan = selected?.observation.plan;
    if (active.current || !selected || selected.visit !== visit.current || !selected.observation.recovery_available || !plan) return;
    active.current = true; operation.current = "execution";
    const epoch = ++visit.current;
    const context: AttemptContext = { analysisId: selected.observation.analysis_id, safeKey: diagnosticKeyForAnalysisId(selected.observation.analysis_id), visit: epoch, queryIssued: false };
    attempt.current = context;
    clearActions(); keepAsPrior(); setProgressStage("checking"); setPhase("executing"); setNotice(null); setSessionUnconfirmed(true);
    // Offer was consumed synchronously before the first await. Backend gates remain decisive.
    try {
      const outcome = await libraryRecoveryService.execute(context.analysisId, plan.id, (stage) => {
        if (visit.current === epoch && active.current && operation.current === "execution") setProgressStage(stage);
      });
      if (visit.current !== epoch) return;
      if (outcome.completed !== true || outcome.backup_retained !== true || outcome.requires_reopen !== true) throw new Error("unconfirmed");
      completedPlan.current = plan.id;
      setResult({ completed: true, backup_retained: true, requires_reopen: true });
      setSessionUnconfirmed(false); setPhase("ready"); heading.current?.focus();
    } catch {
      if (visit.current !== epoch) return; // Cancellation owns its query; this catch must not query.
      clearActions(); setNotice("executionFailed"); setPhase("idle"); setSessionUnconfirmed(true);
      void queryAttempt(context);
    } finally {
      if (visit.current === epoch) { active.current = false; operation.current = null; }
    }
  };
  const reopen = async () => {
    if (active.current || !result?.completed || !completedPlan.current) return;
    active.current = true; operation.current = "reopen"; setPhase("reopening"); setNotice(null);
    const epoch = visit.current;
    try { await libraryRecoveryService.reopen(completedPlan.current); }
    catch {
      if (visit.current === epoch) { clearActions(); keepAsPrior(); setNotice("reopenFailed"); setPhase("idle"); active.current = false; operation.current = null; }
    }
  };
  const label = (group: string, value: unknown, allowed: readonly string[]) => {
    const trusted = allowed.find(item => item === value);
    return trusted === undefined ? t("libraryRecovery.diagnostic.unknown") : t(`libraryRecovery.${group}.${trusted}`);
  };
  const observation = snapshot?.observation;
  const assessment = snapshot?.assessment;
  const diag = diagnostic?.value;
  const failure = diag?.kind === "available" && diag.outcome === "failed" ? diag.failure : null;
  const hasOffer = offer !== null && offer.visit === visit.current && offer.observation.recovery_available && !!offer.observation.plan;
  const reportCurrent = snapshot !== null && !snapshot.prior && snapshot.visit === visit.current && !result;
  const reasons = observation && Array.isArray(observation.reasons) ? ANALYSIS_REASONS.filter(reason => observation.reasons.includes(reason)) : [];
  const countsKnown = observation?.plan && Number.isSafeInteger(observation.plan.previous_tracks) && observation.plan.previous_tracks >= 0 && Number.isSafeInteger(observation.plan.backup_bytes) && observation.plan.backup_bytes >= 0;
  return (
    <section aria-labelledby="library-recovery-title" className="mt-6 space-y-4 text-left">
      <h2 id="library-recovery-title" ref={heading} tabIndex={-1} className="text-base font-semibold">{t("libraryRecovery.title")}</h2>
      <p className="text-sm">{t("libraryRecovery.explanation")}</p>
      <div role="status" aria-live="polite" aria-busy={phase === "executing"} className="text-sm">
        {phase === "analyzing" ? t("libraryRecovery.analyzing") : phase === "exporting" ? t("libraryRecovery.exporting") : phase === "executing" ? t(`libraryRecovery.progress.${progressStage}`) : phase === "reopening" ? t("libraryRecovery.reopening") : notice ? t(`libraryRecovery.${notice}`) : null}
      </div>
      {result?.completed && <div className="space-y-2 text-sm" role="status">
        <p className="font-semibold">{t("libraryRecovery.completed")}</p>
        <p>{t("libraryRecovery.backupRetained")}</p>
      </div>}
      {sessionUnconfirmed && phase !== "executing" && <p className="text-sm">{t("libraryRecovery.sessionUnconfirmed")}</p>}
      {observation && !result && <div className="space-y-3 text-sm">
        {snapshot?.prior && <p className="font-semibold">{t("libraryRecovery.priorAnalysis")}</p>}
        {!snapshot?.prior && <p className="font-semibold">{t(hasOffer ? "libraryRecovery.planReady" : "libraryRecovery.unavailable")}</p>}
        {hasOffer && observation.plan && <div className="space-y-2 rounded border border-current p-3">
          <h3 className="font-semibold">{label("choices", observation.plan.choice, CHOICES)}</h3>
          <p>{t("libraryRecovery.summaryImpact")}</p>
          {observation.plan.resumes_interrupted && <p>{t("libraryRecovery.resumePlan")}</p>}
        </div>}
        <details>
          <summary className="cursor-pointer">{t("libraryRecovery.details")}</summary>
          {hasOffer && observation.plan && <div className="mt-2 space-y-2">
            <p>{t("libraryRecovery.impact")}</p>
            <p>{countsKnown ? t("libraryRecovery.proof", { tracks: observation.plan.previous_tracks, bytes: observation.plan.backup_bytes }) : t("libraryRecovery.proofUnavailable")}</p>
            <p>{t("libraryRecovery.noMusicBackup")}</p>
          </div>}
        <ul className="list-disc space-y-2 pl-5">{reasons.map(reason => <li key={reason}>{label("reasons", reason, ANALYSIS_REASONS)}</li>)}</ul>
        {assessment && <div className="space-y-2">
          <p>{label("assessment.status", assessment.status, ASSESSMENT_STATUSES)}</p>
          {assessment.reason !== null && <p>{label("assessment.reason", assessment.reason, ASSESSMENT_REASONS)}</p>}
          {assessment.targetRole !== null && <p>{label("assessment.role", assessment.targetRole, ASSESSMENT_ROLES)}</p>}
          <p>{t("libraryRecovery.assessment.scope")}</p>
        </div>}
        <p>{t("libraryRecovery.guidance")}</p>
          <p className="mt-2">{label("config", observation.config_matches, CONFIG_MATCHES)}</p>
          {typeof observation.intent_version === "number" && Number.isSafeInteger(observation.intent_version) && observation.intent_version >= 0 && <p>{t("libraryRecovery.intentVersion", { version: observation.intent_version })}</p>}
          {typeof observation.queue_backup_version === "number" && Number.isSafeInteger(observation.queue_backup_version) && observation.queue_backup_version >= 0 && <p>{t("libraryRecovery.queueVersion", { version: observation.queue_backup_version })}</p>}
          <p>{t("libraryRecovery.observationOnly")}</p>
          <p>{t("libraryRecovery.reportV1")}</p>
          {snapshot?.prior && <p>{t("libraryRecovery.reportPriorRefused")}</p>}
          {reportCurrent && <button type="button" disabled={phase !== "ready"} onClick={() => void exportReport()} className="mt-2 rounded border border-current px-4 py-2 text-sm disabled:opacity-50">{t("libraryRecovery.export")}</button>}
        </details>
      </div>}
      {diagnostic && <details className="space-y-2 text-sm">
        <summary className="cursor-pointer">{t("libraryRecovery.diagnostic.title")}</summary>
        {diagnostic.prior && <p>{t("libraryRecovery.diagnostic.prior")}</p>}
        <p>{t("libraryRecovery.diagnostic.informationOnly")}</p>
        {diag?.kind === "unknown" && <p>{t("libraryRecovery.diagnostic.unknown")}</p>}
        {diag?.kind === "unavailable" && <p>{label("diagnostic.unavailableReason", diag.reason, UNAVAILABLE_REASONS)}</p>}
        {diag?.kind === "available" && <>
          <p>{label("diagnostic.outcome", diag.outcome, OUTCOMES)}</p>
          <p>{label("diagnostic.freshness", diag.freshness, FRESHNESS)}</p>
        </>}
        {failure && <>
          <p>{label("diagnostic.failureClass", failure.class, FAILURE_CLASSES)}</p>
          <p>{label("diagnostic.boundary", failure.boundary, FAILURE_BOUNDARIES)}</p>
          <p>{label("diagnostic.knowledge", failure.knowledge, FAILURE_KNOWLEDGE)}</p>
          {failure.knowledge === "unknown_boundary" ? <p>{t("libraryRecovery.diagnostic.unknownEffects")}</p> : <>
            <p>{t("libraryRecovery.diagnostic.mayOnly")}</p>
            <ul className="list-disc space-y-2 pl-5">{failure.possibleEffects.map(effect => <li key={effect}>{label("diagnostic.effect", effect, POSSIBLE_EFFECTS)}</li>)}</ul>
            <ul className="list-disc space-y-2 pl-5">{failure.inheritedObserved.map(fact => <li key={fact}>{label("diagnostic.inherited", fact, INHERITED_OBSERVED)}</li>)}</ul>
          </>}
        </>}
        {diagnostic.availabilityNotice && <p>{t("libraryRecovery.diagnostic.queryUnavailable")}</p>}
        <p>{t("libraryRecovery.diagnostic.guidance")}</p>
      </details>}
      <div className="flex flex-wrap gap-3">
        <button type="button" disabled={phase === "analyzing" || phase === "exporting" || phase === "executing" || phase === "reopening"} onClick={() => void analyze()}
          className="rounded border border-current px-4 py-2 text-sm disabled:opacity-50">
          {t(snapshot || diagnostic ? "libraryRecovery.again" : "libraryRecovery.analyze")}
        </button>
        {(phase === "analyzing" || phase === "executing") && <button type="button" onClick={() => void cancel()} className="rounded border border-current px-4 py-2 text-sm">{t(phase === "executing" ? "libraryRecovery.cancelExecution" : "libraryRecovery.cancel")}</button>}
        {hasOffer && !result && <button type="button" disabled={phase !== "ready"} onClick={() => void execute()} className="rounded border border-current px-4 py-2 text-sm disabled:opacity-50">{t("libraryRecovery.execute")}</button>}
        {result?.completed && <button type="button" disabled={phase === "reopening"} onClick={() => void reopen()} className="rounded border border-current px-4 py-2 text-sm disabled:opacity-50">{t("libraryRecovery.reopen")}</button>}
      </div>
    </section>
  );
}
