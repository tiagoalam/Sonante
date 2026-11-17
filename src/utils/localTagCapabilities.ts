/** SQLite album editing is opt-in while the per-file write/reconciliation
 * workflow remains experimental; the backend independently enforces this gate. */
export function localTagEditingAvailable(input: {
  sqliteBacked: boolean;
  detailUnavailable: boolean;
  catalogLoaded: boolean;
  catalogLoading: boolean;
  catalogError: boolean;
  mpdBusy: boolean;
  saveBlocked: boolean;
  manualCheckPlayback: boolean;
  libraryUpdating: boolean;
  albumPresent: boolean;
  sqliteEditSupported?: boolean;
  earlyAccessEnabled?: boolean;
}): boolean {
  if (input.sqliteBacked) {
    return Boolean(input.earlyAccessEnabled) && Boolean(input.sqliteEditSupported)
      && !input.detailUnavailable && !input.mpdBusy
      && !input.saveBlocked && !input.manualCheckPlayback && !input.libraryUpdating
      && input.albumPresent;
  }
  return !input.detailUnavailable && input.catalogLoaded && !input.catalogLoading
    && !input.catalogError && !input.mpdBusy && !input.saveBlocked
    && !input.manualCheckPlayback && !input.libraryUpdating && input.albumPresent;
}
