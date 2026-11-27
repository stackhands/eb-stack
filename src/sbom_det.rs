//! Deterministic CycloneDX 1.5 documents on top of `cyclonedx-bom` 0.8.
//!
//! The models, hashes, purls and the JSON 1.5 writer are that crate's.
//! This one decides what is allowed to vary: a timestamp is written only
//! when the caller supplies an RFC 3339 date-time, or a form that
//! normalises to one with `Z` or a colon offset. `serialNumber` is a
//! UUIDv5 over the canonical document with the serial omitted, and
//! [`Document::build`] orders components, dependencies and properties
//! before sealing. Components that share a bom-ref, name and version, and
//! dependency edges that share a bom-ref, then sort on the canonical JSON
//! of the whole value, so a reversed input cannot change the sealed bytes.
//! Each component is reduced to the serialised object first (invalid
//! SHA-256 values dropped, hex lowercased), and identical objects are kept
//! once before that sort. Dependency edges are kept once, in the order they
//! first appeared.

use std::str::FromStr;

use cyclonedx_bom::external_models::date_time::DateTime;
use cyclonedx_bom::external_models::normalized_string::NormalizedString;
use cyclonedx_bom::external_models::uri::{Purl, Uri};
use cyclonedx_bom::models::component::{Classification, Component, Components};
use cyclonedx_bom::models::dependency::{Dependencies, Dependency};
use cyclonedx_bom::models::external_reference::{
    ExternalReference, ExternalReferenceType, ExternalReferences, Uri as ExternalUri,
};
use cyclonedx_bom::models::hash::{Hash, HashAlgorithm, HashValue, Hashes};
use cyclonedx_bom::models::lifecycle::{Lifecycle, Lifecycles, Phase};
use cyclonedx_bom::models::metadata::Metadata;
use cyclonedx_bom::models::property::{Properties, Property};
use cyclonedx_bom::models::tool::{Tool, Tools};
use cyclonedx_bom::prelude::{Bom, SpecVersion, UrnUuid};
use uuid::Uuid;

/// Library or application. Other CycloneDX classes stay with the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentKind {
    /// A library, module or other linked unit.
    Library,
    /// The product the document describes.
    Application,
}

/// Where an external reference points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalKind {
    /// A download of the artifact.
    Distribution,
    /// A version-control location.
    Vcs,
    /// A website.
    Website,
}

/// One component, independent of any package ecosystem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentSpec {
    /// CycloneDX component type.
    pub kind: ComponentKind,
    /// Component name.
    pub name: String,
    /// Component version.
    pub version: String,
    /// `bom-ref` for this component.
    pub bom_ref: String,
    /// Package URL. Dropped when it does not parse.
    pub purl: Option<String>,
    /// Prose description. Omitted when absent.
    pub description: Option<String>,
    /// SHA-256 digests, hex. Values that are not 64 hex digits are dropped.
    pub sha256: Vec<String>,
    /// Name/value properties, in the order to emit when not sorting.
    pub properties: Vec<(String, String)>,
    /// External references. A URL that does not parse is dropped.
    pub external_references: Vec<ExternalRef>,
}

/// One URL attached to a component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalRef {
    /// Reference class.
    pub kind: ExternalKind,
    /// Absolute URL.
    pub url: String,
    /// Optional comment stored verbatim.
    pub comment: Option<String>,
}

/// A tool named in document metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSpec {
    /// Tool vendor.
    pub vendor: String,
    /// Tool name.
    pub name: String,
    /// Tool version.
    pub version: String,
}

/// Lifecycle phase written on the document. Only the phases this builder
/// needs are listed; the caller maps its own vocabulary onto these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecyclePhase {
    /// The document describes a plan, not a finished build.
    PreBuild,
}

/// One `dependencies` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependencyEdge {
    /// `bom-ref` of the dependent.
    pub bom_ref: String,
    /// `bom-ref`s it depends on. Duplicates collapse, first occurrence wins,
    /// unless [`Document::build`] sorted them first.
    pub depends_on: Vec<String>,
}

