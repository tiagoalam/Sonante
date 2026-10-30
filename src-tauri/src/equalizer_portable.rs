//! Portable tonal settings only: no internal IDs, paths, runtime state or activation.
use crate::equalizer::{EqBand, EqFilterType, EqPreset};
use serde::{Deserialize, Serialize};
pub(crate) const MAX_BYTES: usize = 32_768;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format: String,
    version: u32,
    name: String,
    preamp_db: f64,
    bands: Vec<Band>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Band {
    enabled: bool,
    filter_type: EqFilterType,
    frequency_hz: f64,
    gain_db: f64,
    q: f64,
}
pub(crate) fn inspect(text: &str) -> Result<EqPreset, String> {
    if text.len() > MAX_BYTES { return Err("EQ_IMPORT_INVALID".into()); }
    let d: Document = serde_json::from_str(text).map_err(|_| "EQ_IMPORT_INVALID")?;
    if d.format != "sonante-eq-preset" || d.version != 1 { return Err("EQ_IMPORT_VERSION".into()); }
    let p = EqPreset { id: "import_preview".into(), name: d.name, preamp_db: d.preamp_db,
        bands: d.bands.into_iter().enumerate().map(|(i,b)|EqBand {
            id: format!("import_{:02}",i+1), enabled:b.enabled, filter_type:b.filter_type,
            frequency_hz:b.frequency_hz, gain_db:b.gain_db,q:b.q,
        }).collect() };
    crate::equalizer::validate_v1(&p).map_err(|_| "EQ_IMPORT_INVALID")?;
    crate::shared_dsp::safety::assess(&p,48000).map_err(|_| "EQ_UNSAFE_HEADROOM")?;
    Ok(p)
}
pub(crate) fn export(preset: &EqPreset) -> Result<String,String> {
    crate::equalizer::validate_v1(preset).map_err(|_| "EQ_PRESET_INVALID")?;
    crate::shared_dsp::safety::assess(preset,48000).map_err(|_| "EQ_UNSAFE_HEADROOM")?;
    let d = Document { format:"sonante-eq-preset".into(),version:1,name:preset.name.clone(),preamp_db:preset.preamp_db,
        bands:preset.bands.iter().map(|b|Band {enabled:b.enabled,filter_type:b.filter_type,frequency_hz:b.frequency_hz,gain_db:b.gain_db,q:b.q}).collect() };
    serde_json::to_string_pretty(&d).map_err(|_|"EQ_EXPORT_FAILED".into())
}
/// Read only the explicitly selected regular file, bounded before parsing.
pub(crate) fn read_document(path: &std::path::Path) -> Result<String,String> {
    use std::{io::Read,os::unix::fs::OpenOptionsExt};
    let file=std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_NONBLOCK)
        .open(path).map_err(|_|"EQ_IMPORT_INVALID")?;
    let meta=file.metadata().map_err(|_|"EQ_IMPORT_INVALID")?;
    if !meta.is_file() || meta.len()>MAX_BYTES as u64 {return Err("EQ_IMPORT_INVALID".into())}
    let mut bytes=Vec::new();file.take(MAX_BYTES as u64+1).read_to_end(&mut bytes).map_err(|_|"EQ_IMPORT_INVALID")?;
    if bytes.len()>MAX_BYTES {return Err("EQ_IMPORT_INVALID".into())}
    String::from_utf8(bytes).map_err(|_|"EQ_IMPORT_INVALID".into())
}
/// A selected export never replaces an existing file or follows a link.
pub(crate) fn write_new_document(path: &std::path::Path, document:&str) -> Result<(),String> {
    use std::{io::Write,os::unix::fs::OpenOptionsExt};
    let mut file=std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW)
        .open(path).map_err(|e|if e.kind()==std::io::ErrorKind::AlreadyExists {"EQ_EXPORT_EXISTS"}else{"EQ_EXPORT_FAILED"})?;
    file.write_all(document.as_bytes()).and_then(|_|file.sync_all()).map_err(|_|"EQ_EXPORT_FAILED".to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_roundtrip_has_no_identity_activation_or_equipment_and_preserves_order() {
        let mut p=crate::equalizer::built_in_presets()[2].clone();p.bands.swap(0,9);
        let text=export(&p).unwrap();let v:serde_json::Value=serde_json::from_str(&text).unwrap();
        assert_eq!(v.as_object().unwrap().len(),5);assert!(v.get("enabled").is_none());assert!(v.get("id").is_none());
        let imported=inspect(&text).unwrap();assert_eq!(imported.name,p.name);assert_eq!(imported.preamp_db,p.preamp_db);
        for (a,b) in imported.bands.iter().zip(&p.bands){assert_eq!((a.enabled,a.filter_type,a.frequency_hz,a.gain_db,a.q),(b.enabled,b.filter_type,b.frequency_hz,b.gain_db,b.q));}
        assert!(imported.bands.iter().all(|b|b.id.starts_with("import_")));
    }
    #[test]
    fn portable_rejects_unknown_fields_versions_sizes_numbers_and_unsafe_budget() {
        let text=export(&crate::equalizer::flat_preset()).unwrap();
        for (key,value) in [("path",serde_json::json!("/tmp/x")),("enabled",serde_json::json!(true)),("version",serde_json::json!(2)),("preamp_db",serde_json::json!(100))] {
            let mut v:serde_json::Value=serde_json::from_str(&text).unwrap();v[key]=value;assert!(inspect(&v.to_string()).is_err());
        }
        assert!(inspect(&" ".repeat(MAX_BYTES+1)).is_err());assert!(inspect("{\"format\":").is_err());
        let mut p=crate::equalizer::flat_preset();for b in &mut p.bands{b.gain_db=12.;b.frequency_hz=1000.;}assert!(export(&p).is_err());
        let mut v:serde_json::Value=serde_json::from_str(&text).unwrap();v["bands"][0]["frequency_hz"]=serde_json::json!(null);assert!(inspect(&v.to_string()).is_err());
    }
    #[test]
    fn selected_file_io_is_bounded_private_and_never_overwrites_or_follows_links() {
        use std::os::unix::{fs::{symlink,PermissionsExt}};
        let root=std::env::temp_dir().join(format!("seq-portable-files-{}",std::process::id()));std::fs::create_dir(&root).unwrap();let path=root.join("preset.json");
        let text=export(&crate::equalizer::flat_preset()).unwrap();write_new_document(&path,&text).unwrap();assert_eq!(read_document(&path).unwrap(),text);assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode()&0o777,0o600);
        assert_eq!(write_new_document(&path,"changed"),Err("EQ_EXPORT_EXISTS".into()));assert_eq!(read_document(&path).unwrap(),text);
        symlink(&path,root.join("link")).unwrap();assert!(read_document(&root.join("link")).is_err());assert!(write_new_document(&root.join("link"),"changed").is_err());
        std::fs::write(root.join("large"),vec![b' ';MAX_BYTES+1]).unwrap();assert!(read_document(&root.join("large")).is_err());
        for name in ["preset.json","link","large"]{std::fs::remove_file(root.join(name)).unwrap();}std::fs::remove_dir(root).unwrap();
    }

}
