import { invoke } from "@tauri-apps/api/core";
import type { LocalFileInfo } from "../types/localFileInfo";
import { singleFileInspection } from "../utils/localFileInfo";
export const inspectLocalFile = singleFileInspection((path: string) =>
  invoke<LocalFileInfo>("get_local_file_info", { path }));
