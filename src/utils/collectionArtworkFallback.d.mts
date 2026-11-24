export function createCollectionArtworkFallback(read: (path: string) => Promise<string | null>, limit?: number): (path: string, current: () => boolean) => Promise<string | null>;
