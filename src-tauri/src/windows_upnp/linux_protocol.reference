//! Small UPnP AV renderer protocol. Untrusted XML and HTTP are bounded here;
//! transport mutations are delegated to the application's existing admission.
use quick_xml::{events::Event, Reader};
use std::collections::BTreeMap;

pub const AVT: &str = "urn:schemas-upnp-org:service:AVTransport:1";
pub const RCS: &str = "urn:schemas-upnp-org:service:RenderingControl:1";
pub const CM: &str = "urn:schemas-upnp-org:service:ConnectionManager:1";
pub const RENDERER: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";
pub const PROTOCOLS: &str = "http-get:*:audio/flac:*,http-get:*:audio/x-flac:*,http-get:*:audio/mpeg:*,http-get:*:audio/mp4:*,http-get:*:audio/aac:*,http-get:*:audio/ogg:*,http-get:*:audio/wav:*,http-get:*:audio/x-wav:*,http-get:*:audio/L16:*";
pub fn escape(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}
pub fn uuid() -> Result<String, String> {
    use std::io::Read;
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|_| "UPNP_ID_UNAVAILABLE")?;
    b[6] = (b[6] & 15) | 64;
    b[8] = (b[8] & 63) | 128;
    Ok(format!("{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7],b[8],b[9],b[10],b[11],b[12],b[13],b[14],b[15]))
}

/// No file/resource/credential URLs, control characters, fragments, or local
/// host services. Ordinary private-LAN media servers remain supported.
pub fn media_url(raw: &str) -> Result<String, u16> {
    if raw.len() > 8192 || raw.chars().any(char::is_control) {
        return Err(716);
    }
    let url = reqwest::Url::parse(raw).map_err(|_| 716u16)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(716);
    }
    let host = url.host_str().ok_or(716u16)?;
    if host.trim_end_matches('.').eq_ignore_ascii_case("localhost")
        || host.trim_end_matches('.').ends_with(".localhost")
    {
        return Err(716);
    }
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
        let invalid = match ip {
            std::net::IpAddr::V4(ip) => {
                ip.is_unspecified()
                    || ip.is_loopback()
                    || ip.is_multicast()
                    || ip.is_link_local()
                    || ip.is_broadcast()
            }
            std::net::IpAddr::V6(ip) => {
                ip.is_unspecified()
                    || ip.is_loopback()
                    || ip.is_multicast()
                    || ip.is_unicast_link_local()
                    || ip.to_ipv4_mapped().is_some()
            }
        };
        if invalid {
            return Err(716);
        }
    }
    Ok(url.into())
}

