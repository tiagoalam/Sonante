//! Session-only restoration. Observations never write volume or remove mute.
#[derive(Default)]
pub(crate) struct Memory { context: String, positive: Option<u32> }
#[derive(Debug, PartialEq)]
pub(crate) enum Action { None, Volume(u32), Mute(bool) }
impl Memory {
    fn observe(&mut self, context: &str, value: u32) {
        if self.context != context { self.context=context.into(); self.positive=None; }
        if (1..=100).contains(&value) { self.positive=Some(value); }
    }
    pub(crate) fn plan(&mut self, expected: &str, context: &str, value: u32, muted: bool, native: bool, requested: bool) -> Result<Action,String> {
        if expected != context || value>100 { return Err("SHARED_MUTE_CONTEXT_CHANGED".into()); }
        self.observe(context,value);
        if (muted || value==0)==requested { return Ok(Action::None); }
        if native && (requested || value>0) { return Ok(Action::Mute(requested)); }
        if requested { return Ok(Action::Volume(0)); }
        self.positive.map(Action::Volume).ok_or_else(||"SHARED_MUTE_LEVEL_UNKNOWN".into())
    }
}
#[cfg(test)] mod tests {
 use super::*;
 #[test] fn restores_same_context_across_surfaces_and_duplicate_requests() {
  let mut m=Memory::default();m.observe("mpd:1",37);
  assert_eq!(m.plan("mpd:1","mpd:1",37,false,false,true).unwrap(),Action::Volume(0));
  assert_eq!(m.plan("mpd:1","mpd:1",0,true,false,true).unwrap(),Action::None);
  assert_eq!(m.plan("mpd:1","mpd:1",0,true,false,false).unwrap(),Action::Volume(37));
  assert_eq!(m.plan("mpd:1","mpd:1",37,false,false,false).unwrap(),Action::None);
 }
 #[test] fn unknown_zero_and_context_changes_never_choose_a_level() {
  let mut m=Memory::default();assert!(m.plan("a","a",0,true,false,false).is_err());
  m.observe("a",23);assert!(m.plan("a","b",0,true,false,false).is_err());
  assert!(m.plan("b","b",0,true,false,false).is_err());
  m.observe("b",61);assert_eq!(m.plan("b","b",0,true,false,false).unwrap(),Action::Volume(61));
 }
 #[test] fn real_native_mute_with_positive_volume_preserves_level() {
  let mut m=Memory::default();assert_eq!(m.plan("pw:1","pw:1",29,true,true,false).unwrap(),Action::Mute(false));
  assert_eq!(m.plan("pw:1","pw:1",29,false,true,true).unwrap(),Action::Mute(true));
 }
 #[test] fn failed_write_does_not_invent_state_and_refresh_never_writes() {
  let mut m=Memory::default();assert_eq!(m.plan("a","a",17,false,false,true).unwrap(),Action::Volume(0));
  // Write failed, actual device still 17; same request is retried from an actual observation.
  assert_eq!(m.plan("a","a",17,false,false,true).unwrap(),Action::Volume(0));
  m.observe("a",0);assert_eq!(m.plan("a","a",0,true,false,false).unwrap(),Action::Volume(17));
 }
}
