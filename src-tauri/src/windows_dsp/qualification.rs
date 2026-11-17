//! Test-executable-only File sink. Absent from every product release; no
//! environment/config switch can enable a File sink in the installed app.
use std::{cell::RefCell,path::PathBuf};
use serde_json::{json,Value};
thread_local!{static FILE_OUTPUT:RefCell<Option<PathBuf>>=const{RefCell::new(None)};}
pub(crate) fn with_file_output<T>(path:PathBuf,run:impl FnOnce()->T)->T{
    FILE_OUTPUT.with(|p|{assert!(p.borrow().is_none(),"nested File qualification");*p.borrow_mut()=Some(path);});
    struct Restore;impl Drop for Restore{fn drop(&mut self){FILE_OUTPUT.with(|p|*p.borrow_mut()=None);}}
    let _restore=Restore;run()
}
pub(super) fn file_output()->Option<Value>{
    FILE_OUTPUT.with(|p|p.borrow().as_ref().map(|p|json!({"type":"File","channels":2,"format":"S32_LE","filename":p.to_str().expect("own fixture UTF8"),"wav_header":false})))
}
#[test]
fn file_sink_is_thread_scoped_and_restored_after_the_explicit_test(){
    assert!(file_output().is_none());
    with_file_output(PathBuf::from("own-fixture.raw"),||{
        assert_eq!(file_output().unwrap()["type"],"File");
        assert!(std::thread::spawn(||file_output().is_none()).join().unwrap());
    });assert!(file_output().is_none());
}
