import React, { useCallback, useEffect, useId, useMemo, useRef, useSyncExternalStore } from "react";
import { LoaderCircle, RotateCcw, Stethoscope, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { libraryDoctorService } from "../services/libraryDoctor";
import type { DoctorEvidence, DoctorFindingLevel, LibraryDoctorReport } from "../types/libraryDoctor";
import { createLibraryDoctorSession, doctorCoverageState, doctorFindingGroups, doctorValue, installDoctorDialogFocus } from "../utils/libraryDoctor";

interface Props {
  albumId: string;
  displayName: string;
  available: boolean;
  catalogVersion: unknown;
  isAvailable: () => boolean;
  onClose: () => void;
}

const levelStyle: Record<DoctorFindingLevel, string> = {
  advisory: "text-sky-200 bg-sky-400/10",
  inconsistency: "text-amber-200 bg-amber-400/10",
  possible_conflict: "text-violet-200 bg-violet-400/10",
};

const EvidenceTable: React.FC<{ evidence: DoctorEvidence[] }> = ({ evidence }) => {
  const { t } = useTranslation();
  return <div className="overflow-x-auto">
    <table className="w-full table-fixed text-left text-xs">
      <thead className="text-[#999999]"><tr>
        <th className="w-1/2 p-2">{t("libraryDoctor.uri")}</th>
        <th className="w-1/4 p-2">{t("libraryDoctor.field")}</th>
        <th className="w-1/4 p-2">{t("libraryDoctor.originalValue")}</th>
      </tr></thead>
      <tbody>{evidence.map((item, index) => {
        const value = doctorValue(item.value);
        return <tr key={index} className="border-t border-[#303030] align-top">
          <td className="break-all p-2 font-mono text-[#CCCCCC]">{item.uri}</td>
          <td className="break-words p-2">{t(`libraryDoctor.fields.${item.field}`)}</td>
          <td className="break-all p-2 font-mono whitespace-pre-wrap">{value.kind === "null"
            ? t("libraryDoctor.valueNull") : value.kind === "empty" ? t("libraryDoctor.valueEmpty") : value.text}</td>
        </tr>;
      })}</tbody>
    </table>
  </div>;
};

export const LibraryDoctorReportContent: React.FC<{ report: LibraryDoctorReport }> = ({ report }) => {
  const { t } = useTranslation();
  const coverage = report.coverage;
  const coverageState = doctorCoverageState(report);
  return <>
    <section aria-label={t("libraryDoctor.coverage")} className="rounded-xl border border-[#333333] p-4">
      <h3 className="font-bold">{t("libraryDoctor.coverage")}</h3>
      <dl className="mt-3 grid gap-2 text-sm sm:grid-cols-3">
        <div><dt className="text-[#999999]">{t("libraryDoctor.physicalReads")}</dt><dd>{t("libraryDoctor.readCount", { read: coverage.physical_tracks_read, total: coverage.physical_track_count })}</dd></div>
        <div><dt className="text-[#999999]">{t("libraryDoctor.failures")}</dt><dd>{coverage.failed_physical_track_count}</dd></div>
        <div><dt className="text-[#999999]">{t("libraryDoctor.cueTracks")}</dt><dd>{coverage.cue_virtual_count}</dd></div>
      </dl>
      <p className={`mt-3 text-xs ${coverageState === "performed" ? "text-[#AAAAAA]" : "text-amber-200"}`}>
        {t(`libraryDoctor.coverageStates.${coverageState}`)}
      </p>
    </section>
    <section aria-label={t("libraryDoctor.findings")}>
      <h3 className="mb-3 font-bold">{t("libraryDoctor.findings")}</h3>
      {report.findings.length === 0 && <p className="text-sm text-[#CCCCCC]">{t(coverageState === "no_reads"
        ? "libraryDoctor.noReadableConclusion" : "libraryDoctor.noFindings")}</p>}
      <div className="space-y-3">{doctorFindingGroups(report).map(group => <section key={group.code} className="rounded-xl border border-[#333333] p-3">
        <h4 className="mb-2 text-sm font-bold">{t(`libraryDoctor.codes.${group.code}`)}</h4>
        <p className="mb-2 break-all font-mono text-[10px] text-[#888888]">{group.code}</p>
        {group.findings.map((finding, index) => <details key={index} className="border-t border-[#303030] py-2">
          <summary className="cursor-pointer rounded p-1 text-xs focus-visible:outline focus-visible:outline-2 focus-visible:outline-[#E5A00D]">
            <span className={`mr-2 inline-block rounded px-2 py-1 ${levelStyle[finding.level]}`}>{t(`libraryDoctor.levels.${finding.level}`)}</span>
            {t("libraryDoctor.evidenceCount", { count: finding.evidence.length })}
          </summary>
          <EvidenceTable evidence={finding.evidence} />
        </details>)}
      </section>)}</div>
    </section>
    {report.failures.length > 0 && <section aria-label={t("libraryDoctor.fileFailures")}>
      <h3 className="mb-2 font-bold">{t("libraryDoctor.fileFailures")}</h3>
      <ul className="space-y-2">{report.failures.map((failure, index) => <li key={index} className="rounded-lg border border-red-400/20 p-3 text-xs">
        <p className="break-all font-mono">{failure.uri}</p>
        <p className="mt-1 text-red-200">{t(`libraryDoctor.fileErrors.${failure.error.code}`, { defaultValue: t("libraryDoctor.fileErrors.unknown") })}</p>
      </li>)}</ul>
    </section>}
    <section aria-label={t("libraryDoctor.unperformed")}>
      <h3 className="mb-2 font-bold">{t("libraryDoctor.unperformed")}</h3>
      <ul className="list-disc space-y-1 pl-5 text-xs text-[#BBBBBB]">{coverage.checks_not_performed.map(check =>
        <li key={check}>{t(`libraryDoctor.checks.${check}`)}</li>)}</ul>
    </section>
  </>;
};

export const LibraryDoctorModal: React.FC<Props> = ({ albumId, displayName, available, catalogVersion, isAvailable, onClose }) => {
  const { t } = useTranslation();
  const id = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const availabilityRef = useRef(isAvailable);
  const closeRef = useRef(onClose);
  availabilityRef.current = isAvailable;
  closeRef.current = onClose;
  const session = useMemo(() => createLibraryDoctorSession(libraryDoctorService.inspectAlbum, () => availabilityRef.current()), []);
  // Invalidate during this render so old results cannot appear even for a frame.
  session.syncContext(albumId, catalogVersion, available, false);
  const state = useSyncExternalStore(session.subscribe, session.getSnapshot, session.getSnapshot);
  const close = useCallback(() => { session.close(); closeRef.current(); }, [session]);

  useEffect(() => {
    session.activate();
    let mounted = true;
    // StrictMode's setup/cleanup replay must not dispatch two IPC reads.
    queueMicrotask(() => { if (mounted) void session.inspect(); });
    return () => { mounted = false; session.close(); };
  }, [session]);
  useEffect(() => {
    if (dialogRef.current) return installDoctorDialogFocus(dialogRef.current, close);
  }, [close]);

  return <div className="fixed inset-0 z-[80] flex items-center justify-center bg-black/75 p-3" onClick={event => { if (event.target === event.currentTarget) close(); }}>
    <div ref={dialogRef} role="dialog" aria-modal="true" aria-labelledby={`${id}-title`} aria-describedby={`${id}-limits`} tabIndex={-1}
      className="flex max-h-[92vh] w-full max-w-3xl flex-col overflow-hidden rounded-2xl border border-[#333333] bg-[#171717] text-white shadow-2xl">
      <header className="flex shrink-0 items-start justify-between gap-3 border-b border-[#303030] p-5">
        <div className="min-w-0"><h2 id={`${id}-title`} className="flex items-center gap-2 font-bold"><Stethoscope size={18} />{t("libraryDoctor.title")}</h2>
          <p className="mt-1 truncate text-xs text-[#999999]">{displayName}</p></div>
        <button type="button" onClick={close} aria-label={t("libraryDoctor.close")} className="rounded-lg p-2 hover:bg-[#303030]"><X size={18} /></button>
      </header>
      <div className="min-h-0 space-y-5 overflow-y-auto p-5">
        {state.phase === "loading" && <p role="status" className="flex items-center gap-2 text-sm"><LoaderCircle size={16} className="animate-spin" />{t("libraryDoctor.loading")}</p>}
        {(state.phase === "stale" || state.phase === "unavailable") && <p role="status" className="text-sm text-amber-200">{t(state.phase === "stale" ? "libraryDoctor.stale" : "libraryDoctor.unavailable")}</p>}
        {state.inFlight && state.phase !== "loading" && <p role="status" className="text-xs text-[#BBBBBB]">{t("libraryDoctor.pendingDiscard")}</p>}
        {state.error && <p role="alert" className="text-sm text-red-200">{t(`libraryDoctor.errors.${state.error}`)}</p>}
        {state.report && <LibraryDoctorReportContent report={state.report} />}
        <section id={`${id}-limits`} className="rounded-xl bg-[#222222] p-4 text-xs text-[#BBBBBB]">
          <h3 className="mb-2 font-bold text-[#DDDDDD]">{t("libraryDoctor.limitsTitle")}</h3>
          <ul className="list-disc space-y-2 pl-4">
            <li>{t("libraryDoctor.limits.membership")}</li>
            <li>{t("libraryDoctor.limits.dates")}</li>
            <li>{t("libraryDoctor.limits.numbers")}</li>
            <li>{t("libraryDoctor.limits.conflicts")}</li>
            <li>{t("libraryDoctor.limits.cue")}</li>
            <li>{t("libraryDoctor.limits.snapshot")}</li>
            <li>{t("libraryDoctor.limits.invalidation")}</li>
          </ul>
        </section>
      </div>
      <footer className="flex shrink-0 flex-wrap justify-end gap-3 border-t border-[#303030] p-4">
        <button type="button" disabled={!available || state.inFlight} onClick={() => void session.inspect()}
          className="flex items-center gap-2 rounded-lg border border-[#444444] px-4 py-2 text-xs hover:bg-[#303030] disabled:cursor-not-allowed disabled:opacity-40"><RotateCcw size={14} />{t("libraryDoctor.inspectAgain")}</button>
        <button type="button" onClick={close} className="rounded-lg bg-[#333333] px-4 py-2 text-xs hover:bg-[#444444]">{t("libraryDoctor.close")}</button>
      </footer>
    </div>
  </div>;
};
