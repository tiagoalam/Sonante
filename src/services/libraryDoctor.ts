import { invoke } from "@tauri-apps/api/core";
import type { LibraryDoctorReport } from "../types/libraryDoctor";

// Closing/unmounting does not cancel Rust. Do not overlap or reuse that old
// snapshot when another modal opens; recovery always needs an explicit retry.
let readInFlight = false;
export const libraryDoctorService = {
  async inspectAlbum(albumId: string): Promise<LibraryDoctorReport> {
    if (readInFlight) throw "read_in_progress";
    readInFlight = true;
    try {
      const command = albumId.startsWith("sqlite:") || albumId.startsWith("sqlite-cue-album:")
        ? "sqlite_library_album_doctor"
        : "get_local_album_doctor";
      return await invoke<LibraryDoctorReport>(command, { albumId });
    } finally {
      readInFlight = false;
    }
  },
};
