//! The original EQ service contract backed by the Windows playback actor.
//! Catalog/preset loading never starts an engine or listener.
use crate::{equalizer::{EqPreset,EqualizerConfig,EqualizerStore},windows_dsp::{self,catalog::{Edit,edit_store,revision}},windows_mpd::session::Controller};
use serde::Serialize;
use tauri::Manager;

#[derive(Serialize)]
pub(crate) struct Catalog {
    built_ins:Vec<EqPreset>,builtin_groups:Vec<crate::equalizer::BuiltinGroup>,
    config:EqualizerConfig,revision:String,runtime:windows_dsp::Snapshot,
    shared:bool,runtime_available:bool,pulse_audio:bool,
}
fn available(app:&tauri::AppHandle)->bool {
    let Some(camilla_hash)=windows_dsp::CAMILLA_SHA else{return false};
    let Ok(root)=app.path().resource_dir()else{return false};
    let root=root.join("runtimes/audio-candidate");
    let mpd:serde_json::Value=match serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"),"/../windows/audio-runtime.json"))){Ok(v)=>v,Err(_)=>return false};
    let Some(hash)=mpd["sha256"].as_str()else{return false};
    let Ok(_mpd)=crate::windows_mpd::session::runtime_lock(&root.join("mpd.exe"),hash)else{return false};
    crate::windows_mpd::session::runtime_lock(&root.join("camilladsp.exe"),camilla_hash).is_ok()
}
#[tauri::command]
pub(crate) async fn get_equalizer(app:tauri::AppHandle,state:tauri::State<'_,Controller>)->Result<Catalog,String>{
    let controller=state.inner().clone();
    tauri::async_runtime::spawn_blocking(move ||{
        let mut config=EqualizerStore::default().load().map_err(|_|"EQ_STORE_UNAVAILABLE_PRESERVED")?;
        let revision=revision(&config)?;
        config.enabled=false; // Saved desire is never evidence of activation.
        let runtime=controller.equalizer_snapshot().map_err(str::to_owned)?;
        let (_,_,_,settings,_)=controller.preferences().map_err(str::to_owned)?;
        Ok(Catalog{built_ins:crate::equalizer::catalog_built_ins(&config),builtin_groups:crate::equalizer::builtin_groups(&config),config,revision,runtime,shared:!settings.direct(),runtime_available:available(&app),pulse_audio:false})
    }).await.map_err(|_|"EQ_STORE_UNAVAILABLE".to_owned())?
}
#[tauri::command]
pub(crate) async fn edit_equalizer(expected_revision:String,edit:Edit,app:tauri::AppHandle,state:tauri::State<'_,Controller>)->Result<Catalog,String>{
    let controller=state.inner().clone();
    tauri::async_runtime::spawn_blocking(move ||{
        let lock=crate::persistence::sonante_config_dir().join("windows-equalizer-transaction.lock");
        let _lock=crate::persistence::operation_lock(&lock).map_err(|_|"EQ_STORE_BUSY")?;
        if matches!(&edit,Edit::Select{..}|Edit::Import{..}|Edit::Create{..}|Edit::Update{..}|Edit::Delete{..}){controller.note_eq_intervention();}
        edit_store(&EqualizerStore::default(),&expected_revision,edit)
    }).await.map_err(|_|"EQ_STORE_WRITE_UNCONFIRMED".to_owned())??;
    get_equalizer(app,state).await
}
#[derive(Serialize)]
pub(crate) struct ImportPreview{document:String,preset:EqPreset}
#[tauri::command]
pub(crate) async fn pick_equalizer_preset()->Result<Option<ImportPreview>,String>{
    tauri::async_runtime::spawn_blocking(||{
        let Some(path)=rfd::FileDialog::new().add_filter("Sonante EQ JSON",&["json"]).pick_file()else{return Ok(None)};
        let document=crate::equalizer_portable::read_document(&path)?;
        let preset=crate::equalizer_portable::inspect(&document)?;
        Ok(Some(ImportPreview{document,preset}))
    }).await.map_err(|_|"EQ_IMPORT_INVALID".to_owned())?
}
#[tauri::command]
pub(crate) async fn export_equalizer_preset(preset:EqPreset)->Result<bool,String>{
    tauri::async_runtime::spawn_blocking(move ||{
        let document=crate::equalizer_portable::export(&preset)?;
        let Some(path)=rfd::FileDialog::new().add_filter("Sonante EQ JSON",&["json"]).set_file_name("preset.sonante-eq.json").save_file()else{return Ok(false)};
        crate::equalizer_portable::write_new_document(&path,&document)?;Ok(true)
    }).await.map_err(|_|"EQ_EXPORT_FAILED".to_owned())?
}
#[tauri::command]
pub(crate) async fn apply_equalizer(enabled:bool,preset:Option<EqPreset>,bypass_expected:Option<EqPreset>,state:tauri::State<'_,Controller>)->Result<windows_dsp::Snapshot,String>{
    let controller=state.inner().clone();
    if enabled&&preset.is_none(){return Err("EQ_PRESET_REQUIRED".into());}
    tauri::async_runtime::spawn_blocking(move ||controller.apply_equalizer(if enabled{preset}else{None},bypass_expected).map_err(str::to_owned)).await.map_err(|_|"EQ_APPLY_FAILED".to_owned())?
}
#[tauri::command]
pub(crate) async fn compare_equalizer(action:windows_dsp::comparison::Action,token:Option<String>,expected:Option<EqPreset>,state:tauri::State<'_,Controller>)->Result<windows_dsp::Snapshot,String>{
    let controller=state.inner().clone();
    tauri::async_runtime::spawn_blocking(move ||controller.compare_equalizer(action,token,expected).map_err(str::to_owned)).await.map_err(|_|"EQ_COMPARISON_UNCONFIRMED".to_owned())?
}
