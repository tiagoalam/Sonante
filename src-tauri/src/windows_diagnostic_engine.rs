//! Windows-only projection of an already received MPD status error. No I/O.
//! Fixed hints are observations of text, not hardware/format proof.
const DIRECT:&[&str]=&["WIN_DIRECT_SOURCE_FORMAT_UNSUPPORTED","WIN_DIRECT_EXCLUSIVE_DENIED","WIN_DIRECT_ENDPOINT_CHANGED","WIN_DIRECT_ENDPOINT_UNAVAILABLE","WIN_DIRECT_INITIALIZE_FAILED","WIN_DIRECT_DEVICE_BUSY","WIN_DIRECT_PLAYBACK_ERROR","WIN_DIRECT_DOP_UNSUPPORTED"];
const STDIO:&[&str]=&["WIN_DIRECT_STDIO_OPEN_FAILED","WIN_DIRECT_STDIO_INITIALIZE_FAILED","WIN_DIRECT_STDIO_SOURCE_FORMAT_UNSUPPORTED","WIN_DIRECT_STDIO_EXCLUSIVE_DENIED","WIN_DIRECT_STDIO_ENDPOINT_CHANGED","WIN_DIRECT_STDIO_ENDPOINT_UNAVAILABLE","WIN_DIRECT_STDIO_DEVICE_BUSY"];
pub(super) fn code(direct:bool,eq:bool,raw:&str)->&'static str {
 if !direct{return if eq{"WIN_EQ_MPD_STATUS_ERROR"}else{"WIN_SHARED_MPD_STATUS_ERROR"};}
 let bytes=raw.as_bytes();let text=String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_ascii_lowercase();
 if text.contains("sonante direct dop unsupported")||text.contains("sonante direct dop: endpoint rejected"){return DIRECT[7];}
 // Exact authored MPD markers, or known public AUDCLNT names/codes only.
 if text.contains("sonante direct strict: source format unsupported")||text.contains("sonante direct pcm negotiation: no compatible supported pcm format")||text.contains("audclnt_e_unsupported_format")||text.contains("0x88890008"){return DIRECT[0];}
 if text.contains("exclusive mode not allowed")||text.contains("audclnt_e_exclusive_mode_not_allowed")||text.contains("0x8889000e"){return DIRECT[1];}
 if text.contains("sonante endpoint identity changed"){return DIRECT[2];}
 if text.contains("sonante endpoint unavailable")||text.contains("audclnt_e_device_invalidated")||text.contains("0x88890004"){return DIRECT[3];}
 if text.contains("audclnt_e_device_in_use")||text.contains("0x8889000a"){return DIRECT[5];}
 if text.contains("unable to initialize audio client"){return DIRECT[4];}
 DIRECT[6]
}
pub(super) fn known(raw:&str)->bool{DIRECT.contains(&raw)||STDIO.contains(&raw)||matches!(raw,"WIN_EQ_MPD_STATUS_ERROR"|"WIN_SHARED_MPD_STATUS_ERROR")}
#[cfg(test)]mod tests{
 use super::*;
 #[test]fn authored_mpd_failures_map_to_fixed_direct_hints(){for(raw,expected)in[("Failed to open output: Sonante Direct strict: source format unsupported",DIRECT[0]),("Sonante Direct PCM negotiation: no compatible supported PCM format",DIRECT[0]),("Sonante Direct DoP unsupported: endpoint rejected exact exclusive PCM carrier",DIRECT[7]),("Exclusive mode not allowed",DIRECT[1]),("Sonante endpoint identity changed",DIRECT[2]),("Sonante endpoint unavailable",DIRECT[3]),("Unable to initialize audio client",DIRECT[4])]{assert_eq!(code(true,false,raw),expected);}}
 #[test]fn only_known_public_audclnt_values_are_projected(){assert_eq!(code(true,false,"PRIVATE device 0x88890008 SECRET"),DIRECT[0]);assert_eq!(code(true,false,"AUDCLNT_E_DEVICE_IN_USE"),DIRECT[5]);assert_eq!(code(true,false,"PRIVATE 0xDEADBEEF UNKNOWN"),DIRECT[6]);}
 #[test]fn other_routes_never_inherit_direct_claims(){assert_eq!(code(false,true,"Sonante Direct strict: source format unsupported"),"WIN_EQ_MPD_STATUS_ERROR");assert_eq!(code(false,false,"0x88890008"),"WIN_SHARED_MPD_STATUS_ERROR");}
 #[test]fn arbitrarily_large_private_text_and_split_utf8_remain_bounded_safe_labels(){let raw=format!("{}PRIVATE_URL SECRET_TOKEN", "á".repeat(100000));let value=code(true,false,&raw);assert!(known(value));assert!(!value.contains("PRIVATE")&&!value.contains("SECRET"));assert!(value.len()<64);}
 #[test]fn redaction_accepts_only_whole_known_codes_not_prefixes_or_suffixes(){for raw in [DIRECT[0],"WIN_EQ_MPD_STATUS_ERROR","WIN_SHARED_MPD_STATUS_ERROR"]{assert!(known(raw));for variant in[format!("{raw}?token=PRIVATE"),format!("{raw} PRIVATE"),format!("PRIVATE{raw}")]{assert!(!known(&variant));}}}
 #[test]fn owned_stdio_hints_are_finite_exact_codes_without_extending_raw_error_projection(){for raw in STDIO{assert!(known(raw));for variant in[format!("{raw}?token=PRIVATE"),format!("{raw} PRIVATE"),format!("PRIVATE{raw}")]{assert!(!known(&variant));}}}
}
