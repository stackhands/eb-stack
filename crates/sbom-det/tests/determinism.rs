//! The same inputs, including across a moved wall clock, are one document.

use sbom_det::{
    ComponentKind, ComponentSpec, DependencyEdge, Document, ExternalKind, ExternalRef,
    LifecyclePhase, ToolSpec,
};

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
