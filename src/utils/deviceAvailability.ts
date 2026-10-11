import type { DeviceAvailability } from "../types/audio";
import type { AppConfig } from "../types/config";

export interface AvailabilityQueryState {
  checking: boolean;
  results: DeviceAvailability[];
  failed: boolean;
}

export function newPhysicalOutputChoice(original: AppConfig | null, proposed: AppConfig): boolean {
  return proposed.audio_output_type === "alsa" && proposed.alsa_device.startsWith("hw:")
    && Boolean(original && (original.alsa_device !== proposed.alsa_device || original.audio_output_type !== "alsa"));
}

// Contextual gestures query only an enumerated physical choice, never the known
// current owner. Alias identity and permission to probe remain backend decisions.
export function shouldCheckContextualDevice(original: AppConfig | null, proposed: AppConfig, enumerated: string[]): boolean {
  return proposed.audio_output_type === "alsa" && proposed.alsa_device.startsWith("hw:")
    && enumerated.includes(proposed.alsa_device)
    && !(original && !original.first_run && original.audio_output_type === "alsa"
      && original.alsa_device === proposed.alsa_device);
}

// At most one IPC in flight; rapid changes retain only the latest queued endpoint.
// No startup requests, timers, polling or durable availability claims.
export function createDeviceAvailabilityQuery(
  read: (id: string) => Promise<DeviceAvailability[]>,
  publish: (state: AvailabilityQueryState) => void,
) {
  type Request = { id: string; generation: number; promise: Promise<DeviceAvailability | null>; resolve: (result: DeviceAvailability | null) => void };
  let generation = 0;
  let disposed = false;
  let running = false;
  let latest: Request | null = null;
  let queued: Request | null = null;

  function launch(request: Request) {
    running = true;
    void (async () => {
      let result: DeviceAvailability = { id: request.id, state: "indeterminate" };
      let failed = false;
      try {
        const rows = await read(request.id);
        const row = rows.find(row => row.id === request.id);
        if (row && ["available", "busy", "current_sonante", "indeterminate"].includes(row.state)) result = { id: request.id, state: row.state };
        else failed = true;
      } catch {
        // Never publish raw IPC/ALSA errors.
        failed = true;
      }
      if (!disposed && request.generation === generation) {
        publish({ checking: false, results: [result], failed });
        latest = null;
        request.resolve(result);
      } else request.resolve(null);
    })().finally(() => {
      running = false;
      const next = queued;
      queued = null;
      if (next && !disposed && next.generation === generation) launch(next);
    });
  }

  function invalidate() {
    generation++;
    latest?.resolve(null);
    queued?.resolve(null);
    latest = queued = null;
    if (!disposed) publish({ checking: false, results: [], failed: false });
  }

  return {
    check(id: string): Promise<DeviceAvailability | null> {
      if (disposed) return Promise.resolve(null);
      if (latest?.id === id) return latest.promise;
      latest?.resolve(null);
      queued?.resolve(null);
      const requestGeneration = ++generation;
      let resolve!: Request["resolve"];
      const promise = new Promise<DeviceAvailability | null>(done => { resolve = done; });
      const request = { id, generation: requestGeneration, promise, resolve };
      latest = request;
      publish({ checking: true, results: [], failed: false });
      if (running) queued = request;
      else launch(request);
      return promise;
    },
    invalidate,
    dispose() {
      disposed = true;
      invalidate();
    },
  };
}
