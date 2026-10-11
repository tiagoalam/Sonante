import type { AlbumDiscTracks } from "./localAlbumDiscs";

export type AlbumTracksState = {
  albumId: string | null;
  phase: "waiting" | "loading" | "ready" | "error";
  groups: AlbumDiscTracks[];
  error: string | null;
  catalogVersion: unknown;
};

export function createAlbumTracksLoader(
  load: (albumId: string) => Promise<AlbumDiscTracks[]>,
  publish: (state: AlbumTracksState) => void,
  onMpdBusy: () => void = () => {},
) {
  let generation = 0;
  return {
    select(albumId: string | null, confirmed: boolean, catalogVersion: unknown = null) {
      const request = ++generation;
      const state: AlbumTracksState = { albumId, phase: "waiting", groups: [], error: null, catalogVersion };
      publish(state);
      if (!albumId || !confirmed) return;
      publish({ ...state, phase: "loading" });
      void load(albumId).then(groups => {
        if (request === generation) publish({ ...state, phase: "ready", groups });
      }).catch((error: unknown) => {
        if (request !== generation) return;
        const message = error instanceof Error ? error.message : String(error);
        if (message === "Biblioteca local em atualização no MPD; tente novamente após a atualização.") {
          publish(state);
          onMpdBusy();
        } else {
          publish({ ...state, phase: "error", error: message });
        }
      });
    },
    cancel() { generation += 1; },
  };
}
