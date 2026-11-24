// Artwork metadata is returned only with a verified, currently available target.
export async function resolvePersonalCollectionArtwork(target, services, current = () => true) {
  if (!target || !current()) return null;
  try {
    if (target.source === "local") {
      const src = await services.local(`sqlite-album://${target.album_id}`, current);
      return current() && src ? { kind: "local", src } : null;
    }
    const art = services.artwork;
    if (!art || art.source !== target.source || !current()) return null;
    if (art.source === "plex") {
      return art.image.server_id === target.server_id ? { kind: "plex", image: art.image, scope: JSON.stringify([target.server_id, target.library_id, art.session]) } : null;
    }
    return art.connection_id && art.cover_id ? { kind: "subsonic", connection: art.connection_id, id: art.cover_id } : null;
  } catch { return null; }
}