/// Inputs for one CycloneDX document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    /// CycloneDX document version (the `version` integer, not the spec).
    pub version: u32,
    /// Timestamp from the caller. `None`, a value that does not parse, or a
    /// form that cannot be written as RFC 3339 with `Z` or a colon offset
    /// omits the field. The clock is never read.
    pub timestamp: Option<String>,
    /// Tools that produced the document.
    pub tools: Vec<ToolSpec>,
    /// The component this document is about.
    pub metadata_component: ComponentSpec,
    /// Document-level properties.
    pub metadata_properties: Vec<(String, String)>,
    /// Lifecycle, when the caller has one.
    pub lifecycle: Option<LifecyclePhase>,
    /// Components other than the metadata component.
    pub components: Vec<ComponentSpec>,
    /// Dependency edges.
    pub dependencies: Vec<DependencyEdge>,
}

impl Document {
    /// Build a document in the caller's order, with no serial number.
    ///
    /// Components that serialise to the same object, and identical
    /// dependency edges, are kept once. The first occurrence wins. Attach
    /// any further sections, then [`seal`]. [`build`] is the entry that
    /// also sorts and seals.
    pub fn assemble(&self) -> Bom {
        let tools = if self.tools.is_empty() {
            None
        } else {
            Some(Tools::List(
                self.tools
                    .iter()
                    .map(|tool| Tool::new(&tool.vendor, &tool.name, &tool.version))
                    .collect(),
            ))
        };
        let lifecycles = self
            .lifecycle
            .map(|phase| Lifecycles(vec![Lifecycle::Phase(phase.into())]));
        let metadata = Metadata {
            timestamp: self.timestamp.as_deref().and_then(rfc3339_timestamp),
            tools,
            component: Some(component(&self.metadata_component)),
            properties: properties_of(&self.metadata_properties),
            lifecycles,
            ..Default::default()
        };

        Bom {
            version: self.version,
            serial_number: None,
            metadata: Some(metadata),
            components: Some(Components(
                keep_emitted(&self.components)
                    .iter()
                    .map(component)
                    .collect(),
            )),
            services: None,
            external_references: None,
            dependencies: Some(Dependencies(
                dependency_edges(&self.dependencies)
                    .into_iter()
                    .map(|edge| Dependency {
                        dependency_ref: edge.bom_ref,
                        dependencies: edge.depends_on,
                    })
                    .collect(),
            )),
            compositions: None,
            properties: None,
            vulnerabilities: None,
            signature: None,
            annotations: None,
            formulation: None,
            spec_version: SpecVersion::V1_5,
        }
    }

    /// Sort components, dependencies and properties, then assemble and seal.
    ///
    /// The same inputs produce the same bytes, including when the wall clock
    /// has moved between calls.
    pub fn build(&self) -> Bom {
        let mut ordered = self.clone();
        ordered.order();
        let mut bom = ordered.assemble();
        seal(&mut bom);
        bom
    }

    /// JSON 1.5 bytes of [`build`].
    pub fn to_bytes(&self) -> Vec<u8> {
        to_json_bytes(self.build())
    }

    fn order(&mut self) {
        normalize_emitted(&mut self.metadata_component);
        sort_pairs(&mut self.metadata_component.properties);
        self.metadata_component.sha256.sort();
        sort_external_refs(&mut self.metadata_component.external_references);
        self.tools.sort_by(|left, right| {
            (&left.vendor, &left.name, &left.version).cmp(&(
                &right.vendor,
                &right.name,
                &right.version,
            ))
        });
        sort_pairs(&mut self.metadata_properties);
        for component in &mut self.components {
            normalize_emitted(component);
            sort_pairs(&mut component.properties);
            component.sha256.sort();
            sort_external_refs(&mut component.external_references);
        }
        // Dedup on the serialised object, then order. bom-ref, name and
        // version are not a total key. Canonical JSON of the whole value
        // orders only the ties, so a distinct key stays put.
        self.components = keep_emitted(&self.components);
        self.components.sort_by(|left, right| {
            (&left.bom_ref, &left.name, &left.version)
                .cmp(&(&right.bom_ref, &right.name, &right.version))
                .then_with(|| {
                    canonical_json(&component_value(left))
                        .cmp(&canonical_json(&component_value(right)))
                })
        });
        for edge in &mut self.dependencies {
            edge.depends_on.sort();
            edge.depends_on.dedup();
        }
        self.dependencies.sort_by(|left, right| {
            left.bom_ref.cmp(&right.bom_ref).then_with(|| {
                canonical_json(&edge_value(left)).cmp(&canonical_json(&edge_value(right)))
            })
        });
    }
}

