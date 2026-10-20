import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { logError } from "../utils/frontendDiagnostics";
import { libraryRecoveryService } from "../services/libraryRecovery";
import type { LibraryRecoveryObservation, LibraryRecoveryResult } from "../types/libraryRecovery";

type Phase = "idle" | "analyzing" | "ready" | "exporting" | "executing" | "reopening";

export function LibraryRecoveryPanel() {
  const { t } = useTranslation();
  const [phase, setPhase] = useState<Phase>("idle");
  const [result, setResult] = useState<LibraryRecoveryResult | null>(null);
  const completedPlan = useRef<string | null>(null);
  const [observation, setObservation] = useState<LibraryRecoveryObservation | null>(null);
  const [notice, setNotice] = useState<"failed" | "exportFailed" | "exported" | "cancelFailed" | "executionFailed" | "reopenFailed" | null>(null);
  const visit = useRef(0);
  const current = useRef<string | null>(null);
  const active = useRef(false);
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
  return () => {
      visit.current++;
      const id = current.current;
      current.current = null;
      if (id) void libraryRecoveryService.cancel(id).catch(() => { logError("libraryRecovery.cancel", "not_confirmed"); });
    };
  }, []);

  const analyze = async () => {
    if (active.current) return;
    active.current = true;
    const epoch = ++visit.current;
    const id = crypto.randomUUID();
    current.current = id;
    setResult(null); completedPlan.current = null; setObservation(null); setNotice(null); setPhase("analyzing");
    try {
      const result = await libraryRecoveryService.analyze(id);
      if (visit.current !== epoch) return;
      setObservation(result); setPhase("ready");
      heading.current?.focus();
    } catch {
      if (visit.current === epoch) { setNotice("failed"); setPhase("idle"); }
    } finally {
      if (visit.current === epoch) active.current = false;
    }
  };
  const cancel = async () => {
    // Invalidate the frontend before awaiting IPC. A late readonly result must
    // never become a successful recovery or trigger bootstrap/audio.
    const epoch = ++visit.current;
    const id = current.current; current.current = null;
    active.current = false; setPhase("idle"); setObservation(null); setNotice(null);
    if (id) {
      try { await libraryRecoveryService.cancel(id); }
      catch { if (visit.current === epoch) setNotice("cancelFailed"); }
    }
  };
  const exportReport = async () => {
    if (active.current || !observation) return;
    active.current = true;
    const epoch = visit.current;
    setPhase("exporting"); setNotice(null);
    try {
      const saved = await libraryRecoveryService.exportReport(observation.analysis_id);
      if (visit.current === epoch && saved) setNotice("exported");
    } catch {
      if (visit.current === epoch) setNotice("exportFailed");
    } finally {
      if (visit.current === epoch) { active.current = false; setPhase("ready"); }
    }
  };
  const execute = async () => {
    const plan = observation?.plan;
    if (active.current || !observation?.recovery_available || !plan) return;
    active.current = true;
    const epoch = ++visit.current;
    setPhase("executing"); setNotice(null); setResult(null);
    try {
      const outcome = await libraryRecoveryService.execute(observation.analysis_id, plan.id);
      if (visit.current !== epoch) return;
      if (!outcome.completed || !outcome.backup_retained || !outcome.requires_reopen) throw new Error("unconfirmed");
      completedPlan.current = plan.id; setResult(outcome); setPhase("ready");
      heading.current?.focus();
    } catch {
      if (visit.current === epoch) { setObservation(null); setNotice("executionFailed"); setPhase("idle"); }
    } finally { if (visit.current === epoch) active.current = false; }
  };
  const reopen = async () => {
    if (active.current || !result?.completed || !completedPlan.current) return;
    active.current = true; setPhase("reopening"); setNotice(null);
    const epoch = visit.current;
    try { await libraryRecoveryService.reopen(completedPlan.current); }
    catch { if (visit.current === epoch) { setNotice("reopenFailed"); setPhase("ready"); active.current = false; } }
  };
  return (
    <section aria-labelledby="library-recovery-title" className="mt-6 space-y-4 text-left">
      <h2 id="library-recovery-title" ref={heading} tabIndex={-1} className="text-base font-semibold">{t("libraryRecovery.title")}</h2>
      <p className="text-sm">{t("libraryRecovery.explanation")}</p>
      <div role="status" aria-live="polite" className="text-sm">
        {phase === "analyzing" ? t("libraryRecovery.analyzing") : phase === "exporting" ? t("libraryRecovery.exporting") : phase === "executing" ? t("libraryRecovery.executing") : phase === "reopening" ? t("libraryRecovery.reopening") : notice ? t(`libraryRecovery.${notice}`) : null}
      </div>
      {result?.completed && <div className="space-y-2 text-sm" role="status">
        <p className="font-semibold">{t("libraryRecovery.completed")}</p>
        <p>{t("libraryRecovery.backupRetained")}</p>
      </div>}
      {observation && !result && <div className="space-y-3 text-sm">
        <p className="font-semibold">{t(observation.recovery_available && observation.plan ? "libraryRecovery.planReady" : "libraryRecovery.unavailable")}</p>
        {observation.recovery_available && observation.plan && <div className="space-y-2 rounded border border-current p-3">
          <h3 className="font-semibold">{t(`libraryRecovery.choices.${observation.plan.choice}`)}</h3>
          <p>{t("libraryRecovery.impact")}</p>
          <p>{t("libraryRecovery.proof", { tracks: observation.plan.previous_tracks, bytes: observation.plan.backup_bytes })}</p>
          <p>{t("libraryRecovery.noMusicBackup")}</p>
          {observation.plan.resumes_interrupted && <p>{t("libraryRecovery.resumePlan")}</p>}
        </div>}
        <ul className="list-disc space-y-2 pl-5">{observation.reasons.map(reason => <li key={reason}>{t(`libraryRecovery.reasons.${reason}`)}</li>)}</ul>
        <p>{t("libraryRecovery.guidance")}</p>
        <details>
          <summary className="cursor-pointer">{t("libraryRecovery.details")}</summary>
          <p className="mt-2">{t(`libraryRecovery.config.${observation.config_matches}`)}</p>
          {observation.intent_version !== null && <p>{t("libraryRecovery.intentVersion", { version: observation.intent_version })}</p>}
          {observation.queue_backup_version !== null && <p>{t("libraryRecovery.queueVersion", { version: observation.queue_backup_version })}</p>}
          <p>{t("libraryRecovery.observationOnly")}</p>
        </details>
      </div>}
      <div className="flex flex-wrap gap-3">
        <button type="button" disabled={phase === "analyzing" || phase === "exporting" || phase === "executing" || phase === "reopening"} onClick={() => void analyze()}
          className="rounded border border-current px-4 py-2 text-sm disabled:opacity-50">
          {t(observation ? "libraryRecovery.again" : "libraryRecovery.analyze")}
        </button>
        {(phase === "analyzing" || phase === "executing") && <button type="button" onClick={() => void cancel()} className="rounded border border-current px-4 py-2 text-sm">{t("libraryRecovery.cancel")}</button>}
        {observation && !result && <button type="button" disabled={phase === "exporting" || phase === "executing" || phase === "reopening"} onClick={() => void exportReport()} className="rounded border border-current px-4 py-2 text-sm disabled:opacity-50">{t("libraryRecovery.export")}</button>}
        {observation?.recovery_available && observation.plan && !result && <button type="button" disabled={phase !== "ready"} onClick={() => void execute()} className="rounded border border-current px-4 py-2 text-sm disabled:opacity-50">{t("libraryRecovery.execute")}</button>}
        {result?.completed && <button type="button" disabled={phase === "reopening"} onClick={() => void reopen()} className="rounded border border-current px-4 py-2 text-sm disabled:opacity-50">{t("libraryRecovery.reopen")}</button>}
      </div>
    </section>
  );
}
