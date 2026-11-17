//! Adapt the unchanged Linux session-only policy to MPD software volume.
//! Owned by the serial worker, shared by every WebView, never persisted.
#[derive(Default)]
pub(super) struct Memory(crate::shared_mute::Memory);
impl Memory {
    pub(super) fn plan(
        &mut self,
        generation: u64,
        owner: u64,
        token: &str,
        volume: u8,
        muted: bool,
    ) -> super::protocol::Result<Option<u8>> {
        // Queue revisions fence the request but do not replace the engine.
        if token != format!("windows:{generation}") {
            return Err("SHARED_MUTE_CONTEXT_CHANGED");
        }
        let context = format!("windows-owner:{owner}");
        match self.0.plan(
            &context,
            &context,
            u32::from(volume),
            volume == 0,
            false,
            muted,
        ) {
            Ok(crate::shared_mute::Action::None) => Ok(None),
            Ok(crate::shared_mute::Action::Volume(value)) => u8::try_from(value)
                .ok()
                .filter(|v| *v <= 100)
                .map(Some)
                .ok_or("SHARED_MUTE_CONTEXT_CHANGED"),
            Ok(crate::shared_mute::Action::Mute(_)) => Err("SHARED_MUTE_CONTEXT_CHANGED"),
            Err(error) if error == "SHARED_MUTE_LEVEL_UNKNOWN" => Err("SHARED_MUTE_LEVEL_UNKNOWN"),
            Err(_) => Err("SHARED_MUTE_CONTEXT_CHANGED"),
        }
    }
}
#[cfg(test)]
#[path = "mute_tests.rs"]
mod tests;
