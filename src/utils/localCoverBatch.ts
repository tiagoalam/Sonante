// Coalesce only the two artwork reads already admitted by the resolver.
// No timer, polling, extra parallel worker or persistent snapshot is added.
export type LocalCoverReply = { Ok: string | null } | { Err: string };
export function createLocalCoverBatch(read: (paths: string[]) => Promise<readonly LocalCoverReply[]>) {
  let scheduled = false;
  const pending: {path: string; resolve: (value: string | null) => void; reject: (reason: unknown) => void}[] = [];
  function schedule() {
    if (scheduled) return;
    scheduled = true;
    queueMicrotask(() => {
      scheduled = false;
      const batch = pending.splice(0, 2);
      if (pending.length) schedule();
      void Promise.resolve().then(() => read(batch.map(item => item.path))).then(replies => {
        if (!Array.isArray(replies) || replies.length !== batch.length || replies.some(reply =>
          !reply || typeof reply !== "object" || Object.keys(reply).length !== 1 ||
          !("Ok" in reply ? reply.Ok === null || typeof reply.Ok === "string" : "Err" in reply && typeof reply.Err === "string"))) {
          throw new Error("LOCAL_COVER_BATCH_INVALID");
        }
        replies.forEach((reply, index) => {
          if ("Ok" in reply) batch[index].resolve(reply.Ok);
          else batch[index].reject(reply.Err);
        });
      }).catch(error => batch.forEach(item => item.reject(error)));
    });
  }
  return (path: string): Promise<string | null> => new Promise((resolve,reject) => {
    pending.push({path,resolve,reject}); schedule();
  });
}