/// Percent-encode one package-URL qualifier value.
///
/// Unreserved characters stay as themselves. Everything else is `%HH` with
/// an uppercase hex byte, which is the encoding package URLs require.
pub fn encode_purl_qualifier(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// A SHA-256 digest as CycloneDX wants it, or nothing.
///
/// Only a 64-digit hex string is a SHA-256. Anything else would be a hash
/// of the wrong kind.
pub fn sha256_hash(value: &str) -> Option<Hash> {
    Some(Hash {
        alg: HashAlgorithm::SHA_256,
        content: HashValue(canonical_sha256(value)?),
    })
}

fn canonical_sha256(value: &str) -> Option<String> {
    let candidate = value.trim();
    if candidate.len() != 64 || !candidate.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(candidate.to_ascii_lowercase())
}

/// Set `serialNumber` to a UUIDv5 over the canonical 1.5 JSON of `bom`
/// with the serial cleared. Other fields are left as they are.
///
/// The namespace is the RFC 4122 URL namespace. Sealing twice yields the
/// same serial, because the canonical bytes exclude the serial itself.
pub fn seal(bom: &mut Bom) {
    bom.serial_number = None;
    let canonical = to_json_bytes(bom.clone());
    let id = Uuid::new_v5(&Uuid::NAMESPACE_URL, &canonical);
    bom.serial_number = Some(UrnUuid::from(id));
}

/// Serialize `bom` with the cyclonedx-bom JSON 1.5 writer and parse it back.
pub fn to_json_value(bom: Bom) -> serde_json::Value {
    let bytes = to_json_bytes(bom);
    serde_json::from_slice(&bytes).expect("cyclonedx JSON is valid serde_json::Value")
}

fn to_json_bytes(bom: Bom) -> Vec<u8> {
    let mut buf = Vec::new();
    bom.output_as_json_v1_5(&mut buf)
        .expect("cyclonedx-bom JSON 1.5 serialize");
    buf
}

fn component(spec: &ComponentSpec) -> Component {
    let mut component = Component::new(
        spec.kind.into(),
        &spec.name,
        &spec.version,
        Some(spec.bom_ref.clone()),
    );
    if let Some(purl) = spec.purl.as_deref() {
        component.purl = Purl::from_str(purl).ok();
    }
    if let Some(description) = spec.description.as_deref() {
        component.description = Some(NormalizedString::new(description));
    }
    let hashes: Vec<Hash> = spec
        .sha256
        .iter()
        .filter_map(|value| sha256_hash(value))
        .collect();
    if !hashes.is_empty() {
        component.hashes = Some(Hashes(hashes));
    }
    component.properties = properties_of(&spec.properties);
    let refs: Vec<ExternalReference> = spec
        .external_references
        .iter()
        .filter_map(|reference| {
            Uri::try_from(reference.url.clone())
                .ok()
                .map(|uri| ExternalReference {
                    external_reference_type: reference.kind.into(),
                    url: ExternalUri::Url(uri),
                    comment: reference.comment.clone(),
                    hashes: None,
                })
        })
        .collect();
    if !refs.is_empty() {
        component.external_references = Some(ExternalReferences(refs));
    }
    component
}

fn properties_of(pairs: &[(String, String)]) -> Option<Properties> {
    if pairs.is_empty() {
        None
    } else {
        Some(Properties(
            pairs
                .iter()
                .map(|(name, value)| Property::new(name, value))
                .collect(),
        ))
    }
}

fn unique_in_order(items: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        if !out.contains(item) {
            out.push(item.clone());
        }
    }
    out
}

