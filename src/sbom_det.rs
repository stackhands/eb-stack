//! Deterministic CycloneDX 1.5 documents on top of `cyclonedx-bom` 0.8.
//!
//! The models, hashes, purls and the JSON 1.5 writer are that crate's.
//! This one decides what is allowed to vary: a timestamp is written only
//! when the caller supplies one, `serialNumber` is a UUIDv5 over the
//! canonical document with the serial omitted, and [`Document::build`]
//! orders components, dependencies and properties before sealing.

use std::convert::TryFrom;
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
    /// ISO-8601 timestamp from the caller. `None`, or a value that does not
    /// parse, omits the field. The clock is never read.
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
    /// Attach any further sections, then [`seal`]. [`build`] is the entry
    /// that also sorts and seals.
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
            timestamp: self
                .timestamp
                .as_deref()
                .and_then(|text| DateTime::try_from(text.to_string()).ok()),
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
            components: Some(Components(self.components.iter().map(component).collect())),
            services: None,
            external_references: None,
            dependencies: Some(Dependencies(
                self.dependencies
                    .iter()
                    .map(|edge| Dependency {
                        dependency_ref: edge.bom_ref.clone(),
                        dependencies: unique_in_order(&edge.depends_on),
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
    #[allow(dead_code)] // used by this crate's tests; eb-stack calls `assemble`
    pub fn build(&self) -> Bom {
        let mut ordered = self.clone();
        ordered.order();
        let mut bom = ordered.assemble();
        seal(&mut bom);
        bom
    }

    /// JSON 1.5 bytes of [`build`].
    #[allow(dead_code)] // used by this crate's tests; eb-stack calls `assemble`
    pub fn to_bytes(&self) -> Vec<u8> {
        to_json_bytes(self.build())
    }

    #[allow(dead_code)] // only `build` sorts; eb-stack keeps lock order
    fn order(&mut self) {
        sort_pairs(&mut self.metadata_component.properties);
        sort_pairs(&mut self.metadata_properties);
        for component in &mut self.components {
            sort_pairs(&mut component.properties);
            component.sha256.sort();
            component.external_references.sort_by(|left, right| {
                (left.kind.rank(), &left.url, &left.comment).cmp(&(
                    right.kind.rank(),
                    &right.url,
                    &right.comment,
                ))
            });
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

#[allow(dead_code)] // only `Document::order` sorts properties
fn sort_pairs(pairs: &mut [(String, String)]) {
    pairs.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
}

impl ExternalKind {
    #[allow(dead_code)] // only `Document::order` sorts references
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
