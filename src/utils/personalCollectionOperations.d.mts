export type CollectionSelectionAlbum =
  | { source: "local"; albumId: string; title: string; artist: string }
  | { source: "plex"; serverId: string; libraryId: string; ratingKey: string; title: string; artist: string }
  | { source: "subsonic"; connectionId: string; accountId: string; albumId: string; title: string; artist: string };

export type CollectionAddOutcome = { album: CollectionSelectionAlbum; status: "added" | "duplicate" | "failed"; code?: string | null };
export function collectionSelectionKey(album: CollectionSelectionAlbum): string;
export function toggleCollectionSelection(current: CollectionSelectionAlbum[], album: CollectionSelectionAlbum): CollectionSelectionAlbum[];
export function associateCollectionAlbums(
  items: CollectionSelectionAlbum[],
  associate: (album: CollectionSelectionAlbum) => Promise<unknown>,
  isCurrent?: () => boolean,
  concurrency?: number,
): Promise<CollectionAddOutcome[] | null>;
export function resolveCollectionMembers<T extends { id: number }>(
  members: T[],
  resolve: (member: T) => Promise<unknown>,
  isCurrent: () => boolean,
  publish: (memberId: number, result: unknown | null, error: string | null) => void,
  concurrency?: number,
): Promise<void>;

export function mergeCollectionOutcomes(previous: CollectionAddOutcome[], retry: CollectionAddOutcome[]): CollectionAddOutcome[];

export function createVisibleResolutionQueue<T extends { id: number }>(resolve: (member: T) => Promise<unknown>, publish: (id: number, result: unknown | null, error: string | null) => void, concurrency?: number): { enqueue(member: T, current: () => boolean): void; dispose(): void };