fn sort_pairs(pairs: &mut [(String, String)]) {
    pairs.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
}

/// Bytes that order two values when their sort key is equal.
///
/// Object keys are sorted by `serde_json`, and every field of the value is
/// included, so unequal values do not compare equal.
fn canonical_json(value: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("canonical json")
}

fn component_value(spec: &ComponentSpec) -> serde_json::Value {
    serde_json::json!({
        "bom_ref": spec.bom_ref,
        "description": spec.description,
        "external_references": spec
            .external_references
            .iter()
            .map(external_ref_value)
            .collect::<Vec<_>>(),
        "kind": match spec.kind {
            ComponentKind::Library => "library",
            ComponentKind::Application => "application",
        },
        "name": spec.name,
        "properties": spec.properties,
        "purl": spec.purl,
        "sha256": spec.sha256,
        "version": spec.version,
    })
}

fn external_ref_value(reference: &ExternalRef) -> serde_json::Value {
    serde_json::json!({
        "comment": reference.comment,
        "kind": match reference.kind {
            ExternalKind::Distribution => "distribution",
            ExternalKind::Vcs => "vcs",
            ExternalKind::Website => "website",
        },
        "url": reference.url,
    })
}

fn edge_value(edge: &DependencyEdge) -> serde_json::Value {
    serde_json::json!({
        "bom_ref": edge.bom_ref,
        "depends_on": edge.depends_on,
    })
}

impl ExternalKind {
    fn rank(self) -> u8 {
        match self {
            Self::Distribution => 0,
            Self::Vcs => 1,
            Self::Website => 2,
        }
    }
}

impl From<ComponentKind> for Classification {
    fn from(kind: ComponentKind) -> Self {
        match kind {
            ComponentKind::Library => Self::Library,
            ComponentKind::Application => Self::Application,
        }
    }
}

impl From<ExternalKind> for ExternalReferenceType {
    fn from(kind: ExternalKind) -> Self {
        match kind {
            ExternalKind::Distribution => Self::Distribution,
            ExternalKind::Vcs => Self::Vcs,
            ExternalKind::Website => Self::Website,
        }
    }
}

impl From<LifecyclePhase> for Phase {
    fn from(phase: LifecyclePhase) -> Self {
        match phase {
            LifecyclePhase::PreBuild => Self::PreBuild,
        }
    }
}

fn normalize_text(value: &str) -> String {
    NormalizedString::new(value).to_string()
}

/// Rewrite `spec` to the fields [`component`] serialises.
fn normalize_emitted(spec: &mut ComponentSpec) {
    spec.name = normalize_text(&spec.name);
    spec.version = normalize_text(&spec.version);
    spec.description = spec.description.as_deref().map(normalize_text);
    spec.sha256 = spec
        .sha256
        .iter()
        .filter_map(|value| canonical_sha256(value))
        .collect();
    spec.external_references
        .retain(|reference| Uri::try_from(reference.url.clone()).is_ok());
}

/// First spec for each object [`component`] serialises. Later copies go.
fn keep_emitted(specs: &[ComponentSpec]) -> Vec<ComponentSpec> {
    let mut kept = Vec::with_capacity(specs.len());
    let mut seen = Vec::with_capacity(specs.len());
    for spec in specs {
        let mut spec = spec.clone();
        normalize_emitted(&mut spec);
        let emitted = component(&spec);
        if !seen.contains(&emitted) {
            seen.push(emitted);
            kept.push(spec);
        }
    }
    kept
}

fn dependency_edges(edges: &[DependencyEdge]) -> Vec<DependencyEdge> {
    let mut kept = Vec::with_capacity(edges.len());
    for edge in edges {
        let next = DependencyEdge {
            bom_ref: edge.bom_ref.clone(),
            depends_on: unique_in_order(&edge.depends_on),
        };
        if !kept.contains(&next) {
            kept.push(next);
        }
    }
    kept
}

fn sort_external_refs(refs: &mut [ExternalRef]) {
    refs.sort_by(|left, right| {
        (left.kind.rank(), &left.url, &left.comment).cmp(&(
            right.kind.rank(),
            &right.url,
            &right.comment,
        ))
    });
}

