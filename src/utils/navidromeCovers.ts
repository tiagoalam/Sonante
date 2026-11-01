// Small, bounded LRU; no URLs, credentials or persistent artwork cache.
// Obsolete queued cards never spend a request ahead of the visible page.
export function navidromeCovers(read: (connection: string, id: string) => Promise<string>) {
    type Job = {
        needed: () => boolean;
        run: () => Promise<void>;
        cancel: () => void;
    };
    const cache = new Map<string, string>();
    const pending = new Map<string, {
        promise: Promise<string>;
        consumers: (() => boolean)[];
    }>();
    const jobs: Job[] = [];
    let active = 0, bytes = 0;
    function pump() {
        while (active < 3 && jobs.length) {
            const job = jobs.shift()!;
            if (!job.needed()) {
                job.cancel();
                continue;
            }
            active++;
            void job.run().finally(() => { active--; pump(); });
        }
    }
    return (connection: string, id: string, current: () => boolean = () => true): Promise<string> => {
        const key = JSON.stringify([connection, id]);
        const hit = cache.get(key);
        if (hit) {
            cache.delete(key);
            cache.set(key, hit);
            return Promise.resolve(hit);
        }
        const existing = pending.get(key);
        if (existing) {
            existing.consumers.push(current);
            return existing.promise;
        }
        // Prune abandoned queued cards before enforcing the finite queue limit.
        for (let i = jobs.length - 1; i >= 0; i--)
            if (!jobs[i].needed())
                jobs.splice(i, 1)[0].cancel();
        if (jobs.length >= 96)
            return Promise.reject('NAVIDROME_UNAVAILABLE');
        const consumers = [current];
        const promise = new Promise<string>((resolve, reject) => {
            jobs.push({ needed: () => consumers.some(consumer => consumer()), cancel: () => { pending.delete(key); reject('NAVIDROME_CANCELLED'); }, run: async () => {
                    try {
                        const image = await read(connection, id);
                        if (!/^data:image\/(?:jpeg|png|webp);base64,[A-Za-z0-9+/]+=*$/.test(image) || image.length > 700000)
                            throw 'NAVIDROME_PROTOCOL';
                        while (cache.size >= 64 || bytes + image.length * 2 > 8 * 1024 * 1024) {
                            const oldest = cache.keys().next().value;
                            if (oldest === undefined)
                                break;
                            bytes -= cache.get(oldest)!.length * 2;
                            cache.delete(oldest);
                        }
                        cache.set(key, image);
                        bytes += image.length * 2;
                        resolve(image);
                    }
                    catch (error) {
                        reject(error);
                    }
                    finally {
                        pending.delete(key);
                    }
                } });
        });
        pending.set(key, { promise, consumers });
        pump();
        return promise;
    };
}
