//! Original preset edit/CAS/schema policy. Windows supplies the operation
//! lock and native selected-file I/O; edits never activate audio.
use crate::equalizer::{self,EqPreset,EqualizerConfig,EqualizerStore,PresetRef};
use serde::Deserialize;
use sha2::{Digest,Sha256};
pub(crate) fn revision(config: &EqualizerConfig) -> Result<String, String> {
    let bytes = serde_json::to_vec(config).map_err(|_| "EQ_STORE_FAILED")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Edit {
    Import { document: String, name: String },
    LinkEquipment { id: Option<String>, name: String, reference: PresetRef },
    UnlinkEquipment { id: String },
    Create {
        name: String,
        source: Option<PresetRef>,
        preset: Option<EqPreset>,
    },
    Rename {
        id: String,
        name: String,
    },
    Update {
        id: String,
        preset: EqPreset,
    },
    Delete {
        reference: PresetRef,
    },
    Select {
        reference: PresetRef,
    },
}
pub(crate) fn edit_store(store: &EqualizerStore, expected_revision: &str, edit: Edit) -> Result<(), String> {
    let mut config = store.load().map_err(|_| "EQ_STORE_UNAVAILABLE_PRESERVED")?;
    let current = revision(&config)?;
    if current != expected_revision {
        return Err("EQ_STORE_CHANGED".into());
    }
    let result = match edit {
        Edit::Import { document, name } => {
            let mut value = crate::equalizer_portable::inspect(&document)?;
            let created=config.create_user_preset(&name,None).map_err(|_|"EQ_PRESET_INVALID")?;
            value.id=created.id.clone();value.name=created.name;
            config.update_user_preset(&created.id,value).map_err(|_|"EQ_PRESET_INVALID")?;
            config.set_active_preset(PresetRef::User{id:created.id})
        }
        Edit::LinkEquipment { id, name, reference } => config.link_equipment(id,&name,reference),
        Edit::UnlinkEquipment { id } => config.unlink_equipment(&id),
        Edit::Create {
            name,
            source,
            preset,
        } => {
            let created = config
                .create_user_preset(&name, source.as_ref())
                .map_err(|_| "EQ_PRESET_INVALID")?;
            if let Some(mut value) = preset {
                value.id = created.id.clone();
                value.name = created.name;
                config
                    .update_user_preset(&created.id, value)
                    .map_err(|_| "EQ_PRESET_INVALID")?;
            }
            config.set_active_preset(PresetRef::User { id: created.id })
        }
        Edit::Rename { id, name } => config.rename_user_preset(&id, &name),
        Edit::Update { id, preset } => config.update_user_preset(&id, preset),
        Edit::Delete { reference } => config.delete_preset(&reference),
        Edit::Select { reference } => config.set_active_preset(reference),
    };
    for p in &config.user_presets {
        crate::equalizer::validate_v1(p).map_err(|_| "EQ_PRESET_INVALID")?;
    }
    result.map_err(|_| "EQ_PRESET_INVALID")?;
    config.set_enabled(false).map_err(|_| "EQ_PRESET_INVALID")?;
    store
        .save(&config)
        .map_err(|_| "EQ_STORE_WRITE_UNCONFIRMED".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_crud_cas_does_not_activate_and_preserves_corruption() {
        let dir = std::env::temp_dir().join(format!("sonante-eq-crud-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("equalizer-shared-v1.json");
        let store = EqualizerStore::new(path.clone());
        let base = store.load().unwrap();
        let initial = revision(&base).unwrap();
        let mut preset = equalizer::flat_preset();
        preset.preamp_db = -4.;
        edit_store(
            &store,
            &initial,
            Edit::Create {
                name: "Desk".into(),
                source: None,
                preset: Some(preset.clone()),
            },
        )
        .unwrap();
        let saved = store.load().unwrap();
        assert!(!saved.enabled);
        assert_eq!(saved.active().unwrap().preamp_db, -4.);
        let id = saved.active().unwrap().id;
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            edit_store(
                &store,
                &initial,
                Edit::Rename {
                    id: id.clone(),
                    name: "Changed".into()
                }
            ),
            Err("EQ_STORE_CHANGED".into())
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        edit_store(
            &store,
            &revision(&saved).unwrap(),
            Edit::Rename {
                id: id.clone(),
                name: "Office".into(),
            },
        )
        .unwrap();
        let saved = store.load().unwrap();
        assert_eq!(saved.active().unwrap().name, "Office");
        preset.bands.pop();
        assert_eq!(
            edit_store(
                &store,
                &revision(&saved).unwrap(),
                Edit::Update {
                    id: id.clone(),
                    preset
                }
            ),
            Err("EQ_PRESET_INVALID".into())
        );
        edit_store(
            &store,
            &revision(&saved).unwrap(),
            Edit::Delete {
                reference: PresetRef::User { id },
            },
        )
        .unwrap();
        assert_eq!(store.load().unwrap().active().unwrap().id, "flat");
        std::fs::write(&path, b"broken").unwrap();
        assert!(edit_store(
            &store,
            "",
            Edit::Select {
                reference: PresetRef::BuiltIn { id: "flat".into() }
            }
        )
        .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}

#[cfg(test)]
mod daily_tests {
 use super::*;
 #[test]
 fn imported_copy_and_equipment_use_cas_without_activation_or_lost_references() {
  let root=std::env::temp_dir().join(format!("seq-daily-cas-{}",std::process::id()));std::fs::create_dir(&root).unwrap();let store=EqualizerStore::new(root.join("eq.json"));
  let document=crate::equalizer_portable::export(&crate::equalizer::built_in_presets()[2]).unwrap();let initial=store.load().unwrap();let rev=revision(&initial).unwrap();
  edit_store(&store,&rev,Edit::Import{document:document.clone(),name:"Travel".into()}).unwrap();let c=store.load().unwrap();assert!(!c.enabled);assert_eq!(c.user_presets.len(),1);assert_ne!(c.user_presets[0].id,"import_preview");let selected=c.active_preset.clone();
  let bytes=std::fs::read(root.join("eq.json")).unwrap();assert_eq!(edit_store(&store,&rev,Edit::Import{document:document.clone(),name:"Stale".into()}),Err("EQ_STORE_CHANGED".into()));assert_eq!(std::fs::read(root.join("eq.json")).unwrap(),bytes);
  assert!(edit_store(&store,&revision(&c).unwrap(),Edit::Import{document,name:"Travel".into()}).is_err());assert_eq!(std::fs::read(root.join("eq.json")).unwrap(),bytes);
  edit_store(&store,&revision(&c).unwrap(),Edit::LinkEquipment{id:None,name:"DAC and headphones".into(),reference:selected.clone()}).unwrap();let c=store.load().unwrap();assert!(!c.enabled);let equipment=c.equipment_profiles[0].clone();
  assert!(edit_store(&store,&revision(&c).unwrap(),Edit::Delete{reference:selected.clone()}).is_err());assert_eq!(store.load().unwrap(),c);
  edit_store(&store,&revision(&c).unwrap(),Edit::Rename{id:c.user_presets[0].id.clone(),name:"Renamed".into()}).unwrap();let c=store.load().unwrap();assert_eq!(c.equipment_profiles[0].preset,selected);
  edit_store(&store,&revision(&c).unwrap(),Edit::UnlinkEquipment{id:equipment.id}).unwrap();let c=store.load().unwrap();assert_eq!(c.user_presets.len(),1);
  edit_store(&store,&revision(&c).unwrap(),Edit::Delete{reference:selected}).unwrap();assert!(store.load().unwrap().user_presets.is_empty());std::fs::remove_file(root.join("eq.json")).unwrap();std::fs::remove_dir(root).unwrap();
 }
 #[test]
 fn equipment_is_optional_in_legacy_schema_and_references_names_and_limits_are_validated() {
  let c=EqualizerConfig::default();let mut v=serde_json::to_value(&c).unwrap();v.as_object_mut().unwrap().remove("equipment_profiles");let mut c:EqualizerConfig=serde_json::from_value(v).unwrap();assert!(c.equipment_profiles.is_empty());
  let flat=PresetRef::BuiltIn{id:"flat".into()};c.link_equipment(None,"Desk",flat.clone()).unwrap();let saved=c.clone();assert!(c.link_equipment(None,"desk",flat.clone()).is_err());assert_eq!(c,saved);
  assert!(c.link_equipment(Some("missing".into()),"Unknown",flat.clone()).is_err());assert!(c.link_equipment(None,"Bad",PresetRef::User{id:"missing".into()}).is_err());assert_eq!(c,saved);
  for i in 1..32 {c.link_equipment(None,&format!("equipment {i}"),flat.clone()).unwrap();}let saved=c.clone();assert!(c.link_equipment(None,"Overflow",flat).is_err());assert_eq!(c,saved);assert!(!c.enabled);
 }
}
