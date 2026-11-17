//! Exact private capture and Shared playback identity. Camilla serializes
//! optional defaults; accept only defaults with the same effective behavior.
use super::runtime::RuntimeError;
use serde_json::{json,Value};

pub(super) fn playback()->Value {
    #[cfg(all(test,target_os="windows"))]
    if let Some(output)=super::qualification::file_output(){return output;}
    json!({"type":"Wasapi","channels":2,"device":null,"format":"F32","exclusive":false,"polling":false})
}
pub(super) fn capture(parameters:Value)->Value {
    json!({"type":"Stdin","channels":2,"format":"S32_LE","sonante_pcm":parameters})
}
fn defaults(actual:&Value,expected:&Value,allowed:&[(&str,Value)])->bool {
    let (Some(actual),Some(expected))=(actual.as_object(),expected.as_object()) else{return false};
    expected.iter().all(|(k,v)|actual.get(k)==Some(v)) && actual.iter().all(|(k,v)|{
        expected.contains_key(k)||allowed.iter().any(|(key,default)|k==key && (v.is_null()||v==default))
    })
}
pub(super) fn verify(config:&Value,capture:&Value,playback:&Value)->Result<(),RuntimeError>{
    // All handle values, owner PID and nonce words must match the current
    // inherited child duplicates, not just a filename/title or user setting.
    if !defaults(&config["devices"]["capture"],capture,&[("labels",Value::Null),("extra_samples",json!(0)),("skip_bytes",json!(0)),("read_bytes",json!(0))])
        || !defaults(&config["devices"]["playback"],playback,&[])
    {return Err(RuntimeError::UnexpectedConfig)}
    Ok(())
}
/// Readback from the authenticated MPD child. This attests its private output
/// format and state, never the physical playback device or a render clock.
pub(crate) fn verify_mpd(lines:&[String],playing:bool)->Result<(),&'static str>{
    const ERROR:&str="EQ_RUNTIME_UNCONFIRMED";
    let mut fields=std::collections::BTreeMap::new();let mut attributes=std::collections::BTreeMap::new();
    for line in lines{
        let (key,value)=line.split_once(": ").ok_or(ERROR)?;
        if key=="attribute"{let (key,value)=value.split_once('=').ok_or(ERROR)?;if attributes.insert(key,value).is_some(){return Err(ERROR);}}
        else if fields.insert(key,value).is_some(){return Err(ERROR);}
    }
    if fields.get("outputid")!=Some(&"0")||fields.get("plugin")!=Some(&"sonante_private_pcm")||fields.get("outputname")!=Some(&"Sonante Shared EQ")||fields.get("outputenabled")!=Some(&"1")||attributes.get("sonante_pcm_abi")!=Some(&"1")||attributes.get("sonante_pcm_format")!=Some(&"48000:32:2")||attributes.get("sonante_pcm_state")!=Some(&if playing{"open"}else{"ready"}){return Err(ERROR);}
    let number=|key|attributes.get(key).ok_or(ERROR)?.parse::<u64>().map_err(|_|ERROR);
    let read=number("sonante_pcm_read_frames")?;let written=number("sonante_pcm_written_frames")?;
    if written>i64::MAX as u64||read>written||written-read>4096||playing&&written==0{return Err(ERROR);}
    Ok(())
}
#[cfg(test)]
pub(super) fn test_capture()->Value {
    capture(json!({"mapping":"101","readable":"102","writable":"103","stop":"104","owner_pid":99,"nonce0":"123","nonce1":"456"}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_binding_rejects_every_foreign_handle_owner_nonce_and_audio_route(){
        let c=test_capture();let p=playback();
        let config=json!({"devices":{"capture":c,"playback":p}});
        verify(&config,&c,&p).unwrap();
        for path in ["/devices/capture/sonante_pcm/mapping","/devices/capture/sonante_pcm/readable","/devices/capture/sonante_pcm/writable","/devices/capture/sonante_pcm/stop","/devices/capture/sonante_pcm/nonce0","/devices/capture/sonante_pcm/nonce1","/devices/playback/device","/devices/playback/format"]{
            let mut changed=config.clone();*changed.pointer_mut(path).unwrap()=json!("foreign");
            assert_eq!(verify(&changed,&c,&p),Err(RuntimeError::UnexpectedConfig),"{path}");
        }
        for (path,value) in [("/devices/capture/sonante_pcm/owner_pid",json!(100)),("/devices/playback/exclusive",json!(true)),("/devices/capture/channels",json!(1)),("/devices/capture/type",json!("Wasapi"))]{
            let mut changed=config.clone();*changed.pointer_mut(path).unwrap()=value;
            assert_eq!(verify(&changed,&c,&p),Err(RuntimeError::UnexpectedConfig));
        }
    }
    #[test]
    fn optional_defaults_cannot_insert_capture_truncation_or_unreviewed_settings(){
        let c=test_capture();let p=playback();let mut config=json!({"devices":{"capture":c,"playback":p}});
        for key in ["extra_samples","skip_bytes","read_bytes"]{
            config["devices"]["capture"][key]=json!(0);verify(&config,&c,&p).unwrap();
            config["devices"]["capture"][key]=json!(1);assert!(verify(&config,&c,&p).is_err());
            config["devices"]["capture"].as_object_mut().unwrap().remove(key);
        }
        config["devices"]["capture"]["loopback"]=json!(false);assert!(verify(&config,&c,&p).is_err());
    }
    #[test]
    fn owned_mpd_output_requires_exact_private_route_abi_state_and_bounded_frames(){
        let ready=["outputid: 0","outputname: Sonante Shared EQ","outputenabled: 1","plugin: sonante_private_pcm","attribute: sonante_pcm_abi=1","attribute: sonante_pcm_format=48000:32:2","attribute: sonante_pcm_state=ready","attribute: sonante_pcm_written_frames=0","attribute: sonante_pcm_read_frames=0"].map(str::to_owned).to_vec();
        verify_mpd(&ready,false).unwrap();assert!(verify_mpd(&ready,true).is_err());
        let mut active=ready.clone();active[6]="attribute: sonante_pcm_state=open".into();active[7]="attribute: sonante_pcm_written_frames=1024".into();verify_mpd(&active,true).unwrap();
        for (index,text) in [(3,"plugin: wasapi"),(5,"attribute: sonante_pcm_format=44100:16:2"),(6,"attribute: sonante_pcm_state=lost"),(7,"attribute: sonante_pcm_written_frames=5000"),(8,"attribute: sonante_pcm_read_frames=2048")]{let mut wrong=active.clone();wrong[index]=text.into();assert!(verify_mpd(&wrong,true).is_err());}
        let mut duplicate=active;duplicate.push("outputid: 1".into());assert!(verify_mpd(&duplicate,true).is_err());
    }
}