/// Parse leaf text only; DTDs, entities outside the XML predefined set,
/// duplicates, excess depth, malformed endings and mixed nesting are refused.
pub fn leaves(xml: &str, limit: usize) -> Result<BTreeMap<String, String>, u16> {
    parse_leaves(xml, limit, false)
}
pub fn metadata(xml: &str) -> Result<BTreeMap<String, String>, u16> {
    parse_leaves(xml, 16384, true)
}
fn parse_leaves(xml: &str, limit: usize, repeated: bool) -> Result<BTreeMap<String, String>, u16> {
    if xml.len() > limit
        || xml.chars().any(|c| {
            (c.is_control() && !matches!(c, '\t' | '\r' | '\n'))
                || matches!(c, '\u{fffe}' | '\u{ffff}')
        })
    {
        return Err(402);
    }
    let mut reader = Reader::from_str(xml);
    reader.config_mut().check_end_names = true;
    let mut stack: Vec<(String, String, bool)> = Vec::new();
    let mut values = BTreeMap::new();
    let mut roots = 0;
    loop {
        match reader.read_event().map_err(|_| 402u16)? {
            Event::Start(e) => {
                if stack.len() >= 24 {
                    return Err(402);
                }
                if stack.is_empty() {
                    roots += 1;
                    if roots > 1 {
                        return Err(402);
                    }
                }
                if let Some(parent) = stack.last_mut() {
                    parent.2 = true;
                }
                let name = e.local_name().as_ref().to_string();
                stack.push((name, String::new(), false));
            }
            Event::Empty(e) => {
                if stack.is_empty() {
                    return Err(402);
                }
                let name = e.local_name().as_ref().to_string();
                if values.contains_key(&name) {
                    if !repeated {
                        return Err(402);
                    }
                } else {
                    values.insert(name, String::new());
                }
                if let Some(parent) = stack.last_mut() {
                    parent.2 = true;
                }
            }
            Event::Text(e) => {
                let text = e.xml10_content();
                let text = quick_xml::escape::unescape(&text).map_err(|_| 402u16)?;
                if let Some(parent) = stack.last_mut() {
                    parent.1.push_str(&text);
                } else if !text.trim().is_empty() {
                    return Err(402);
                }
            }
            Event::CData(e) => {
                let text = e.xml10_content();
                stack.last_mut().ok_or(402u16)?.1.push_str(&text);
            }
            Event::End(_) => {
                let (name, text, has_child) = stack.pop().ok_or(402u16)?;
                if !has_child {
                    if values.contains_key(&name) {
                        if !repeated {
                            return Err(402);
                        }
                    } else {
                        values.insert(name, text);
                    }
                }
            }
            Event::GeneralRef(e) => {
                let c = if let Some(c) = e.resolve_char_ref().map_err(|_| 402u16)? {
                    c
                } else {
                    match e.as_ref() {
                        "amp" => '&',
                        "lt" => '<',
                        "gt" => '>',
                        "quot" => '"',
                        "apos" => '\'',
                        _ => return Err(402),
                    }
                };
                if c.is_control() && !matches!(c, '\t' | '\r' | '\n') {
                    return Err(402);
                }
                stack.last_mut().ok_or(402u16)?.1.push(c);
            }
            Event::DocType(_) => return Err(402),
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() || roots != 1 || values.len() > 64 {
        return Err(402);
    }
    Ok(values)
}

pub fn time(raw: &str) -> Result<f64, u16> {
    let parts: Vec<_> = raw.split(':').collect();
    if parts.len() != 3 || parts[0].len() > 5 {
        return Err(711);
    }
    let h = parts[0].parse::<u32>().map_err(|_| 711u16)?;
    let m = parts[1].parse::<u32>().map_err(|_| 711u16)?;
    let s = parts[2].parse::<f64>().map_err(|_| 711u16)?;
    if m >= 60 || !(0.0..60.0).contains(&s) {
        return Err(711);
    }
    Ok(h as f64 * 3600.0 + m as f64 * 60.0 + s)
}
pub fn timestamp(seconds: f64) -> String {
    let n = if seconds.is_finite() && seconds > 0.0 {
        seconds as u64
    } else {
        0
    };
    format!("{:02}:{:02}:{:02}", n / 3600, (n / 60) % 60, n % 60)
}
pub fn success(service: &str, action: &str, fields: &[(&str, String)]) -> String {
    let fields = fields
        .iter()
        .map(|(key, value)| format!("<{key}>{}</{key}>", escape(value)))
        .collect::<String>();
    format!("<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action}Response xmlns:u=\"{service}\">{fields}</u:{action}Response></s:Body></s:Envelope>")
}
pub fn fault(code: u16) -> String {
    let description = match code {
        401 => "Invalid Action",
        402 => "Invalid Args",
        701 => "Transition not available",
        702 => "No contents",
        710 => "Seek mode not supported",
        711 => "Illegal seek target",
        716 => "Resource not found",
        718 => "Invalid InstanceID",
        _ => "Action Failed",
    };
    format!("<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail><UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\"><errorCode>{code}</errorCode><errorDescription>{description}</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>")
}

pub fn device(name: &str, id: &str) -> String {
    let services=[("avtransport",AVT,"AVTransport"),("rendering",RCS,"RenderingControl"),("connection",CM,"ConnectionManager")].iter().map(|(path,kind,name)|format!("<service><serviceType>{kind}</serviceType><serviceId>urn:upnp-org:serviceId:{name}</serviceId><SCPDURL>/{path}.xml</SCPDURL><controlURL>/control/{path}</controlURL><eventSubURL>/event/{path}</eventSubURL></service>")).collect::<String>();
    format!("<?xml version=\"1.0\"?><root xmlns=\"urn:schemas-upnp-org:device-1-0\"><specVersion><major>1</major><minor>0</minor></specVersion><device><deviceType>{RENDERER}</deviceType><friendlyName>{}</friendlyName><manufacturer>Sonante</manufacturer><modelName>Sonante UPnP Receiver</modelName><modelNumber>{}</modelNumber><UDN>uuid:{id}</UDN><serviceList>{services}</serviceList></device></root>",escape(name),env!("CARGO_PKG_VERSION"))
}

pub fn scpd(path: &str) -> Option<&'static str> {
    match path {
        "/avtransport.xml" => Some(include_str!("upnp/avtransport.xml")),
        "/rendering.xml" => Some(include_str!("upnp/rendering.xml")),
        "/connection.xml" => Some(include_str!("upnp/connection.xml")),
        _ => None,
    }
}

pub fn soap_valid(xml: &str, service: &str, action: &str) -> bool {
    use quick_xml::{name::ResolveResult, reader::NsReader};
    if xml.len() > 65536 {
        return false;
    }
    let mut reader = NsReader::from_str(xml);
    let mut depth = 0usize;
    let mut body = false;
    let mut bodies = 0;
    let mut actions = 0;
    loop {
        let Ok((namespace, event)) = reader.read_resolved_event() else {
            return false;
        };
        let expected = |value: &str| matches!(&namespace,ResolveResult::Bound(ns) if ns.0==value);
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                if depth > 24 || matches!(namespace, ResolveResult::Unknown(_)) {
                    return false;
                }
                let name = e.local_name();
                if depth == 0
                    && (name.as_ref() != "Envelope"
                        || !expected("http://schemas.xmlsoap.org/soap/envelope/"))
                {
                    return false;
                }
                if depth == 1 {
                    body = name.as_ref() == "Body";
                    if !expected("http://schemas.xmlsoap.org/soap/envelope/")
                        || (!body && name.as_ref() != "Header")
                    {
                        return false;
                    }
                    if body {
                        bodies += 1;
                    }
                }
                if depth == 2 && body {
                    if name.as_ref() != action || !expected(service) {
                        return false;
                    }
                    actions += 1;
                }
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                }
            }
            Event::End(_) => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            Event::DocType(_) => return false,
            Event::Eof => return depth == 0 && bodies == 1 && actions == 1,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upnp_xml_limits_and_entities() {
        assert_eq!(
            leaves(
                "<Envelope><Body><Action><Title>A &amp; B</Title></Action></Body></Envelope>",
                1024
            )
            .unwrap()["Title"],
            "A & B"
        );
        for s in [
            "<!DOCTYPE x [<!ENTITY a SYSTEM 'file:///etc/passwd'>]><x>&a;</x>",
            "<x><a>1</a><a>2</a></x>",
            "<x><a></x>",
            "<a/><b/>",
            "<x>&unknown;</x>",
            "<x>\0</x>",
        ] {
            assert!(leaves(s, 4096).is_err(), "{s}");
        }
        assert!(leaves(&format!("<x>{}</x>", "a".repeat(50)), 10).is_err());
    }
    #[test]
    fn upnp_media_url_and_time_bounds() {
        assert!(media_url("http://192.168.1.2/music.flac?token=private").is_ok());
        for url in [
            "file:///etc/passwd",
            "http://localhost/x",
            "http://127.0.0.1/x",
            "http://169.254.169.254/x",
            "http://[::1]/x",
            "http://user:pass@server/x",
            "http://server/\nnext",
            "http://224.0.0.1/x",
        ] {
            assert!(media_url(url).is_err());
        }
        assert_eq!(time("01:02:03.5"), Ok(3723.5));
        for t in ["00:60:00", "00:00:NaN", "00:00:inf", "00:00:-1"] {
            assert!(time(t).is_err());
        }
        assert_eq!(timestamp(3723.9), "01:02:03");
    }
    #[test]
    fn upnp_soap_header_body_and_namespace_must_agree() {
        let xml=format!("<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><u:GetProtocolInfo xmlns:u=\"{CM}\"/></s:Body></s:Envelope>");
        assert!(soap_valid(&xml, CM, "GetProtocolInfo"));
        assert!(!soap_valid(&xml, AVT, "GetProtocolInfo"));
        assert!(!soap_valid(&xml, CM, "SetAVTransportURI"));
        assert!(!soap_valid(
            &xml.replace("soap/envelope/", "wrong/"),
            CM,
            "GetProtocolInfo"
        ));
        let tags=metadata("<DIDL-Lite><item><title>A &amp; B</title><res>one</res><res>two</res><artist>A</artist><artist>B</artist></item></DIDL-Lite>").unwrap();
        assert_eq!(tags["title"], "A & B");
        assert_eq!(tags["artist"], "A");
    }
    #[test]
    fn upnp_descriptions_are_bounded_and_escape_device_name() {
        let id = uuid().unwrap();
        assert_eq!(id.len(), 36);
        let xml = device("A < B & C", &id);
        assert!(xml.contains("A &lt; B &amp; C"));
        for path in ["/avtransport.xml", "/rendering.xml", "/connection.xml"] {
            assert!(scpd(path).unwrap().contains("<serviceStateTable>"));
        }
        assert!(!success(
            AVT,
            "GetMediaInfo",
            &[("CurrentURI", "http://host/?a=1&b=2".into())]
        )
        .contains("?a=1&b=2"));
    }
}
