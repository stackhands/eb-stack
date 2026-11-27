//! Deterministic CycloneDX 1.5 documents on top of `cyclonedx-bom` 0.8.
//!
//! The models, hashes, purls and the JSON 1.5 writer are that crate's.
//! This one decides what is allowed to vary: a timestamp is written only
//! when the caller supplies an RFC 3339 date-time, or a form that
//! normalises to one with `Z` or a colon offset. `serialNumber` is a
//! UUIDv5 over the canonical document with the serial omitted, and
//! [`Document::build`] orders components, dependencies and properties
//! before sealing. Identical components and dependency edges are kept
//! once, in the order they first appeared.

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
    /// Identical components and dependency edges are kept once; the first
    /// occurrence wins. Attach any further sections, then [`seal`].
    /// [`build`] is the entry that also sorts and seals.
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
                keep_first(&self.components).iter().map(component).collect(),
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
            sort_pairs(&mut component.properties);
            component.sha256.sort();
            sort_external_refs(&mut component.external_references);
        }
        self.components.sort_by(|left, right| {
            (&left.bom_ref, &left.name, &left.version).cmp(&(
                &right.bom_ref,
                &right.name,
                &right.version,
            ))
        });
        for edge in &mut self.dependencies {
            edge.depends_on.sort();
            edge.depends_on.dedup();
        }
        self.dependencies
            .sort_by(|left, right| left.bom_ref.cmp(&right.bom_ref));
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
    let candidate = value.trim();
    if candidate.len() != 64 || !candidate.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(Hash {
        alg: HashAlgorithm::SHA_256,
        content: HashValue(candidate.to_ascii_lowercase()),
    })
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

fn keep_first<T: Clone + PartialEq>(items: &[T]) -> Vec<T> {
    let mut kept = Vec::with_capacity(items.len());
    for item in items {
        if !kept.contains(item) {
            kept.push(item.clone());
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
