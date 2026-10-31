import React from "react";
import { useTranslation } from "react-i18next";
import { LibrarySourceTabs } from "./LibrarySourceTabs";
import type { ExploreSource } from "../utils/exploreNavigation";

export const ExploreSourceTabs: React.FC<{ source: ExploreSource; onChange: (source: ExploreSource) => void }> = ({ source, onChange }) => {
  const { t } = useTranslation();
  return <LibrarySourceTabs source={source} onChange={onChange} idPrefix="explore-source"
    label={t("explore.sourceTabs")} labels={{ local: t("explore.sources.local"), plex: t("explore.sources.plex") }} />;
};
