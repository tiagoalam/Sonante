import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { diagnosticInvoke } from "../utils/frontendDiagnostics";
import { SleepTimerStore } from "../utils/sleepTimer";
export const sleepTimer = new SleepTimerStore({
  read: () => diagnosticInvoke(invoke, "get_sleep_timer"),
  set: minutes => diagnosticInvoke(invoke, "set_sleep_timer", { minutes }),
  listen: changed => listen("sleep-timer://changed", event => changed(event.payload)),
}, () => performance.now());
