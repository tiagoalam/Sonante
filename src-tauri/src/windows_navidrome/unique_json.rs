//! Exact original duplicate-key rejection policy; no filesystem or source mutation.
use serde::Deserialize;
/// serde maps normally accept duplicate keys (including duplicate IDs). Reject
/// duplicates recursively before typed deserialization; reject trailing JSON too.
pub(crate) fn readonly_unique_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ()> {
    struct Unique(serde_json::Value);
    impl<'de> Deserialize<'de> for Unique {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> serde::de::Visitor<'de> for V {
                type Value = Unique;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str("JSON without duplicate keys") }
                fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Unique,E> { Ok(Unique(v.into())) }
                fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Unique,E> { Ok(Unique(v.into())) }
                fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Unique,E> { Ok(Unique(v.into())) }
                fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Unique,E> {
                    serde_json::Number::from_f64(v).map(|n| Unique(n.into())).ok_or_else(|| E::custom("nonfinite"))
                }
                fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Unique,E> { Ok(Unique(v.into())) }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Unique,E> { Ok(Unique(serde_json::Value::Null)) }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut a:A) -> Result<Unique,A::Error> {
                    let mut v=Vec::new(); while let Some(Unique(x))=a.next_element()? { v.push(x); }
                    Ok(Unique(v.into()))
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(self, mut a:A) -> Result<Unique,A::Error> {
                    let mut v=serde_json::Map::new();
                    while let Some(k)=a.next_key::<String>()? {
                        if v.contains_key(&k) { return Err(serde::de::Error::custom("duplicate key")); }
                        let Unique(x)=a.next_value()?; v.insert(k,x);
                    }
                    Ok(Unique(v.into()))
                }
            }
            d.deserialize_any(V)
        }
    }
    let Unique(v)=serde_json::from_slice(bytes).map_err(|_| ())?;
    serde_json::from_value(v).map_err(|_| ())
}
