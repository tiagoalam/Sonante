//! Owned MPD output readback, not a physical/DAC or bit-perfect attestation.
use super::{endpoint_key,Result};
use std::collections::BTreeMap;
use serde::Serialize;
const ERROR:&str="windows_direct_unconfirmed";
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
#[serde(rename_all="snake_case")]
pub(crate) enum PcmEncoding{S8,S16,S24,S32,Float32}
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
pub(crate) struct PcmFormat{pub rate:u32,pub encoding:PcmEncoding,pub channels:u32}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) struct VerifiedFormat{pub source:PcmFormat,pub plugin_input:PcmFormat,pub selected:PcmFormat,pub render_container_bits:u32,pub render_valid_bits:u32,pub converted:bool}
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
#[serde(rename_all="camelCase")]
pub(crate) struct DsdFormat{pub rate:u32,pub channels:u32}
pub(crate) fn parse_dsd_source(value:&str)->Option<DsdFormat>{
 let fields=value.split(':').collect::<Vec<_>>();
 if fields.len()!=3||fields[1]!="dsd"{return None;}
 let rate=fields[0].parse::<u32>().ok()?;let channels=fields[2].parse::<u32>().ok()?;
 if !(1..=6_144_000).contains(&rate)||rate%2!=0||!(1..=8).contains(&channels){return None;}
 Some(DsdFormat{rate,channels})
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) struct VerifiedDop{pub source:DsdFormat,pub transport_rate:u32,pub transport_channels:u32,pub render_container_bits:u32,pub render_valid_bits:u32}
fn precision(value:PcmEncoding)->u32{match value{PcmEncoding::S8=>8,PcmEncoding::S16=>16,PcmEncoding::S24=>24,PcmEncoding::S32|PcmEncoding::Float32=>32}}
pub fn verify(lines:&[String],key:&str,source:&str)->Result<VerifiedFormat>{
 let mut fields=BTreeMap::new();let mut attrs=BTreeMap::new();
 for line in lines{
  let (name,value)=line.split_once(": ").ok_or(ERROR)?;
  if name=="attribute"{let (a,b)=value.split_once('=').ok_or(ERROR)?;if attrs.insert(a,b).is_some(){return Err(ERROR);}}
  else if fields.insert(name,value).is_some(){return Err(ERROR);}
 }
 if fields.get("outputid")!=Some(&"0")||fields.get("plugin")!=Some(&"wasapi")||fields.get("outputname")!=Some(&"Sonante Windows Direct")||fields.get("outputenabled")!=Some(&"1")||attrs.get("sonante_exclusive")!=Some(&"yes")||attrs.get("sonante_direct_strict")!=Some(&"yes")||attrs.get("sonante_direct_pcm_negotiation")!=Some(&"restricted-v1"){return Err(ERROR);}
 if endpoint_key(attrs.get("sonante_endpoint_id").ok_or(ERROR)?)!=key{return Err(ERROR);}
 fn format(s:&str)->Result<PcmFormat>{let a=s.split(':').collect::<Vec<_>>();if a.len()!=3{return Err(ERROR);}let rate=a[0].parse::<u32>().map_err(|_|ERROR)?;let channels=a[2].parse::<u32>().map_err(|_|ERROR)?;let encoding=match a[1]{"8"=>PcmEncoding::S8,"16"=>PcmEncoding::S16,"24"=>PcmEncoding::S24,"32"=>PcmEncoding::S32,"f"=>PcmEncoding::Float32,_=>return Err(ERROR)};if rate==0||rate>3072000||!(1..=8).contains(&channels){return Err(ERROR);}Ok(PcmFormat{rate,encoding,channels})}
 let source=format(source)?;
 let plugin_input=format(attrs.get("sonante_plugin_input").ok_or(ERROR)?)?;
 let selected=format(attrs.get("sonante_plugin_selected").ok_or(ERROR)?)?;
 if plugin_input.rate!=source.rate||plugin_input.channels!=source.channels||selected.channels!=source.channels{return Err(ERROR);}
 let number=|name:&str|->Result<u32>{attrs.get(name).ok_or(ERROR)?.parse::<u32>().map_err(|_|ERROR)};
 let container=number("sonante_render_container_bits")?;let valid=number("sonante_render_valid_bits")?;
 let converted=plugin_input!=selected;
 if attrs.get("sonante_pcm_conversion")!=Some(&(if converted{"converted"}else{"none"}))||number("sonante_render_rate")?!=selected.rate||number("sonante_render_channels")?!=selected.channels||!matches!(container,8|16|24|32)||valid!=precision(selected.encoding)||valid>container{return Err(ERROR);}
 Ok(VerifiedFormat{source,plugin_input,selected,render_container_bits:container,render_valid_bits:valid,converted})
}

/// Verify an MPD readback that remains DSD internally and is exported only as
/// a DoP carrier by the bound WASAPI Exclusive endpoint.
pub fn verify_dop(lines:&[String],key:&str,source:&str)->Result<VerifiedDop>{
 let mut fields=BTreeMap::new();let mut attrs=BTreeMap::new();
 for line in lines{
  let (name,value)=line.split_once(": ").ok_or(ERROR)?;
  if name=="attribute"{let (a,b)=value.split_once('=').ok_or(ERROR)?;if attrs.insert(a,b).is_some(){return Err(ERROR);}}
  else if fields.insert(name,value).is_some(){return Err(ERROR);}
 }
 if fields.get("outputid")!=Some(&"0")||fields.get("plugin")!=Some(&"wasapi")||fields.get("outputname")!=Some(&"Sonante Windows Direct")||fields.get("outputenabled")!=Some(&"1")||attrs.get("sonante_exclusive")!=Some(&"yes")||attrs.get("sonante_direct_strict")!=Some(&"yes")||attrs.get("sonante_direct_pcm_negotiation")!=Some(&"restricted-v1")||attrs.get("sonante_dsd_transport")!=Some(&"dop")||attrs.get("sonante_dsd_conversion")!=Some(&"none")||attrs.get("sonante_pcm_conversion")!=Some(&"none"){return Err(ERROR);}
 if endpoint_key(attrs.get("sonante_endpoint_id").ok_or(ERROR)?)!=key{return Err(ERROR);}
 let source=parse_dsd_source(source).ok_or(ERROR)?;
 let input=parse_dsd_source(attrs.get("sonante_plugin_input").ok_or(ERROR)?).ok_or(ERROR)?;
 let selected=parse_dsd_source(attrs.get("sonante_plugin_selected").ok_or(ERROR)?).ok_or(ERROR)?;
 if source!=input||input!=selected{return Err(ERROR);}
 let number=|name:&str|->Result<u32>{attrs.get(name).ok_or(ERROR)?.parse::<u32>().map_err(|_|ERROR)};
 let transport_rate=source.rate/2;let transport_channels=source.channels;let container=number("sonante_render_container_bits")?;let valid=number("sonante_render_valid_bits")?;
 if number("sonante_render_rate")?!=transport_rate||number("sonante_render_channels")?!=transport_channels||!matches!(container,24|32)||valid!=24||valid>container{return Err(ERROR);}
 Ok(VerifiedDop{source,transport_rate,transport_channels,render_container_bits:container,render_valid_bits:valid})
}
