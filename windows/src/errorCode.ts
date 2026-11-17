// Only fixed application codes can be shown; never raw ACKs, URLs or paths.
const codes = new Set([
 "PLAYBACK_RANDOM_ORDER_UNAVAILABLE",
 "EQ_RUNTIME_UNTRUSTED","EQ_SHARED_SWITCH_UNCONFIRMED",
 "EQ_BUSY","EQ_CANCELLED","EQ_SHARED_ONLY","EQ_PRESET_REQUIRED","EQ_PRESET_INVALID","EQ_UNSAFE_HEADROOM","EQ_CONFIG_INVALID","EQ_CONFIG_FAILED","EQ_STORE_BUSY","EQ_STORE_CHANGED","EQ_STORE_UNAVAILABLE","EQ_STORE_UNAVAILABLE_PRESERVED","EQ_STORE_WRITE_UNCONFIRMED","EQ_IMPORT_INVALID","EQ_IMPORT_VERSION","EQ_EXPORT_FAILED","EQ_EXPORT_EXISTS","EQ_RUNTIME_START_FAILED","EQ_RUNTIME_EXITED","EQ_RUNTIME_UNCONFIRMED","EQ_APPLY_FAILED","EQ_ROLLBACK_FAILED","EQ_CONTINUITY_LOST","EQ_CURSOR_UNCONFIRMED","EQ_COMPARISON_STALE","EQ_COMPARISON_INTERRUPTED","EQ_COMPARISON_UNAVAILABLE","EQ_COMPARISON_UNCONFIRMED","EQ_BYPASS_LEVEL_RETURN_CONFIRMATION_REQUIRED","EQ_RANDOM_STEP_UNAVAILABLE",
 "windows_direct_unconfirmed","WIN_DIRECT_DOP_UNSUPPORTED","windows_audio_invalid","windows_audio_storage","windows_audio_busy","windows_audio_device_missing","windows_audio_device_changed","windows_audio_devices_unavailable","windows_audio_devices_busy","windows_audio_devices_limit","windows_audio_devices_timeout","DIRECT_VOLUME_CONFIRMATION_REQUIRED","DIRECT_VOLUME_CONTEXT_CHANGED","DIRECT_VOLUME_ON_EQUIPMENT",
 "PLEX_ARTWORK_UNAVAILABLE","PLEX_INVALID","PLEX_CONTEXT_CHANGED","PLEX_UNAVAILABLE","PLEX_STORAGE","PLEX_PROTOCOL","PLEX_MEDIA_MISSING","NAVIDROME_INVALID","NAVIDROME_CONTEXT_CHANGED","NAVIDROME_UNAVAILABLE","NAVIDROME_STORAGE","windows_config_uncertain","windows_source_required","windows_source_disabled",
 "engine_cpu_incompatible", "engine_dependency_missing", "engine_image_invalid", "engine_state_failed", "engine_system_incompatible", "invalid_argument", "job_failed", "queue_boundary", "queue_checkpoint_failed", "queue_limit", "shutdown_failed",
 "mpd_media_not_requested", "mpd_media_request_only", "mpd_media_payload_not_sent",
 "mpd_error_after_media_delivery", "mpd_media_read_failed", "mpd_playback_error",
 "mpd_protocol", "mpd_ack", "mpd_timeout", "mpd_io", "mpd_disconnected",
 "playback_unconfirmed", "file_unavailable", "windows_media_unavailable", "unsupported_path",
 "unsupported_file", "runtime_missing", "runtime_changed", "runtime_directory_failed",
 "engine_start_failed", "engine_stdio_failed", "engine_exited", "engine_crashed",
 "peer_unverified", "port_failed", "random_failed", "worker_failed", "engine_busy",
 "index_timeout", "windows_network_unavailable", "PLAYBACK_QUEUE_UNCERTAIN", "PLAYBACK_PERSISTENCE_FAILED",
 "queue_restore_failed", "queue_save_failed", "library_changed", "no_track", "stale_request"
]);
export function publicErrorCode(value: unknown): string | null {
 return typeof value === "string" && codes.has(value) ? value : null;
}
