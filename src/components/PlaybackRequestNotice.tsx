import { useTranslation } from "react-i18next";
import type { PlaybackFailure } from "../utils/playbackIntent";
export const PlaybackRequestNotice = ({ failure }: { failure: PlaybackFailure | null }) => {
  const { t } = useTranslation();
  return failure ? <p role="alert" data-playback-request-error className="px-4 py-2 text-sm text-[color:var(--theme-fg-accent,#C9A45D)]">{t(`player.requestError.${failure}`)}</p> : null;
};
