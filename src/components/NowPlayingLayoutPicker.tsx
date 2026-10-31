import React from "react";
import { ChevronDown, PanelsTopLeft } from "lucide-react";
import { useTranslation } from "react-i18next";
import { NOW_PLAYING_LAYOUTS, type NowPlayingLayout, type PsychoPlacement } from "../utils/nowPlayingLayout";

interface Props {
  layout: NowPlayingLayout;
  placement: PsychoPlacement;
  onLayout?: (layout: NowPlayingLayout) => void;
  onPlacement?: (placement: PsychoPlacement) => void;
}
export const NowPlayingLayoutPicker: React.FC<Props> = ({ layout, placement, onLayout, onPlacement }) => {
  const { t } = useTranslation();
  return <div className="np-look-tools">
    <label className="np-look-picker">
      <PanelsTopLeft size={16} aria-hidden="true" /><span>{t("nowPlaying.look.label")}</span>
      <select value={layout} aria-label={t("nowPlaying.look.label")} aria-keyshortcuts="1 2 3 4" title={t("nowPlaying.look.shortcuts")} onChange={event => {
        const next = NOW_PLAYING_LAYOUTS.find(value => value === event.target.value);
        if (next) onLayout?.(next);
      }}>
        {NOW_PLAYING_LAYOUTS.map(value => <option key={value} value={value}>{t(`nowPlaying.layouts.${value}`)}</option>)}
      </select><ChevronDown size={13} aria-hidden="true" />
    </label>
    {layout === "psycho" && <label className="np-look-picker np-placement-picker">
      <span>{t("nowPlaying.look.placement")}</span>
      <select value={placement} aria-label={t("nowPlaying.look.placement")} onChange={event => {
        if (event.target.value === "frame" || event.target.value === "background") onPlacement?.(event.target.value);
      }}>
        <option value="frame">{t("nowPlaying.look.frame")}</option>
        <option value="background">{t("nowPlaying.look.background")}</option>
      </select><ChevronDown size={13} aria-hidden="true" />
    </label>}
  </div>;
};
