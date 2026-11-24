export function collectionSelectionKey(album) {
  switch (album.source) {
    case "local": return JSON.stringify(["local", album.albumId]);
    case "plex": return JSON.stringify(["plex", album.serverId, album.libraryId, album.ratingKey]);
    case "subsonic": return JSON.stringify(["subsonic", album.connectionId, album.accountId, album.albumId]);
  }
}

export function toggleCollectionSelection(current, album) {
  const key = collectionSelectionKey(album);
  const exists = current.some(item => collectionSelectionKey(item) === key);
  return exists
    ? current.filter(item => collectionSelectionKey(item) !== key)
    : [...current, album];
}

export async function associateCollectionAlbums(items, associate, isCurrent = () => true, concurrency = 4) {
  const outcomes = new Array(items.length);
  let cursor = 0;
  const workers = Array.from({ length: Math.min(Math.max(1, concurrency), items.length) }, async () => {
    while (isCurrent()) {
      const index = cursor++;
      if (index >= items.length) return;
      try {
        await associate(items[index]);
        outcomes[index] = { album: items[index], status: "added" };
      } catch (error) {
        const message = typeof error === "string" ? error : error instanceof Error ? error.message : "";
        outcomes[index] = {
          album: items[index],
          status: message.includes("COLLECTION_DUPLICATE_MEMBER") ? "duplicate" : "failed",
          code: message.match(/COLLECTION_[A-Z0-9_]+/)?.[0] ?? null,
        };
      }
    }
  });
  await Promise.all(workers);
  return isCurrent() ? outcomes.filter(Boolean) : null;
}

export async function resolveCollectionMembers(members, resolve, isCurrent, publish, concurrency = 4) {
  let cursor = 0;
  const workers = Array.from({ length: Math.min(Math.max(1, concurrency), members.length) }, async () => {
    while (isCurrent()) {
      const index = cursor++;
      if (index >= members.length) return;
      const member = members[index];
      try {
        const result = await resolve(member);
        if (isCurrent()) publish(member.id, result, null);
      } catch {
        if (isCurrent()) publish(member.id, null, "error");
      }
    }
  });
  await Promise.all(workers);
}

export function mergeCollectionOutcomes(previous, retry) {
  const byKey = new Map(retry.map(item => [collectionSelectionKey(item.album), item]));
  return previous.map(item => byKey.get(collectionSelectionKey(item.album)) ?? item);
}

// Only visible cards enter the queue; stale queued work is skipped before IPC.
export function createVisibleResolutionQueue(resolve, publish, concurrency = 4) {
  const jobs = [], pending = new Set();
  let active = 0, disposed = false;
  function pump() {
    while (!disposed && active < concurrency && jobs.length) {
      const job = jobs.shift();
      if (!job.current()) { pending.delete(job.member.id); continue; }
      active++;
      Promise.resolve().then(() => resolve(job.member)).then(
        result => { if (!disposed && job.current()) publish(job.member.id, result, null); },
        () => { if (!disposed && job.current()) publish(job.member.id, null, "error"); },
      ).finally(() => { active--; pending.delete(job.member.id); pump(); });
    }
  }
  return {
    enqueue(member, current) { if (disposed || pending.has(member.id)) return; pending.add(member.id); jobs.push({ member, current }); pump(); },
    dispose() { disposed = true; jobs.length = 0; pending.clear(); },
  };
}