/// A timestamp the CycloneDX 1.5 date-time schema will accept.
///
/// Basic dates, offsets without a colon, and times without seconds are
/// expanded. A string that is already RFC 3339 is returned unchanged.
/// Anything that cannot be written with `Z` or `±HH:MM` is refused.
fn rfc3339_timestamp(text: &str) -> Option<DateTime> {
    let normalized = normalize_rfc3339(text.trim())?;
    if !schema_datetime(&normalized) {
        return None;
    }
    DateTime::try_from(normalized).ok()
}

fn normalize_rfc3339(text: &str) -> Option<String> {
    let split = text.find(['T', 't'])?;
    let date = normalize_date(&text[..split])?;
    let (time, offset) = normalize_clock(&text[split + 1..])?;
    Some(format!("{date}T{time}{offset}"))
}

fn normalize_date(date: &str) -> Option<String> {
    let bytes = date.as_bytes();
    if bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && all_digits(&date[..4])
        && all_digits(&date[5..7])
        && all_digits(&date[8..10])
    {
        return Some(date.to_string());
    }
    if bytes.len() == 8 && all_digits(date) {
        return Some(format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..8]));
    }
    None
}

fn normalize_clock(rest: &str) -> Option<(String, String)> {
    let offset_at = rest
        .bytes()
        .position(|byte| matches!(byte, b'+' | b'-' | b'Z' | b'z'))?;
    let body = rest[..offset_at].replace(',', ".");
    let offset = normalize_offset(&rest[offset_at..])?;
    Some((normalize_hms(&body)?, offset))
}

fn normalize_offset(offset: &str) -> Option<String> {
    if offset == "Z" || offset == "z" {
        return Some("Z".to_string());
    }
    let sign = offset.chars().next()?;
    if sign != '+' && sign != '-' {
        return None;
    }
    let rest = &offset[1..];
    let bytes = rest.as_bytes();
    if bytes.len() == 5 && bytes[2] == b':' && all_digits(&rest[..2]) && all_digits(&rest[3..]) {
        return Some(offset.to_string());
    }
    if bytes.len() == 4 && all_digits(rest) {
        return Some(format!("{sign}{}:{}", &rest[..2], &rest[2..]));
    }
    if bytes.len() == 2 && all_digits(rest) {
        return Some(format!("{sign}{rest}:00"));
    }
    None
}

fn normalize_hms(body: &str) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let (main, fraction) = match body.split_once('.') {
        Some((main, fraction)) if !fraction.is_empty() && all_digits(fraction) => {
            (main, Some(fraction))
        }
        Some(_) => return None,
        None => (body, None),
    };
    let clock = if main.contains(':') {
        let mut parts = main.split(':');
        let hour = parts.next()?;
        let minute = parts.next()?;
        let second = parts.next();
        if parts.next().is_some() || !two_digits(hour) || !two_digits(minute) {
            return None;
        }
        match second {
            Some(second) if two_digits(second) => format!("{hour}:{minute}:{second}"),
            None => format!("{hour}:{minute}:00"),
            Some(_) => return None,
        }
    } else if main.len() == 6 && all_digits(main) {
        format!("{}:{}:{}", &main[..2], &main[2..4], &main[4..])
    } else if main.len() == 4 && all_digits(main) {
        format!("{}:{}:00", &main[..2], &main[2..])
    } else {
        return None;
    };
    match fraction {
        Some(fraction) => Some(format!("{clock}.{fraction}")),
        None => Some(clock),
    }
}

fn schema_datetime(value: &str) -> bool {
    let Some(split) = value.find('T') else {
        return false;
    };
    schema_date(&value[..split]) && schema_time(&value[split + 1..])
}

