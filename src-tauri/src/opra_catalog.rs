//! Bounded, read-only parser for OPRA's generated JSONL catalog.
//!
//! This module deliberately has no network, filesystem, audio, or preset-store
//! integration. It preserves OPRA's filter vocabulary and source metadata;
//! `CompatibilityAssessment` is informational and never converts/applies EQ.

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Number, Value};
use std::collections::{BTreeMap, HashSet};
use std::fmt;

pub const MAX_CATALOG_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_LINE_BYTES: usize = 256 * 1024;
pub const MAX_RECORDS: usize = 50_000;
pub const MAX_STRING_BYTES: usize = 4_096;
pub const MAX_JSON_DEPTH: usize = 16;
pub const MAX_JSON_NODES_PER_RECORD: usize = 8_192;
pub const MAX_BANDS_PER_PROFILE: usize = 256;
const MAX_ID_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: Option<usize>,
    pub kind: ParseErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseErrorKind {
    EmptyCatalog,
    CatalogTooLarge,
    InvalidUtf8,
    LineTooLarge,
    TooManyRecords,
    MalformedJson,
    DuplicateJsonKey,
    InvalidRecord,
    UnknownRecordType,
    InvalidIdentifier,
    DuplicateRecordId,
    InvalidField,
    InvalidNumber,
    LimitExceeded,
    BrokenReference,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    pub vendors: BTreeMap<String, VendorRecord>,
    pub products: BTreeMap<String, ProductRecord>,
    pub equalizers: BTreeMap<String, EqRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VendorRecord {
    pub id: String,
    pub line: usize,
    pub data: VendorInfo,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductRecord {
    pub id: String,
    pub line: usize,
    pub data: ProductInfo,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EqRecord {
    pub id: String,
    pub line: usize,
    pub data: EqInfo,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VendorInfo {
    pub name: String,
    #[serde(default)]
    pub official_name: Option<String>,
    #[serde(default)]
    pub blurb: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductInfo {
    #[serde(default)]
    pub vendor_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub blurb: Option<String>,
    #[serde(default)]
    pub line_art_svg: Option<String>,
    #[serde(default)]
    pub line_art_96x64_png: Option<String>,
    #[serde(rename = "type")]
    pub product_type: ProductType,
    pub subtype: ProductSubtype,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductType {
    Headphones,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductSubtype {
    OverTheEar,
    OnEar,
    InEar,
    Earbuds,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EqInfo {
    #[serde(default)]
    pub product_id: Option<String>,
    pub author: String,
    #[serde(default)]
    pub details: Option<String>,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(rename = "type")]
    pub eq_type: EqType,
    pub parameters: EqParameters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqType {
    ParametricEq,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EqParameters {
    pub gain_db: f64,
    pub bands: Vec<OpraBand>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpraBand {
    #[serde(rename = "type")]
    pub filter_type: OpraFilterType,
    pub frequency: f64,
    #[serde(default)]
    pub gain_db: Option<f64>,
    #[serde(default)]
    pub q: Option<f64>,
    #[serde(default)]
    pub slope: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpraFilterType {
    PeakDip,
    HighShelf,
    LowShelf,
    LowPass,
    HighPass,
    BandPass,
    BandStop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityAssessment {
    pub filter_types: FilterTypeCompatibility,
    pub band_count: BandCountCompatibility,
    pub parameters: ParameterCompatibility,
    /// The OPRA schema describes Q generically and calls the overall gain an
    /// adjustment; the editor's exact equivalence has not been established.
    pub semantics: SemanticCompatibility,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterTypeCompatibility {
    SupportedByEditor,
    Unsupported { band_indexes: Vec<usize> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandCountCompatibility {
    WithinTenBandLimit,
    ExceedsTenBandLimit { count: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterCompatibility {
    WithinEditorLimits,
    OutsideEditorLimits { fields: Vec<ParameterIssue> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterIssue {
    Frequency { band_index: usize },
    Gain { band_index: usize },
    Q { band_index: usize },
    OverallGain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCompatibility {
    pub q_definition_unconfirmed: bool,
    pub slope_mapping_unconfirmed: bool,
    pub overall_gain_mapping_unconfirmed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    #[serde(rename = "type")]
    record_type: String,
    id: String,
    data: Value,
}

/// Parse the OPRA JSONL representation as one transaction: on any invalid
/// record/reference the caller receives only an error, never a partial catalog.
pub fn parse_catalog(bytes: &[u8]) -> Result<Catalog, ParseError> {
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err(error(None, ParseErrorKind::CatalogTooLarge));
    }
    if bytes.is_empty() || bytes.iter().all(u8::is_ascii_whitespace) {
        return Err(error(None, ParseErrorKind::EmptyCatalog));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| error(None, ParseErrorKind::InvalidUtf8))?;
    let mut catalog = Catalog {
        vendors: BTreeMap::new(),
        products: BTreeMap::new(),
        equalizers: BTreeMap::new(),
    };
    let mut record_count = 0usize;
    let mut record_ids = HashSet::new();
    for (line_index, raw) in text.split('\n').enumerate() {
        let line_number = line_index + 1;
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if raw.len() > MAX_LINE_BYTES {
            return Err(error(Some(line_number), ParseErrorKind::LineTooLarge));
        }
        if raw.trim().is_empty() {
            continue;
        }
        record_count += 1;
        if record_count > MAX_RECORDS {
            return Err(error(Some(line_number), ParseErrorKind::TooManyRecords));
        }

        let mut deserializer = serde_json::Deserializer::from_str(raw);
        let unique = UniqueValue::deserialize(&mut deserializer)
            .map_err(|e| error(Some(line_number), classify_json_error(&e.to_string())))?;
        deserializer
            .end()
            .map_err(|_| error(Some(line_number), ParseErrorKind::MalformedJson))?;
        check_json_complexity(&unique.0, line_number)?;
        let envelope: Envelope = serde_json::from_value(unique.0)
            .map_err(|_| error(Some(line_number), ParseErrorKind::InvalidRecord))?;
        validate_id(&envelope.id).map_err(|kind| error(Some(line_number), kind))?;
        if !record_ids.insert(envelope.id.clone()) {
            return Err(error(Some(line_number), ParseErrorKind::DuplicateRecordId));
        }

        match envelope.record_type.as_str() {
            "vendor" => {
                let data: VendorInfo = decode_data(envelope.data, line_number)?;
                validate_vendor(&data).map_err(|kind| error(Some(line_number), kind))?;
                let id = envelope.id;
                catalog.vendors.insert(
                    id.clone(),
                    VendorRecord {
                        id,
                        line: line_number,
                        data,
                    },
                );
            }
            "product" => {
                let data: ProductInfo = decode_data(envelope.data, line_number)?;
                validate_product(&data).map_err(|kind| error(Some(line_number), kind))?;
                let id = envelope.id;
                catalog.products.insert(
                    id.clone(),
                    ProductRecord {
                        id,
                        line: line_number,
                        data,
                    },
                );
            }
            "eq" => {
                let data: EqInfo = decode_data(envelope.data, line_number)?;
                validate_eq(&data).map_err(|kind| error(Some(line_number), kind))?;
                let id = envelope.id;
                catalog.equalizers.insert(
                    id.clone(),
                    EqRecord {
                        id,
                        line: line_number,
                        data,
                    },
                );
            }
            _ => return Err(error(Some(line_number), ParseErrorKind::UnknownRecordType)),
        }
    }
    if record_count == 0 {
        return Err(error(None, ParseErrorKind::EmptyCatalog));
    }
    validate_references(&catalog)?;
    Ok(catalog)
}

impl EqRecord {
    pub fn assess_compatibility(&self) -> CompatibilityAssessment {
        let bands = &self.data.parameters.bands;
        let unsupported = bands
            .iter()
            .enumerate()
            .filter_map(|(index, band)| {
                matches!(
                    band.filter_type,
                    OpraFilterType::LowPass
                        | OpraFilterType::HighPass
                        | OpraFilterType::BandPass
                        | OpraFilterType::BandStop
                )
                .then_some(index)
            })
            .collect::<Vec<_>>();
        let filter_types = if unsupported.is_empty() {
            FilterTypeCompatibility::SupportedByEditor
        } else {
            FilterTypeCompatibility::Unsupported {
                band_indexes: unsupported,
            }
        };
        let band_count = if bands.len() <= 10 {
            BandCountCompatibility::WithinTenBandLimit
        } else {
            BandCountCompatibility::ExceedsTenBandLimit { count: bands.len() }
        };
        let mut fields = Vec::new();
        if !(-24.0..=12.0).contains(&self.data.parameters.gain_db) {
            fields.push(ParameterIssue::OverallGain);
        }
        for (index, band) in bands.iter().enumerate() {
            if !(20.0..=20_000.0).contains(&band.frequency) {
                fields.push(ParameterIssue::Frequency { band_index: index });
            }
            if band
                .gain_db
                .is_some_and(|gain| !(-15.0..=15.0).contains(&gain))
            {
                fields.push(ParameterIssue::Gain { band_index: index });
            }
            if band.q.is_some_and(|q| !(0.1..=30.0).contains(&q)) {
                fields.push(ParameterIssue::Q { band_index: index });
            }
        }
        let parameters = if fields.is_empty() {
            ParameterCompatibility::WithinEditorLimits
        } else {
            ParameterCompatibility::OutsideEditorLimits { fields }
        };
        CompatibilityAssessment {
            filter_types,
            band_count,
            parameters,
            semantics: SemanticCompatibility {
                q_definition_unconfirmed: bands.iter().any(|b| b.q.is_some()),
                slope_mapping_unconfirmed: bands.iter().any(|b| b.slope.is_some()),
                overall_gain_mapping_unconfirmed: true,
            },
        }
    }
}

fn validate_references(catalog: &Catalog) -> Result<(), ParseError> {
    for record in catalog.products.values() {
        if record
            .data
            .vendor_id
            .as_ref()
            .is_some_and(|id| !catalog.vendors.contains_key(id))
        {
            return Err(error(Some(record.line), ParseErrorKind::BrokenReference));
        }
    }
    for record in catalog.equalizers.values() {
        if record
            .data
            .product_id
            .as_ref()
            .is_some_and(|id| !catalog.products.contains_key(id))
        {
            return Err(error(Some(record.line), ParseErrorKind::BrokenReference));
        }
    }
    Ok(())
}

fn validate_vendor(data: &VendorInfo) -> Result<(), ParseErrorKind> {
    validate_text(&data.name, 1)?;
    if let Some(v) = &data.official_name {
        validate_text(v, 1)?;
    }
    if let Some(v) = &data.blurb {
        validate_text(v, 10)?;
        if v.chars().count() > 1_000 {
            return Err(ParseErrorKind::LimitExceeded);
        }
    }
    if let Some(v) = &data.logo {
        validate_asset_path(v, "png")?;
    }
    Ok(())
}

fn validate_product(data: &ProductInfo) -> Result<(), ParseErrorKind> {
    if let Some(id) = &data.vendor_id {
        validate_id(id)?;
    }
    validate_text(&data.name, 1)?;
    if let Some(v) = &data.blurb {
        validate_text(v, 10)?;
        if v.chars().count() > 1_000 {
            return Err(ParseErrorKind::LimitExceeded);
        }
    }
    if let Some(v) = &data.line_art_svg {
        validate_asset_path(v, "svg")?;
    }
    if let Some(v) = &data.line_art_96x64_png {
        validate_asset_path(v, "png")?;
    }
    Ok(())
}

fn validate_eq(data: &EqInfo) -> Result<(), ParseErrorKind> {
    if let Some(id) = &data.product_id {
        validate_id(id)?;
    }
    validate_text(&data.author, 1)?;
    if let Some(v) = &data.details {
        validate_text(v, 1)?;
    }
    if let Some(v) = &data.link {
        validate_uri(v)?;
    }
    if !data.parameters.gain_db.is_finite() || data.parameters.gain_db.abs() > 1_000_000.0 {
        return Err(ParseErrorKind::InvalidNumber);
    }
    if data.parameters.bands.len() > MAX_BANDS_PER_PROFILE {
        return Err(ParseErrorKind::LimitExceeded);
    }
    for band in &data.parameters.bands {
        if !band.frequency.is_finite() || band.frequency <= 0.0 || band.frequency > 1_000_000.0 {
            return Err(ParseErrorKind::InvalidNumber);
        }
        if band
            .gain_db
            .is_some_and(|v| !v.is_finite() || v.abs() > 1_000_000.0)
        {
            return Err(ParseErrorKind::InvalidNumber);
        }
        if band
            .q
            .is_some_and(|v| !v.is_finite() || v < 0.1 || v > 1_000_000.0)
        {
            return Err(ParseErrorKind::InvalidNumber);
        }
        if let Some(slope) = band.slope {
            if ![6, 12, 18, 24, 30, 36].contains(&slope) {
                return Err(ParseErrorKind::InvalidField);
            }
        }
        match band.filter_type {
            OpraFilterType::LowPass | OpraFilterType::HighPass if band.slope.is_none() => {
                return Err(ParseErrorKind::InvalidField)
            }
            OpraFilterType::PeakDip
            | OpraFilterType::HighShelf
            | OpraFilterType::LowShelf
            | OpraFilterType::BandPass
            | OpraFilterType::BandStop
                if band.q.is_none() =>
            {
                return Err(ParseErrorKind::InvalidField)
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_id(value: &str) -> Result<(), ParseErrorKind> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || !value
            .bytes()
            // OPRA's generated catalog uses namespaced IDs such as
            // `vendor:model::author&curve`. Keep IDs bounded and ASCII-only
            // while accepting the separators present in legitimate records.
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':' | b'&'))
    {
        return Err(ParseErrorKind::InvalidIdentifier);
    }
    Ok(())
}

fn validate_text(value: &str, min_chars: usize) -> Result<(), ParseErrorKind> {
    if value.trim().is_empty()
        || value.len() > MAX_STRING_BYTES
        || value.chars().count() < min_chars
        || value.chars().any(char::is_control)
    {
        return Err(ParseErrorKind::InvalidField);
    }
    Ok(())
}

fn validate_asset_path(value: &str, extension: &str) -> Result<(), ParseErrorKind> {
    validate_text(value, 5)?;
    if value.contains('\\')
        || value.starts_with('/')
        || value.contains(':')
        || !value
            .to_ascii_lowercase()
            .ends_with(&format!(".{extension}"))
    {
        return Err(ParseErrorKind::InvalidField);
    }
    if value
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(ParseErrorKind::InvalidField);
    }
    Ok(())
}

fn validate_uri(value: &str) -> Result<(), ParseErrorKind> {
    if value.is_empty()
        || value.len() > MAX_STRING_BYTES
        || !value.is_ascii()
        || value.chars().any(char::is_whitespace)
        || value.contains('\\')
    {
        return Err(ParseErrorKind::InvalidField);
    }
    let Some((scheme, rest)) = value.split_once(':') else {
        return Err(ParseErrorKind::InvalidField);
    };
    let mut chars = scheme.bytes();
    if !chars.next().is_some_and(|b| b.is_ascii_alphabetic())
        || !chars.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
        || rest.is_empty()
    {
        return Err(ParseErrorKind::InvalidField);
    }
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(ParseErrorKind::InvalidField);
            }
            index += 3;
        } else if bytes[index].is_ascii_alphanumeric()
            || matches!(
                bytes[index],
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b':'
                    | b'/'
                    | b'?'
                    | b'#'
                    | b'['
                    | b']'
                    | b'@'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
            )
        {
            index += 1;
        } else {
            return Err(ParseErrorKind::InvalidField);
        }
    }
    Ok(())
}

fn decode_data<T: for<'de> Deserialize<'de>>(value: Value, line: usize) -> Result<T, ParseError> {
    serde_json::from_value(value).map_err(|_| error(Some(line), ParseErrorKind::InvalidRecord))
}

fn check_json_complexity(value: &Value, line: usize) -> Result<(), ParseError> {
    fn visit(
        value: &Value,
        depth: usize,
        nodes: &mut usize,
        line: usize,
    ) -> Result<(), ParseError> {
        *nodes += 1;
        if depth > MAX_JSON_DEPTH || *nodes > MAX_JSON_NODES_PER_RECORD {
            return Err(error(Some(line), ParseErrorKind::LimitExceeded));
        }
        match value {
            Value::String(s) if s.len() > MAX_STRING_BYTES => {
                Err(error(Some(line), ParseErrorKind::LimitExceeded))
            }
            Value::Array(values) => {
                for item in values {
                    visit(item, depth + 1, nodes, line)?;
                }
                Ok(())
            }
            Value::Object(values) => {
                for (key, item) in values {
                    if key.len() > MAX_STRING_BYTES {
                        return Err(error(Some(line), ParseErrorKind::LimitExceeded));
                    }
                    visit(item, depth + 1, nodes, line)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    let mut nodes = 0;
    visit(value, 0, &mut nodes, line)
}

fn classify_json_error(message: &str) -> ParseErrorKind {
    if message.contains("duplicate field")
        || message.contains("duplicate entry")
        || message.contains("duplicate key")
    {
        ParseErrorKind::DuplicateJsonKey
    } else if message.contains("number out of range") || message.contains("non-finite number") {
        ParseErrorKind::InvalidNumber
    } else {
        ParseErrorKind::MalformedJson
    }
}

fn error(line: Option<usize>, kind: ParseErrorKind) -> ParseError {
    ParseError { line, kind }
}

/// A small JSON value deserializer that rejects duplicate keys at every depth.
struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = UniqueValue;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value without duplicate object keys")
    }
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Bool(v)))
    }
    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(Number::from(v))))
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(Number::from(v))))
    }
    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
        Number::from_f64(v)
            .map(|n| UniqueValue(Value::Number(n)))
            .ok_or_else(|| E::custom("non-finite number"))
    }
    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(v.to_owned())))
    }
    fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(v)))
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        self.visit_unit()
    }
    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        UniqueValue::deserialize(deserializer)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element::<UniqueValue>()? {
            values.push(value.0);
        }
        Ok(UniqueValue(Value::Array(values)))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut values = Map::new();
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(serde::de::Error::custom("duplicate key"));
            }
            let value = map.next_value::<UniqueValue>()?;
            values.insert(key, value.0);
        }
        Ok(UniqueValue(Value::Object(values)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vendor(id: &str) -> String {
        format!(r#"{{"type":"vendor","id":"{id}","data":{{"name":"München Audio"}}}}"#)
    }
    fn product(id: &str, vendor_id: Option<&str>) -> String {
        let relation = vendor_id
            .map(|v| format!(",\"vendor_id\":\"{v}\""))
            .unwrap_or_default();
        format!(
            r#"{{"type":"product","id":"{id}","data":{{"name":"Headphone Δ"{relation},"type":"headphones","subtype":"over_the_ear"}}}}"#
        )
    }
    fn band(kind: &str, extra: &str) -> String {
        format!(r#"{{"type":"{kind}","frequency":1000{extra}}}"#)
    }
    fn eq_record(id: &str, product_id: Option<&str>, bands: &[String], extra: &str) -> String {
        let relation = product_id
            .map(|v| format!(",\"product_id\":\"{v}\""))
            .unwrap_or_default();
        format!(
            r#"{{"type":"eq","id":"{id}","data":{{"author":"Synthetic author"{relation},"type":"parametric_eq","parameters":{{"gain_db":0,"bands":[{}]}}{extra}}}}}"#,
            bands.join(",")
        )
    }
    fn parse_lines(lines: &[String]) -> Result<Catalog, ParseError> {
        parse_catalog(lines.join("\n").as_bytes())
    }

    #[test]
    fn accepts_synthetic_vendor_product_and_eq_in_any_order_preserving_unicode_and_provenance() {
        let bands = vec![
            band("peak_dip", r#","gain_db":-2.5,"q":1.2"#),
            band("low_shelf", r#","q":0.8"#),
            band("high_shelf", r#","gain_db":1.25,"q":0.7"#),
        ];
        let eq = eq_record(
            "eq.01",
            Some("product_1"),
            &bands,
            ",\"details\":\"Target • synthetic\",\"link\":\"https://example.invalid/source\"",
        );
        let catalog = parse_lines(&[
            eq,
            product("product_1", Some("vendor_1")),
            vendor("vendor_1"),
        ])
        .unwrap();
        let profile = &catalog.equalizers["eq.01"];
        assert_eq!(profile.data.author, "Synthetic author");
        assert_eq!(profile.data.details.as_deref(), Some("Target • synthetic"));
        assert_eq!(
            profile.data.link.as_deref(),
            Some("https://example.invalid/source")
        );
        assert_eq!(
            profile.data.parameters.bands[0].filter_type,
            OpraFilterType::PeakDip
        );
        assert_eq!(
            profile.data.parameters.bands[1].filter_type,
            OpraFilterType::LowShelf
        );
        assert_eq!(
            profile.data.parameters.bands[2].filter_type,
            OpraFilterType::HighShelf
        );
        assert_eq!(profile.data.parameters.bands[1].gain_db, None);
        assert!(matches!(
            profile.assess_compatibility().semantics,
            SemanticCompatibility {
                q_definition_unconfirmed: true,
                overall_gain_mapping_unconfirmed: true,
                ..
            }
        ));
    }

    #[test]
    fn recognizes_all_filter_types_and_classifies_unsupported_without_conversion() {
        let kinds = [
            "peak_dip",
            "high_shelf",
            "low_shelf",
            "low_pass",
            "high_pass",
            "band_pass",
            "band_stop",
        ];
        let bands = kinds
            .iter()
            .map(|kind| {
                let extra = if matches!(*kind, "low_pass" | "high_pass") {
                    r#","slope":12"#
                } else {
                    r#","q":1.0"#
                };
                band(kind, extra)
            })
            .collect::<Vec<_>>();
        let catalog = parse_lines(&[eq_record("eq", None, &bands, "")]).unwrap();
        assert_eq!(catalog.equalizers["eq"].data.parameters.bands.len(), 7);
        assert_eq!(
            catalog.equalizers["eq"].assess_compatibility().filter_types,
            FilterTypeCompatibility::Unsupported {
                band_indexes: vec![3, 4, 5, 6]
            }
        );
    }

    #[test]
    fn more_than_ten_bands_are_preserved_and_classified_not_truncated() {
        let bands = (0..11)
            .map(|_| band("peak_dip", r#","q":1.0"#))
            .collect::<Vec<_>>();
        let catalog = parse_lines(&[eq_record("eq", None, &bands, "")]).unwrap();
        assert_eq!(catalog.equalizers["eq"].data.parameters.bands.len(), 11);
        assert_eq!(
            catalog.equalizers["eq"].assess_compatibility().band_count,
            BandCountCompatibility::ExceedsTenBandLimit { count: 11 }
        );
    }

    #[test]
    fn rejects_duplicate_ids_broken_references_and_duplicate_json_keys() {
        let duplicate = vendor("v");
        assert_eq!(
            parse_lines(&[duplicate.clone(), duplicate])
                .unwrap_err()
                .kind,
            ParseErrorKind::DuplicateRecordId
        );
        assert_eq!(
            parse_lines(&[vendor("shared"), product("shared", None)])
                .unwrap_err()
                .kind,
            ParseErrorKind::DuplicateRecordId
        );
        assert_eq!(
            parse_lines(&[product("p", Some("missing"))])
                .unwrap_err()
                .kind,
            ParseErrorKind::BrokenReference
        );
        assert_eq!(
            parse_lines(&[eq_record(
                "e",
                Some("missing"),
                &[band("peak_dip", r#","q":1"#)],
                ""
            )])
            .unwrap_err()
            .kind,
            ParseErrorKind::BrokenReference
        );
        let dup_key = br#"{"type":"vendor","id":"v","data":{"name":"First","name":"Second"}}"#;
        assert_eq!(
            parse_catalog(dup_key).unwrap_err().kind,
            ParseErrorKind::DuplicateJsonKey
        );
    }

    #[test]
    fn optional_fields_may_be_absent_but_required_schema_fields_may_not() {
        let parsed = parse_lines(&[
            vendor("v"),
            product("p", None),
            eq_record("e", None, &[band("peak_dip", r#","q":1"#)], ""),
        ])
        .unwrap();
        assert_eq!(parsed.products["p"].data.vendor_id, None);
        assert_eq!(parsed.equalizers["e"].data.product_id, None);
        assert_eq!(parsed.equalizers["e"].data.details, None);
        let missing_author = r#"{"type":"eq","id":"e","data":{"type":"parametric_eq","parameters":{"gain_db":0,"bands":[]}}}"#;
        assert_eq!(
            parse_catalog(missing_author.as_bytes()).unwrap_err().kind,
            ParseErrorKind::InvalidRecord
        );
    }

    #[test]
    fn malformed_json_numbers_links_and_identifiers_are_rejected_with_typed_errors() {
        assert_eq!(
            parse_catalog(b"{bad json}\n").unwrap_err().kind,
            ParseErrorKind::MalformedJson
        );
        assert_eq!(
            parse_lines(&[eq_record(
                "bad/id",
                None,
                &[band("peak_dip", r#","q":1"#)],
                ""
            )])
            .unwrap_err()
            .kind,
            ParseErrorKind::InvalidIdentifier
        );
        let bad_url = eq_record(
            "e",
            None,
            &[band("peak_dip", r#","q":1"#)],
            ",\"link\":\"https://example.invalid/%XZ\"",
        );
        assert_eq!(
            parse_lines(&[bad_url]).unwrap_err().kind,
            ParseErrorKind::InvalidField
        );
        let bad_q = eq_record("e", None, &[band("peak_dip", r#","q":0.01"#)], "");
        assert_eq!(
            parse_lines(&[bad_q]).unwrap_err().kind,
            ParseErrorKind::InvalidNumber
        );
        let nan = br#"{"type":"eq","id":"e","data":{"author":"x","type":"parametric_eq","parameters":{"gain_db":NaN,"bands":[]}}}"#;
        assert_eq!(
            parse_catalog(nan).unwrap_err().kind,
            ParseErrorKind::MalformedJson
        );
        let overflow = br#"{"type":"eq","id":"e","data":{"author":"x","type":"parametric_eq","parameters":{"gain_db":1e999,"bands":[]}}}"#;
        assert_eq!(
            parse_catalog(overflow).unwrap_err().kind,
            ParseErrorKind::InvalidNumber
        );
        assert_eq!(
            parse_catalog(&[0xff]).unwrap_err().kind,
            ParseErrorKind::InvalidUtf8
        );
    }

    #[test]
    fn official_opra_namespaced_identifiers_keep_colon_and_ampersand_separators() {
        let catalog = parse_lines(&[
            vendor("maker"),
            product("maker:model", Some("maker")),
            eq_record(
                "maker:model::author&curve",
                Some("maker:model"),
                &[band("peak_dip", r#","q":1"#)],
                "",
            ),
        ])
        .expect("OPRA's generated namespaced IDs are valid identifiers");
        assert!(catalog.products.contains_key("maker:model"));
        assert!(catalog.equalizers.contains_key("maker:model::author&curve"));
    }

    #[test]
    fn reports_parameter_limits_without_converting_profile() {
        let bands = vec![band("peak_dip", r#","gain_db":20,"q":40"#)];
        let catalog = parse_lines(&[eq_record("e", None, &bands, "")]).unwrap();
        assert_eq!(
            catalog.equalizers["e"].assess_compatibility().parameters,
            ParameterCompatibility::OutsideEditorLimits {
                fields: vec![
                    ParameterIssue::Gain { band_index: 0 },
                    ParameterIssue::Q { band_index: 0 }
                ]
            }
        );
    }

    #[test]
    fn enforces_catalog_line_record_string_and_complexity_limits() {
        assert_eq!(
            parse_catalog(&vec![b' '; MAX_CATALOG_BYTES + 1])
                .unwrap_err()
                .kind,
            ParseErrorKind::CatalogTooLarge
        );
        let oversized_line = format!("{}\n{}", " ".repeat(MAX_LINE_BYTES + 1), vendor("after"));
        assert_eq!(
            parse_catalog(oversized_line.as_bytes()).unwrap_err().kind,
            ParseErrorKind::LineTooLarge
        );
        let large_text = format!(
            r#"{{"type":"vendor","id":"v","data":{{"name":"{}"}}}}"#,
            "x".repeat(MAX_STRING_BYTES + 1)
        );
        assert_eq!(
            parse_catalog(large_text.as_bytes()).unwrap_err().kind,
            ParseErrorKind::LimitExceeded
        );
        let too_many = (0..=MAX_RECORDS)
            .map(|index| vendor(&format!("v{index:05}")))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            parse_catalog(too_many.as_bytes()).unwrap_err().kind,
            ParseErrorKind::TooManyRecords
        );
        let nested = format!(
            r#"{{"type":"vendor","id":"v","data":{{"name":"ok","extra":{}}}}}"#,
            "[".repeat(MAX_JSON_DEPTH + 2) + "0" + &"]".repeat(MAX_JSON_DEPTH + 2)
        );
        assert_eq!(
            parse_catalog(nested.as_bytes()).unwrap_err().kind,
            ParseErrorKind::LimitExceeded
        );
    }

    #[test]
    fn all_or_nothing_catalog_error_never_returns_partial_success() {
        let text = format!("{}\n{{not-json}}", vendor("ok"));
        let result = parse_catalog(text.as_bytes());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().line, Some(2));
    }
}
