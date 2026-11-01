import { useEffect } from 'react';
import type { PlaybackStatus } from '../types/audio';
import { audioService } from '../services/audio';
import { logError } from './frontendDiagnostics';
// Same dependencies and effect as the application shell: progress/status polling
// never resends an unchanged title. Failure is log-only and never controls audio.
export function useAppWindowTitle(isPlaybackAvailable: boolean, playbackStatus: Pick<PlaybackStatus, 'state' | 'title' | 'artist'>) {
useEffect(() => {
    if (isPlaybackAvailable && playbackStatus.state === "play" && playbackStatus.title) {
      const trackLabel = playbackStatus.artist
        ? `${playbackStatus.artist} — ${playbackStatus.title}`
        : playbackStatus.title;
      audioService.setWindowTitle(`Sonante • ${trackLabel}`).catch(error => logError("app.window-title", error));
    } else {
      audioService.setWindowTitle("Sonante").catch(error => logError("app.window-title", error));
    }
  }, [isPlaybackAvailable, playbackStatus.state, playbackStatus.title, playbackStatus.artist]);
}