fn schema_date(date: &str) -> bool {
    if date.len() != 10 {
        return false;
    }
    let Some(year) = parse_four(&date[..4]) else {
        return false;
    };
    let Some(month) = parse_two(&date[5..7]) else {
        return false;
    };
    let Some(day) = parse_two(&date[8..10]) else {
        return false;
    };
    if date.as_bytes()[4] != b'-' || date.as_bytes()[7] != b'-' {
        return false;
    }
    if !(1..=12).contains(&month) || day == 0 {
        return false;
    }
    let max = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
    };
    day <= max
}

fn schema_time(time: &str) -> bool {
    let bytes = time.as_bytes();
    if bytes.len() < 9 || bytes[2] != b':' || bytes[5] != b':' {
        return false;
    }
    let Some(hour) = parse_two(&time[..2]) else {
        return false;
    };
    let Some(minute) = parse_two(&time[3..5]) else {
        return false;
    };
    let Some(second) = parse_two(&time[6..8]) else {
        return false;
    };
    if hour > 23 || minute > 59 || second > 60 {
        return false;
    }
    let mut index = 8;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index == start {
            return false;
        }
    }
    if index >= bytes.len() {
        return false;
    }
    match bytes[index] {
        b'Z' => index + 1 == bytes.len() && (second != 60 || (hour == 23 && minute == 59)),
        b'+' => offset_ok(time, index, hour, minute, second, true),
        b'-' => offset_ok(time, index, hour, minute, second, false),
        _ => false,
    }
}

fn offset_ok(time: &str, index: usize, hour: u8, minute: u8, second: u8, plus: bool) -> bool {
    let bytes = time.as_bytes();
    if bytes.len() - index != 6 || bytes[index + 3] != b':' {
        return false;
    }
    let Some(offset_hour) = parse_two(&time[index + 1..index + 3]) else {
        return false;
    };
    let Some(offset_minute) = parse_two(&time[index + 4..index + 6]) else {
        return false;
    };
    if offset_hour > 23 || offset_minute > 59 {
        return false;
    }
    if second != 60 {
        return true;
    }
    let mut utc_hour = i16::from(hour);
    let mut utc_minute = i16::from(minute);
    let off_hour = i16::from(offset_hour);
    let off_minute = i16::from(offset_minute);
    if plus {
        utc_hour -= off_hour;
        utc_minute -= off_minute;
    } else {
        utc_hour += off_hour;
        utc_minute += off_minute;
    }
    utc_hour += utc_minute / 60;
    utc_minute %= 60;
    if utc_minute < 0 {
        utc_minute += 60;
        utc_hour -= 1;
    }
    utc_hour = (utc_hour + 24) % 24;
    utc_hour == 23 && utc_minute == 59
}

