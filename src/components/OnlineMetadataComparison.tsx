import React, { useId } from "react";
import { useTranslation } from "react-i18next";
import type { OnlineMetadataDetails } from "../types/onlineMetadata";
import type { LocalAlbumFieldSummary } from "../types/localTags";
import type { LocalAlbumTagEditorDraft } from "../utils/localAlbumTagEditor";
import { suggestionValue, transferFields, transferReason } from "../utils/onlineMetadataTransfer";
import type { DateSource, TransferField } from "../utils/onlineMetadataTransfer";

interface Props {
  details: OnlineMetadataDetails;
  draft: LocalAlbumTagEditorDraft;
  available: boolean;
  choices: TransferField[];
  dateSource: DateSource | null;
  onChoose: (field: TransferField, checked: boolean) => void;
  onDateSource: (source: DateSource) => void;
  onTransfer: () => void;
}

export const OnlineMetadataComparison: React.FC<Props> = ({ details, draft, available, choices, dateSource,
  onChoose, onDateSource, onTransfer }) => {
  const { t } = useTranslation();
  const id = useId();
  const value = (text: string | null) => text === null ? t("onlineMetadata.missing")
    : text === "" ? t("onlineMetadata.empty") : text;
  const summary = (field: LocalAlbumFieldSummary) => field.state === "common" ? value(field.value)
    : t(`albumTagEditor.states.${field.state}`);
  return <section aria-labelledby={`${id}-title`} className="mt-4 rounded-lg border border-[#555555] p-3 text-xs">
    <h4 id={`${id}-title`} className="font-bold text-white">{t("onlineMetadata.comparison.title")}</h4>
    <p className="mt-2 whitespace-pre-wrap break-words">{t("onlineMetadata.comparison.source")} {value(details.title)}</p>
    <p className="break-all font-mono text-[10px]">{details.id}</p>
    <p className="mt-2 text-[#BBBBBB]">{t("onlineMetadata.comparison.explanation")}</p>
    <fieldset className="mt-3 space-y-2">
      <legend className="font-bold">{t("onlineMetadata.comparison.dateSource")}</legend>
      {(["edition", "first_release"] as const).map(source => {
        const date = suggestionValue(details, "date", source), reason = transferReason(date, true);
        return <label key={source} className="block whitespace-pre-wrap break-words">
          <input type="radio" name={`${id}-date-source`} checked={dateSource === source} disabled={!available || !!reason}
            onChange={() => onDateSource(source)} className="mr-2" />
          {t(`onlineMetadata.comparison.${source}`)}: {value(date)}
          {reason && <span className="ml-2 text-amber-200">{t(`onlineMetadata.comparison.reasons.${reason}`)}</span>}
        </label>;
      })}
      {details.release_group_id && <p className="break-all font-mono text-[10px]">{t("onlineMetadata.comparison.releaseGroup")}: {details.release_group_id}</p>}
    </fieldset>
    <div className="mt-3 overflow-x-auto">
      <table className="w-full text-left">
        <thead><tr>{["field", "inspected", "draft", "suggestion"].map(column =>
          <th key={column} className="p-2 align-top">{t(`onlineMetadata.comparison.${column}`)}</th>)}</tr></thead>
        <tbody>{transferFields.map(field => {
          const entry = draft.fields[field], proposed = suggestionValue(details, field, dateSource);
          const reason = field === "date" && !dateSource ? "chooseDateSource" : transferReason(proposed, field === "date");
          return <tr key={field} className="border-t border-[#444444]">
            <th className="p-2 align-top"><label><input type="checkbox" checked={choices.includes(field)} disabled={!available || !!reason}
              onChange={event => onChoose(field, event.target.checked)} className="mr-2" />{t(`tagEditor.fields.${field}`)}</label></th>
            <td className="whitespace-pre-wrap break-words p-2 align-top">{summary(draft.inspection.fields[field])}</td>
            <td className="whitespace-pre-wrap break-words p-2 align-top">{entry.clearAll ? t("onlineMetadata.comparison.remove")
              : entry.touched ? value(entry.value) : <>{summary(entry.summary)} <span className="text-[#999999]">({t("onlineMetadata.comparison.unchanged")})</span></>}</td>
            <td className="whitespace-pre-wrap break-words p-2 align-top">{value(proposed)}
              {reason && <p className="text-amber-200">{t(`onlineMetadata.comparison.reasons.${reason}`)}</p>}</td>
          </tr>;
        })}</tbody>
      </table>
    </div>
    {!available && <p role="status" className="mt-2 text-amber-200">{t("onlineMetadata.comparison.blocked")}</p>}
    <button type="button" disabled={!available || !choices.length} onClick={onTransfer}
      className="mt-3 rounded-lg border border-[#555555] px-3 py-2 text-white hover:bg-[#303030] disabled:opacity-40">
      {t("onlineMetadata.comparison.transfer")}
    </button>
  </section>;
};