fn is_leap_year(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn parse_two(text: &str) -> Option<u8> {
    let bytes = text.as_bytes();
    if bytes.len() != 2 || !bytes[0].is_ascii_digit() || !bytes[1].is_ascii_digit() {
        return None;
    }
    Some((bytes[0] - b'0') * 10 + (bytes[1] - b'0'))
}

fn parse_four(text: &str) -> Option<u16> {
    let bytes = text.as_bytes();
    if bytes.len() != 4 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(
        u16::from(bytes[0] - b'0') * 1000
            + u16::from(bytes[1] - b'0') * 100
            + u16::from(bytes[2] - b'0') * 10
            + u16::from(bytes[3] - b'0'),
    )
}

fn two_digits(text: &str) -> bool {
    text.len() == 2 && all_digits(text)
}

fn all_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod determinism {
    use super::*;

    fn library(name: &str, version: &str) -> ComponentSpec {
        ComponentSpec {
            kind: ComponentKind::Library,
            name: name.to_string(),
            version: version.to_string(),
            bom_ref: format!("pkg:generic/{name}@{version}"),
            purl: Some(format!("pkg:generic/{name}@{version}")),
            description: None,
            sha256: vec!["ab".repeat(32)],
            properties: vec![
                ("zeta".to_string(), "1".to_string()),
                ("alpha".to_string(), "2".to_string()),
            ],
            external_references: vec![ExternalRef {
                kind: ExternalKind::Website,
                url: format!("https://example.com/{name}"),
                comment: None,
            }],
        }
    }

    fn sample() -> Document {
        Document {
            version: 1,
            timestamp: None,
            tools: vec![ToolSpec {
                vendor: "example".to_string(),
                name: "sbom-det".to_string(),
                version: "0.1.0".to_string(),
            }],
            metadata_component: ComponentSpec {
                kind: ComponentKind::Application,
                name: "demo".to_string(),
                version: "1.0.0".to_string(),
                bom_ref: "pkg:generic/demo@1.0.0".to_string(),
                purl: Some("pkg:generic/demo@1.0.0".to_string()),
                description: Some("demo".to_string()),
                sha256: vec!["cd".repeat(32)],
                properties: vec![
                    ("zeta".to_string(), "1".to_string()),
                    ("alpha".to_string(), "2".to_string()),
                ],
                external_references: vec![],
            },
            metadata_properties: vec![("kind".to_string(), "test".to_string())],
            lifecycle: Some(LifecyclePhase::PreBuild),
            components: vec![library("zlib", "1.3.1"), library("bzip2", "1.0.8")],
            dependencies: vec![DependencyEdge {
                bom_ref: "pkg:generic/demo@1.0.0".to_string(),
                depends_on: vec![
                    "pkg:generic/zlib@1.3.1".to_string(),
                    "pkg:generic/bzip2@1.0.8".to_string(),
                ],
            }],
        }
    }

    fn serial_number(bytes: &[u8]) -> String {
        let value: serde_json::Value = serde_json::from_slice(bytes).expect("json");
        value["serialNumber"]
            .as_str()
            .expect("serialNumber")
            .to_string()
    }

    #[test]
    fn the_same_input_built_twice_matches_across_a_changed_clock() {
        let doc = sample();
        let first = doc.to_bytes();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let second = doc.to_bytes();
        assert_eq!(first, second);
        let serial = serial_number(&first);
        assert_eq!(serial, serial_number(&second));
        assert!(serial.starts_with("urn:uuid:"), "{serial}");
        let text = String::from_utf8(first.clone()).expect("utf-8");
        assert!(!text.contains("\"timestamp\""));

        let mut stamped = doc.clone();
        stamped.timestamp = Some("2024-01-02T03:04:05Z".to_string());
        let stamped_first = stamped.to_bytes();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let stamped_second = stamped.to_bytes();
        assert_eq!(stamped_first, stamped_second);
        assert_ne!(stamped_first, first);
        let stamped_text = String::from_utf8(stamped_first).expect("utf-8");
        assert!(stamped_text.contains("2024-01-02T03:04:05Z"));
        assert_eq!(
            serial_number(&stamped_second),
            serial_number(stamped_text.as_bytes())
        );

        let mut flipped = doc;
        flipped.components.reverse();
        flipped.metadata_component.properties.reverse();
        flipped.components[0].properties.reverse();
        flipped.dependencies[0].depends_on.reverse();
        assert_eq!(sample().to_bytes(), flipped.to_bytes());

        // Basic and colon-less forms are rewritten to RFC 3339. One row: the
        // basic date the CycloneDX date-time schema rejects as written.
        let stamps = [("20260814T000000Z", "2026-08-14T00:00:00Z")];
        for (input, expect) in stamps {
            let mut basic = sample();
            basic.timestamp = Some(input.to_string());
            let text = String::from_utf8(basic.to_bytes()).expect("utf-8");
            assert!(text.contains(expect), "{input} -> {text}");
            assert!(!text.contains(input), "{input} was stored unchanged");
        }

        // Metadata hashes, external refs, and tools sort, so input order cannot
        // change the sealed bytes. One row covers those three fields together.
        let orderings = [(
            "metadata hashes, external refs, and tools",
            |doc: &mut Document| {
                doc.metadata_component.sha256.reverse();
                doc.metadata_component.external_references.reverse();
                doc.tools.reverse();
            },
        )];
        for (label, flip) in orderings {
            let mut ordered = sample();
            ordered.metadata_component.sha256 = vec!["11".repeat(32), "ff".repeat(32)];
            ordered.metadata_component.external_references = vec![
                ExternalRef {
                    kind: ExternalKind::Website,
                    url: "https://example.com/z".to_string(),
                    comment: None,
                },
                ExternalRef {
                    kind: ExternalKind::Distribution,
                    url: "https://example.com/a".to_string(),
                    comment: Some("source".to_string()),
                },
            ];
            ordered.tools.push(ToolSpec {
                vendor: "aaa".to_string(),
                name: "earlier".to_string(),
                version: "0.0.1".to_string(),
            });
            let mut reversed = ordered.clone();
            flip(&mut reversed);
            assert_ne!(
                ordered.metadata_component.sha256, reversed.metadata_component.sha256,
                "{label}"
            );
            assert_eq!(ordered.to_bytes(), reversed.to_bytes(), "{label}");
        }
    }

    #[test]
    fn tied_keys_match_when_the_input_is_reversed() {
        let mut low = library("twin", "1.0.0");
        low.sha256 = vec!["11".repeat(32)];
        let mut high = library("twin", "1.0.0");
        high.sha256 = vec!["ff".repeat(32)];
        let mut forward = sample();
        forward.components = vec![low, high];
        forward.dependencies = vec![
            DependencyEdge {
                bom_ref: "pkg:generic/demo@1.0.0".to_string(),
                depends_on: vec!["pkg:generic/bzip2@1.0.8".to_string()],
            },
            DependencyEdge {
                bom_ref: "pkg:generic/demo@1.0.0".to_string(),
                depends_on: vec!["pkg:generic/xz@5.4.0".to_string()],
            },
        ];
        let mut backward = forward.clone();
        backward.components.reverse();
        backward.dependencies.reverse();
        assert_ne!(
            forward.components[0].sha256, backward.components[0].sha256,
            "the reversed components must differ"
        );
        assert_ne!(
            forward.dependencies[0].depends_on, backward.dependencies[0].depends_on,
            "the reversed edges must differ"
        );
        let forward_bytes = forward.to_bytes();
        let backward_bytes = backward.to_bytes();
        assert_eq!(forward_bytes, backward_bytes);
        assert_eq!(
            serial_number(&forward_bytes),
            serial_number(&backward_bytes)
        );
    }

    #[test]
    fn emitted_components_are_unique() {
        let digest = "ab".repeat(32);

        let mut invalid = sample();
        let mut bad_low = library("twin", "1.0.0");
        bad_low.sha256 = vec!["aa".to_string()];
        let mut bad_high = library("twin", "1.0.0");
        bad_high.sha256 = vec!["bb".to_string()];
        invalid.components = vec![bad_low, bad_high];

        let mut folded = sample();
        let mut upper = library("twin", "1.0.0");
        upper.sha256 = vec![digest.to_ascii_uppercase()];
        let mut lower = library("twin", "1.0.0");
        lower.sha256 = vec![digest.clone()];
        folded.components = vec![upper, lower];

        let mut distinct = sample();
        let mut low = library("twin", "1.0.0");
        low.sha256 = vec!["11".repeat(32)];
        let mut high = library("twin", "1.0.0");
        high.sha256 = vec!["ff".repeat(32)];
        distinct.components = vec![low, high];

        let cases = [
            ("invalid hash", invalid, 1, None),
            ("sha256 letter case", folded, 1, Some(digest.as_str())),
            ("distinct digests", distinct, 2, None),
        ];
        for (label, doc, count, kept_digest) in cases {
            let value: serde_json::Value = serde_json::from_slice(&doc.to_bytes()).expect("json");
            let components = value["components"].as_array().expect("components");
            assert_eq!(components.len(), count, "{label}");
            for (index, left) in components.iter().enumerate() {
                for right in components.iter().skip(index + 1) {
                    assert_ne!(left, right, "{label}");
                }
            }
            if let Some(kept_digest) = kept_digest {
                let text = components[0].to_string();
                assert!(text.contains(kept_digest), "{label}");
                assert!(!text.contains(&kept_digest.to_ascii_uppercase()), "{label}");
            } else if count == 1 {
                assert!(components[0].get("hashes").is_none(), "{label}");
            }
        }
    }
}
