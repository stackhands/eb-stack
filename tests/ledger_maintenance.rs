//! `ledger backfill`, `ledger anchor`, `ledger verify-anchors` and
//! `ledger project` against real ledgers written by `ledger append` and
//! `ledger head`, through the library and through the binary.
#![cfg(unix)]

use eb_stack::ledger::{self, SignedHead, VerifyMode, VerifyRequest};
use eb_stack::ledger_maintenance::{self as lm, Failure};
use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use ed25519_dalek::pkcs8::{EncodePrivateKey, EncodePublicKey};
use ed25519_dalek::SigningKey;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const DAY1: i64 = 1_793_707_200; // 2026-11-03T12:00:00Z
const DAY2: i64 = DAY1 + 86_400;
const DAY3: i64 = DAY2 + 86_400;
const OBSERVED: &str = "2026-11-02T09:30:00Z";

/// Pipes in a Markdown table row that are not backslash-escaped.
fn cell_separators(line: &str) -> usize {
    let (mut count, mut escaped) = (0, false);
    for ch in line.chars() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '|' {
            count += 1;
        }
    }
    count
}

/// Every file under `root` with its bytes, by relative path.
fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, directory: &Path, found: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(directory).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(root, &path, found);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                found.insert(relative, std::fs::read(&path).expect("read"));
            }
        }
    }
    let mut found = BTreeMap::new();
    walk(root, root, &mut found);
    found
}

fn write_key(key: &Path, public: &Path, seed: u8) {
    let signing = SigningKey::from_bytes(&[seed; 32]);
    std::fs::write(
        key,
        signing
            .to_pkcs8_pem(LineEnding::LF)
            .expect("pem")
            .as_bytes(),
    )
    .expect("write key");
    std::fs::write(
        public,
        signing
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .expect("spki"),
    )
    .expect("write public key");
    std::fs::set_permissions(key, std::fs::Permissions::from_mode(0o400)).expect("chmod key");
}

fn failure(result: &anyhow::Result<Value>) -> Failure {
    match result {
        Ok(value) => panic!("expected a failure, got {value}"),
        Err(error) => {
            lm::failure_of(error).unwrap_or_else(|| panic!("not a maintenance error: {error:#}"))
        }
    }
}

/// `result` is a refusal whose message contains `text`.
fn assert_refused(result: anyhow::Result<Value>, text: &str) {
    let kind = failure(&result);
    let message = format!("{:#}", result.unwrap_err());
    assert!(kind.is_refusal(), "{kind:?}: {message}");
    assert!(
        message.contains(text),
        "{message:?} does not contain {text:?}"
    );
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    key: PathBuf,
    public: PathBuf,
    log: PathBuf,
    head: PathBuf,
    anchors: PathBuf,
    out: PathBuf,
    count: usize,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().canonicalize().expect("resolve tempdir");
        for name in ["keys", "ledger", "heads", "anchors", "work"] {
            std::fs::create_dir(root.join(name)).expect("mkdir");
        }
        let key = root.join("keys/ledger.pem");
        let public = root.join("keys/ledger.pub");
        write_key(&key, &public, 11);
        Self {
            log: root.join("ledger/stack.jsonl"),
            head: root.join("heads/head.json"),
            anchors: root.join("anchors"),
            out: root.join("out"),
            key,
            public,
            root,
            count: 0,
            _temp: temp,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Append one record to `log`, signing `head`; return the line written.
    fn append_to(
        &mut self,
        log: &Path,
        head: &Path,
        module: &str,
        action: &str,
        extra: Value,
    ) -> Map<String, Value> {
        self.count += 1;
        let mut record = json!({
            "ts": format!("2026-11-03T10:{:02}:{:02}Z", self.count / 60, self.count % 60),
            "actor": "site-admin",
            "stack": "2026.06",
            "target": "x86_64/generic",
            "module": module,
            "location": format!("/sw/{module}"),
            "action": action,
            "build": {"event_id": format!("test/{}", self.count)},
        });
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            record[key] = value;
        }
        let path = self.path(&format!("work/record-{}.json", self.count));
        std::fs::write(&path, record.to_string()).expect("write record");
        ledger::append(&ledger::AppendRequest {
            log: log.to_path_buf(),
            record: path,
            expect_head: None,
            key: Some(self.key.clone()),
            head_out: Some(head.to_path_buf()),
            tiles: None,
            origin: None,
        })
        .expect("ledger append");
        records(log).pop().expect("a record")
    }

    fn append(&mut self, module: &str, action: &str, extra: Value) -> Map<String, Value> {
        let (log, head) = (self.log.clone(), self.head.clone());
        self.append_to(&log, &head, module, action, extra)
    }

    fn records(&self) -> Vec<Map<String, Value>> {
        records(&self.log)
    }

    fn log_bytes(&self) -> Vec<u8> {
        std::fs::read(&self.log).expect("read log")
    }

    fn sign(&self, key: &Path, out: &Path, ts: Option<&str>) {
        ledger::sign_head(&ledger::HeadRequest {
            log: self.log.clone(),
            key: key.to_path_buf(),
            out: out.to_path_buf(),
            ts: ts.map(str::to_string),
            tiles: None,
            origin: None,
        })
        .expect("ledger head");
    }

    fn verify_record(&self, index: usize, head: &Path) -> bool {
        ledger::verify(&VerifyRequest {
            log: self.log.clone(),
            public_key: self.public.clone(),
            mode: VerifyMode::Inclusion {
                record: index,
                head: head.to_path_buf(),
            },
            tiles: None,
            origin: None,
        })
        .is_ok()
    }

    fn default_observations() -> Value {
        json!([
            {"module": "legacy/1.0", "location": "/sw/legacy/1.0",
             "modulefile": {"path": "/modules/all/legacy/1.0.lua", "sha256": "ab".repeat(32)},
             "provenance": [{"ref": "/archive/easybuild/legacy-1.0.log", "kind": "easybuild-log",
                             "sha256": "cd".repeat(32)}]},
            {"module": "old/2", "location": "/sw/old/2",
             "provenance": [{"ref": "ticket HPC-1234"}], "note": "found during audit"},
            {"module": "old/3", "location": "/sw/old/3", "provenance": [{"ref": "inventory.csv row 9"}]},
        ])
    }

    /// Write a manifest with `overrides` over the default fields; return its path and SHA-256.
    fn write_manifest(&self, name: &str, overrides: Value) -> (PathBuf, String) {
        let mut manifest = json!({
            "schema": lm::MANIFEST_SCHEMA, "manifest_id": "site-audit-1", "observed_at": OBSERVED,
            "observer": "audit/jdoe", "stack": "2026.06", "target": "x86_64/generic",
            "observations": Self::default_observations(),
        });
        for (key, value) in overrides.as_object().cloned().unwrap_or_default() {
            manifest[key] = value;
        }
        let path = self.path(&format!("work/{name}"));
        let data = serde_json::to_string_pretty(&manifest).unwrap() + "\n";
        std::fs::write(&path, &data).expect("write manifest");
        (path, sha256(data.as_bytes()))
    }

    fn backfill_at(&self, manifest: &Path, digest: &str, now: i64) -> anyhow::Result<Value> {
        lm::backfill(&lm::BackfillRequest {
            log: self.log.clone(),
            key: self.key.clone(),
            head: self.head.clone(),
            public_key: self.public.clone(),
            manifest: manifest.to_path_buf(),
            manifest_sha256: digest.to_string(),
            now,
        })
    }

    fn backfill(&self, manifest: &Path, digest: &str) -> anyhow::Result<Value> {
        self.backfill_at(manifest, digest, DAY1)
    }

    fn anchor_with(&self, now: i64, key: &Path, anchors: &Path) -> anyhow::Result<Value> {
        lm::anchor(&lm::AnchorRequest {
            log: self.log.clone(),
            key: key.to_path_buf(),
            public_key: self.public.clone(),
            anchors: anchors.to_path_buf(),
            now,
        })
    }

    fn anchor(&self, now: i64) -> anyhow::Result<Value> {
        self.anchor_with(now, &self.key, &self.anchors)
    }

    fn verify_anchors(&self) -> anyhow::Result<Value> {
        lm::verify_anchors(&self.log, &self.public, &self.anchors)
    }

    fn project_with(&self, out: &Path, head: &Path) -> anyhow::Result<Value> {
        lm::project(&lm::ProjectRequest {
            log: self.log.clone(),
            head: head.to_path_buf(),
            public_key: self.public.clone(),
            out: out.to_path_buf(),
        })
    }

    fn project(&self) -> anyhow::Result<Value> {
        self.project_with(&self.out, &self.head)
    }

    fn anchor_files(&self) -> Vec<String> {
        let mut names = std::fs::read_dir(&self.anchors)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".head.json"))
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn staging(&self) -> Vec<String> {
        std::fs::read_dir(&self.anchors)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".anchor-staging-"))
            .collect()
    }
}

fn records(log: &Path) -> Vec<Map<String, Value>> {
    std::fs::read_to_string(log)
        .expect("read log")
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap()
                .as_object()
                .unwrap()
                .clone()
        })
        .collect()
}

fn sha256(data: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn s(path: &Path) -> &str {
    path.to_str().expect("utf8 path")
}

// Backfill

#[test]
fn observations_become_findings_at_the_observation_time() {
    let fixture = Fixture::new();
    let (manifest, digest) = fixture.write_manifest("manifest.json", json!({}));
    let result = fixture.backfill(&manifest, &digest).expect("backfill");
    assert_eq!(
        (result["appended"].as_u64(), result["replayed"].as_u64()),
        (Some(3), Some(0))
    );
    assert!(result["receipts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|receipt| receipt["verified"] == true));
    let records = fixture.records();
    let modules = records
        .iter()
        .map(|record| record["module"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(modules, ["legacy/1.0", "old/2", "old/3"]);
    for record in &records {
        assert_eq!(record["action"], "finding");
        assert_eq!(record["ts"], OBSERVED);
        assert!(!record.contains_key("stage"));
        assert_eq!(record["build"]["kind"], lm::BACKFILL_KIND);
        assert_eq!(record["build"]["manifest_sha256"], digest.as_str());
        assert_eq!(record["build"]["observed_at"], OBSERVED);
        assert!(record["reason"]
            .as_str()
            .unwrap()
            .contains("no history before the observation"));
    }
    assert_eq!(
        records[0]["build"]["provenance"],
        json!([{"ref": "/archive/easybuild/legacy-1.0.log", "kind": "easybuild-log", "sha256": "cd".repeat(32)}])
    );
    assert_eq!(
        records[0]["modulefile"],
        json!({"path": "/modules/all/legacy/1.0.lua", "sha256": "ab".repeat(32)})
    );
    assert_eq!(records[1]["build"]["note"], "found during audit");
    for index in 0..3 {
        assert!(fixture.verify_record(index, &fixture.head));
    }
}

#[test]
fn repeated_retries_append_nothing() {
    let fixture = Fixture::new();
    let (manifest, digest) = fixture.write_manifest("manifest.json", json!({}));
    let first = fixture.backfill(&manifest, &digest).expect("backfill");
    let before = fixture.log_bytes();
    let ids = |result: &Value| {
        result["receipts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|receipt| receipt["id"].clone())
            .collect::<Vec<_>>()
    };
    for now in [DAY1 + 1, DAY2, DAY3] {
        let again = fixture.backfill_at(&manifest, &digest, now).expect("retry");
        assert_eq!(
            (again["appended"].as_u64(), again["replayed"].as_u64()),
            (Some(0), Some(3))
        );
        assert_eq!(ids(&again), ids(&first));
        assert_eq!(fixture.log_bytes(), before);
    }
}

#[test]
fn an_interrupted_backfill_resumes_without_duplicates() {
    let mut fixture = Fixture::new();
    let (manifest, digest) = fixture.write_manifest("manifest.json", json!({}));
    // The state a backfill leaves when it stops after its first append.
    let (loaded, loaded_digest) = lm::load_manifest(&manifest, &digest, DAY1).expect("manifest");
    let first = lm::backfill_records(&loaded, &loaded_digest).remove(0);
    let path = fixture.path("work/first.json");
    std::fs::write(&path, first.to_string()).unwrap();
    ledger::append(&ledger::AppendRequest {
        log: fixture.log.clone(),
        record: path,
        expect_head: None,
        key: Some(fixture.key.clone()),
        head_out: Some(fixture.head.clone()),
        tiles: None,
        origin: None,
    })
    .expect("first append");
    fixture.count += 1;
    assert_eq!(fixture.records().len(), 1);
    let result = fixture.backfill(&manifest, &digest).expect("resume");
    assert_eq!(
        (result["appended"].as_u64(), result["replayed"].as_u64()),
        (Some(2), Some(1))
    );
    let records = fixture.records();
    assert_eq!(records.len(), 3);
    let events = records
        .iter()
        .map(|record| record["build"]["event_id"].as_str().unwrap().to_string())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(events.len(), 3);
}

#[test]
fn a_torn_tail_is_quarantined_before_the_backfill() {
    let mut fixture = Fixture::new();
    fixture.append("demo/1", "install", json!({}));
    let mut log = std::fs::OpenOptions::new()
        .append(true)
        .open(&fixture.log)
        .unwrap();
    std::io::Write::write_all(&mut log, br#"{"action":"inst"#).unwrap();
    drop(log);
    let (manifest, digest) = fixture.write_manifest("manifest.json", json!({}));
    fixture.backfill(&manifest, &digest).expect("backfill");
    assert_eq!(fixture.records().len(), 4);
    let quarantined = std::fs::read_dir(fixture.path("ledger"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".quarantine.")
        })
        .count();
    assert_eq!(quarantined, 1);
    assert!(fixture.verify_record(3, &fixture.head));
}

#[test]
fn changed_manifest_content_is_refused_before_anything_is_appended() {
    let fixture = Fixture::new();
    let (manifest, digest) = fixture.write_manifest("manifest.json", json!({}));
    fixture.backfill(&manifest, &digest).expect("backfill");
    let before = fixture.log_bytes();
    let (changed, changed_digest) = fixture.write_manifest(
        "changed.json",
        json!({"observations": [
            {"module": "legacy/1.0", "location": "/sw/legacy/1.0", "provenance": [{"ref": "a different source"}]},
            {"module": "new/9", "location": "/sw/new/9", "provenance": [{"ref": "x"}]},
        ]}),
    );
    assert_refused(fixture.backfill(&changed, &changed_digest), "immutable");
    assert_eq!(fixture.log_bytes(), before);
    assert_refused(fixture.backfill(&changed, &digest), "not the pinned");
    assert_eq!(fixture.log_bytes(), before);
}

#[test]
fn an_event_recorded_with_other_content_is_refused_before_anything_is_appended() {
    let mut fixture = Fixture::new();
    let (manifest, digest) = fixture.write_manifest("manifest.json", json!({}));
    let (loaded, loaded_digest) = lm::load_manifest(&manifest, &digest, DAY1).expect("manifest");
    let record = lm::backfill_records(&loaded, &loaded_digest).remove(2);
    fixture.append(
        "old/3",
        "finding",
        json!({"ts": OBSERVED, "build": {"event_id": record["build"]["event_id"]}}),
    );
    let before = fixture.log_bytes();
    assert_refused(fixture.backfill(&manifest, &digest), "other content");
    assert_eq!(fixture.log_bytes(), before);
}

#[test]
fn unpinned_or_guessed_input_is_refused() {
    let fixture = Fixture::new();
    let (manifest, _digest) = fixture.write_manifest("manifest.json", json!({}));
    assert_refused(
        fixture.backfill(&manifest, &"0".repeat(64)),
        "not the pinned",
    );
    let one = |item: Value| json!({"observations": [item]});
    let cases = [
        (
            json!({"observed_at": "2027-01-01T00:00:00Z"}),
            "after the current time",
        ),
        (json!({"observed_at": "2026-11-02"}), "observed_at"),
        (
            json!({"observed_at": "2026-11-02T09:30:00+01:00"}),
            "observed_at",
        ),
        (
            one(
                json!({"module": "a/1", "location": "/sw/a/1", "installed_at": "2019-01-01T00:00:00Z",
                       "provenance": [{"ref": "r"}]}),
            ),
            "unknown fields installed_at",
        ),
        (
            one(
                json!({"module": "a/1", "location": "/sw/a/1", "action": "install", "provenance": [{"ref": "r"}]}),
            ),
            "unknown fields action",
        ),
        (
            one(json!({"module": "a/1", "location": "/sw/a/1", "provenance": []})),
            "original reference",
        ),
        (
            one(json!({"module": "a/1", "location": "sw/a/1", "provenance": [{"ref": "r"}]})),
            "absolute",
        ),
        (
            one(
                json!({"module": "a/1", "location": "/sw/a/1", "provenance": [{"ref": "r", "date": "x"}]}),
            ),
            "unknown fields date",
        ),
        (
            json!({"observations": [
                {"module": "a/1", "location": "/sw/a/1", "provenance": [{"ref": "r"}]},
                {"module": "a/1", "location": "/sw/a/1", "provenance": [{"ref": "r"}]},
            ]}),
            "repeats",
        ),
        (json!({"stage": "finished"}), "unknown fields stage"),
        (json!({"schema": "other"}), "schema"),
    ];
    for (overrides, message) in cases {
        let (path, digest) = fixture.write_manifest("case.json", overrides);
        assert_refused(fixture.backfill(&path, &digest), message);
    }
    assert!(!fixture.log.exists());
}

#[test]
fn path_aliases_and_a_key_beside_the_log_are_refused() {
    let fixture = Fixture::new();
    let (manifest, digest) = fixture.write_manifest("manifest.json", json!({}));
    let request = |log: PathBuf, key: PathBuf, head: PathBuf| lm::BackfillRequest {
        log,
        key,
        head,
        public_key: fixture.public.clone(),
        manifest: manifest.clone(),
        manifest_sha256: digest.clone(),
        now: DAY1,
    };
    std::os::unix::fs::symlink(fixture.path("ledger"), fixture.path("alias")).unwrap();
    assert_refused(
        lm::backfill(&request(
            fixture.path("alias/stack.jsonl"),
            fixture.key.clone(),
            fixture.head.clone(),
        )),
        "aliases are refused",
    );
    assert_refused(
        lm::backfill(&request(
            PathBuf::from(format!("{}/ledger/../ledger/stack.jsonl", s(&fixture.root))),
            fixture.key.clone(),
            fixture.head.clone(),
        )),
        "normalised",
    );
    let beside = fixture.path("ledger/key.pem");
    std::fs::copy(&fixture.key, &beside).unwrap();
    assert_refused(
        lm::backfill(&request(fixture.log.clone(), beside, fixture.head.clone())),
        "inside the ledger directory",
    );
    assert_refused(
        lm::backfill(&request(
            fixture.log.clone(),
            fixture.key.clone(),
            fixture.key.clone(),
        )),
        "same file",
    );
    assert!(!fixture.log.exists());
}

// Anchors

#[test]
fn the_first_anchor_is_read_only_single_link_and_verifies() {
    let mut fixture = Fixture::new();
    fixture.append("demo/1", "install", json!({}));
    let result = fixture.anchor(DAY1).expect("anchor");
    assert_eq!(result["status"], "published");
    assert_eq!(fixture.anchor_files(), ["2026-11-03.head.json"]);
    let path = PathBuf::from(result["path"].as_str().unwrap());
    let info = std::fs::symlink_metadata(&path).unwrap();
    assert_eq!((info.mode() & 0o7777, info.nlink()), (0o444, 1));
    let head = lm::parse_head(&std::fs::read(&path).unwrap(), "anchor").expect("head");
    assert_eq!((head.size, head.ts.as_str()), (1, "2026-11-03T12:00:00Z"));
    assert!(fixture.verify_record(0, &path));
    assert_eq!(fixture.verify_anchors().expect("verify")["anchors"], 1);
    assert!(fixture.staging().is_empty());
}

#[test]
fn an_empty_log_can_be_anchored() {
    let mut fixture = Fixture::new();
    std::fs::write(&fixture.log, "").unwrap();
    assert_eq!(fixture.anchor(DAY1).expect("empty anchor")["size"], 0);
    fixture.append("demo/1", "install", json!({}));
    assert_eq!(fixture.anchor(DAY2).expect("second anchor")["size"], 1);
    assert_eq!(fixture.verify_anchors().expect("verify")["anchors"], 2);
}

#[test]
fn a_same_day_retry_never_replaces_the_anchor() {
    let mut fixture = Fixture::new();
    fixture.append("demo/1", "install", json!({}));
    let first = fixture.anchor(DAY1).expect("anchor");
    let path = PathBuf::from(first["path"].as_str().unwrap());
    let inode = std::fs::symlink_metadata(&path).unwrap().ino();
    let before = std::fs::read(&path).unwrap();
    fixture.append("demo/2", "install", json!({}));
    for _ in 0..2 {
        let again = fixture.anchor(DAY1 + 3600).expect("retry");
        assert_eq!(
            (&again["status"], &again["size"], &again["root"]),
            (&json!("exists"), &json!(1), &first["root"])
        );
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::symlink_metadata(&path).unwrap().ino(), inode);
    assert_eq!(fixture.anchor_files(), ["2026-11-03.head.json"]);
}

#[test]
fn daily_anchors_form_a_verified_chain() {
    let mut fixture = Fixture::new();
    fixture.append("demo/1", "install", json!({}));
    fixture.anchor(DAY1).expect("first");
    fixture.append("demo/2", "install", json!({}));
    fixture.append("demo/3", "install", json!({}));
    let second = fixture.anchor(DAY2).expect("second");
    assert_eq!(
        (&second["status"], &second["size"]),
        (&json!("published"), &json!(3))
    );
    let third = fixture.anchor(DAY3).expect("third");
    assert_eq!(
        (&third["status"], &third["size"]),
        (&json!("published"), &json!(3))
    );
    ledger::verify(&VerifyRequest {
        log: fixture.log.clone(),
        public_key: fixture.public.clone(),
        mode: VerifyMode::Consistency {
            from: fixture.anchors.join("2026-11-03.head.json"),
            to: PathBuf::from(second["path"].as_str().unwrap()),
        },
        tiles: None,
        origin: None,
    })
    .expect("consistency between anchors");
    let result = fixture.verify_anchors().expect("verify");
    assert_eq!(
        (&result["anchors"], &result["size"]),
        (&json!(3), &json!(3))
    );
}

#[test]
fn a_rewritten_log_is_refused_and_no_anchor_is_added() {
    let mut fixture = Fixture::new();
    fixture.append("demo/1", "install", json!({}));
    fixture.append("demo/2", "install", json!({}));
    fixture.anchor(DAY1).expect("anchor");
    let other = fixture.path("work/rewritten.jsonl");
    let other_head = fixture.path("work/other-head.json");
    fixture.append_to(
        &other,
        &other_head,
        "demo/1",
        "install",
        json!({"location": "/sw/evil"}),
    );
    for number in 0..3 {
        fixture.append_to(
            &other,
            &other_head,
            &format!("demo/{}", number + 2),
            "install",
            json!({}),
        );
    }
    std::fs::write(&fixture.log, std::fs::read(&other).unwrap()).unwrap();
    assert_eq!(failure(&fixture.anchor(DAY2)), Failure::Equivocation);
    assert_eq!(fixture.anchor_files(), ["2026-11-03.head.json"]);
    assert_eq!(failure(&fixture.anchor(DAY1 + 60)), Failure::Equivocation);
    assert!(failure(&fixture.verify_anchors()).is_refusal());
}

#[test]
fn a_truncated_log_is_refused() {
    let mut fixture = Fixture::new();
    for number in 0..3 {
        fixture.append(&format!("demo/{number}"), "install", json!({}));
    }
    fixture.anchor(DAY1).expect("anchor");
    let data = fixture.log_bytes();
    let first = data.iter().position(|byte| *byte == b'\n').unwrap();
    std::fs::write(&fixture.log, &data[..=first]).unwrap();
    assert_eq!(failure(&fixture.anchor(DAY2)), Failure::Equivocation);
    assert_eq!(fixture.anchor_files(), ["2026-11-03.head.json"]);
}

#[test]
fn a_tampered_or_malformed_anchor_is_refused_and_kept() {
    let mut fixture = Fixture::new();
    fixture.append("demo/1", "install", json!({}));
    let published = PathBuf::from(
        fixture.anchor(DAY1).expect("anchor")["path"]
            .as_str()
            .unwrap(),
    );
    fixture.append("demo/2", "install", json!({}));
    let good = std::fs::read(&published).unwrap();
    let head: SignedHead = serde_json::from_slice(&good).unwrap();
    let flipped = if head.signature.starts_with('0') {
        "1"
    } else {
        "0"
    };
    let tampered = [
        SignedHead {
            signature: format!("{flipped}{}", &head.signature[1..]),
            ..head.clone()
        },
        SignedHead {
            root: "0".repeat(64),
            ..head.clone()
        },
    ];
    for case in tampered {
        std::fs::set_permissions(&published, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(&published, serde_json::to_string(&case).unwrap() + "\n").unwrap();
        assert!(failure(&fixture.anchor(DAY2)).is_refusal());
        assert_eq!(fixture.anchor_files(), ["2026-11-03.head.json"]);
    }
    let mut extra = good[..good.len() - 2].to_vec();
    extra.extend_from_slice(b",\"x\":1}\n");
    for data in [good[..40].to_vec(), extra] {
        std::fs::write(&published, &data).unwrap();
        assert!(failure(&fixture.anchor(DAY2)).is_refusal());
        assert_eq!(std::fs::read(&published).unwrap(), data);
        assert_eq!(fixture.anchor_files(), ["2026-11-03.head.json"]);
    }
}

#[test]
fn an_anchor_from_a_later_day_or_another_key_is_refused() {
    let mut fixture = Fixture::new();
    fixture.append("demo/1", "install", json!({}));
    fixture.anchor(DAY2).expect("anchor");
    assert_refused(fixture.anchor(DAY1), "after 2026-11-03");
    let (other_key, other_public) = (
        fixture.path("keys/other.pem"),
        fixture.path("keys/other.pub"),
    );
    write_key(&other_key, &other_public, 23);
    assert!(failure(&fixture.anchor_with(DAY3, &other_key, &fixture.anchors)).is_refusal());
    assert_eq!(fixture.anchor_files(), ["2026-11-04.head.json"]);
}

#[test]
fn a_run_killed_before_publication_leaves_no_partial_anchor() {
    let mut fixture = Fixture::new();
    fixture.append("demo/1", "install", json!({}));
    // A run killed after it signed: its staging directory holds a signed
    // head and a copy of the log that were never linked into place.
    let leftover = fixture.anchors.join(".anchor-staging-killed");
    std::fs::create_dir(&leftover).unwrap();
    fixture.sign(
        &fixture.key,
        &leftover.join("head.json"),
        Some("2026-11-03T12:00:00Z"),
    );
    std::fs::write(leftover.join("ledger.jsonl"), b"{\"partial").unwrap();
    assert!(fixture.anchor_files().is_empty());
    assert_eq!(fixture.verify_anchors().expect("verify")["anchors"], 0);
    assert_eq!(fixture.anchor(DAY1).expect("anchor")["status"], "published");
    assert!(fixture.staging().is_empty());
    assert_eq!(fixture.anchor_files(), ["2026-11-03.head.json"]);
    assert_eq!(fixture.anchor(DAY1).expect("rerun")["status"], "exists");
    assert_eq!(fixture.verify_anchors().expect("verify")["anchors"], 1);
}

#[test]
fn a_crash_between_link_and_cleanup_is_recovered() {
    let mut fixture = Fixture::new();
    fixture.append("demo/1", "install", json!({}));
    let published = PathBuf::from(
        fixture.anchor(DAY1).expect("anchor")["path"]
            .as_str()
            .unwrap(),
    );
    let leftover = fixture.anchors.join(".anchor-staging-crashed");
    std::fs::create_dir(&leftover).unwrap();
    std::fs::hard_link(&published, leftover.join("head.json")).unwrap();
    std::fs::write(leftover.join("ledger.jsonl"), b"{\"partial").unwrap();
    assert_refused(fixture.verify_anchors(), "one link");
    assert_eq!(fixture.anchor(DAY1).expect("rerun")["status"], "exists");
    assert!(!leftover.exists());
    assert_eq!(std::fs::symlink_metadata(&published).unwrap().nlink(), 1);
}

#[test]
fn an_anchor_with_another_name_is_refused() {
    let mut fixture = Fixture::new();
    fixture.append("demo/1", "install", json!({}));
    let published = PathBuf::from(
        fixture.anchor(DAY1).expect("anchor")["path"]
            .as_str()
            .unwrap(),
    );
    let copy = fixture.path("work/copy.json");
    std::fs::hard_link(&published, &copy).unwrap();
    assert_refused(fixture.anchor(DAY2), "one link");
    std::fs::remove_file(&copy).unwrap();
    std::fs::rename(&published, fixture.anchors.join("2026-11-02.head.json")).unwrap();
    assert_refused(fixture.anchor(DAY2), "not on 2026-11-02");
}

#[test]
fn anchor_paths_are_checked() {
    let mut fixture = Fixture::new();
    fixture.append("demo/1", "install", json!({}));
    assert_refused(
        fixture.anchor_with(DAY1, &fixture.key, &fixture.path("ledger")),
        "must not hold the ledger",
    );
    let inside = fixture.anchors.join("key.pem");
    std::fs::copy(&fixture.key, &inside).unwrap();
    assert_refused(
        fixture.anchor_with(DAY1, &inside, &fixture.anchors),
        "inside the anchor directory",
    );
    std::fs::remove_file(&inside).unwrap();
    std::os::unix::fs::symlink(&fixture.anchors, fixture.path("anchors-alias")).unwrap();
    assert_refused(
        fixture.anchor_with(DAY1, &fixture.key, &fixture.path("anchors-alias")),
        "aliases are refused",
    );
    std::fs::hard_link(&fixture.log, fixture.path("work/log-alias")).unwrap();
    assert_refused(fixture.anchor(DAY1), "hard links");
    assert!(fixture.anchor_files().is_empty());
}

// Package history

/// A log with every kind of event the projection interprets; returns the
/// finished rollback record.
fn history(fixture: &mut Fixture) -> Map<String, Value> {
    fixture.append("demo/1", "install", json!({"stage": "started"}));
    fixture.append("demo/1", "install", json!({"stage": "finished"}));
    fixture.append(
        "demo/1",
        "rebuild-inplace",
        json!({"stage": "started", "location": "/sw/demo/1-gen2"}),
    );
    let rebuilt = fixture.append(
        "demo/1",
        "rebuild-inplace",
        json!({"stage": "finished", "location": "/sw/demo/1-gen2"}),
    );
    fixture.append(
        "demo/1",
        "correction",
        json!({"stage": "started", "corrects": rebuilt["id"], "reason": "rolled back"}),
    );
    let rollback = fixture.append(
        "demo/1",
        "correction",
        json!({"stage": "finished", "corrects": rebuilt["id"], "reason": "rolled back"}),
    );
    fixture.append(
        "demo/1",
        "access",
        json!({"reason": {"group": "hpc", "change": "granted"}}),
    );
    fixture.append("tool/2", "install", json!({}));
    let retired = fixture.append("tool/2", "retire", json!({"reason": "published on CVMFS"}));
    fixture.append(
        "tool/2",
        "correction",
        json!({"stage": "aborted", "corrects": retired["id"]}),
    );
    fixture.append(
        "zlib/1.3",
        "finding",
        json!({"reason": "CVE-2026-0001 reported"}),
    );
    let (manifest, digest) = fixture.write_manifest("manifest.json", json!({}));
    fixture.backfill(&manifest, &digest).expect("backfill");
    rollback
}

fn package(out: &Path, name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(out.join(format!("packages/{name}.json"))).unwrap())
        .unwrap()
}

fn events(document: &Value) -> BTreeMap<u64, Value> {
    document["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| (event["index"].as_u64().unwrap(), event.clone()))
        .collect()
}

#[test]
fn history_keeps_correction_retire_access_and_finding_semantics() {
    let mut fixture = Fixture::new();
    let rollback = history(&mut fixture);
    let result = fixture.project().expect("project");
    assert_eq!(
        (&result["size"], &result["packages"]),
        (&json!(14), &json!(5))
    );
    let demo = package(&fixture.out, "demo");
    let by_index = events(&demo);
    assert_eq!(by_index[&3]["status"], "corrected");
    assert_eq!(by_index[&3]["corrected_by"], json!([5]));
    assert_eq!(by_index[&3]["correction_pending_by"], json!([4]));
    assert_eq!(
        by_index[&5]["links"]["corrects"],
        json!({"id": by_index[&3]["id"], "index": 3})
    );
    assert_eq!(
        demo["modules"],
        json!([{"module": "demo/1", "state": "installed", "location": "/sw/demo/1",
                "last_change": 1, "access": [6], "findings": []}])
    );
    assert_eq!(
        by_index[&6]["reason"],
        json!({"change": "granted", "group": "hpc"})
    );
    let tool = package(&fixture.out, "tool");
    assert_eq!(tool["modules"][0]["state"], "retired");
    assert_eq!(tool["events"][1]["status"], "effective");
    assert_eq!(tool["events"][1]["corrected_by"], json!([]));
    let zlib = package(&fixture.out, "zlib");
    assert_eq!(
        (
            &zlib["modules"][0]["state"],
            &zlib["modules"][0]["findings"]
        ),
        (&json!("observed"), &json!([10]))
    );
    let legacy = package(&fixture.out, "legacy");
    assert_eq!(legacy["modules"][0]["state"], "observed");
    assert_eq!(legacy["events"][0]["observed"], true);
    assert_eq!(
        legacy["events"][0]["provenance"][0]["ref"],
        "/archive/easybuild/legacy-1.0.log"
    );
    assert_eq!(legacy["events"][0]["ts"], OBSERVED);
    let markdown = std::fs::read_to_string(fixture.out.join("packages/demo.md")).unwrap();
    assert!(markdown.contains("corrected by \\#5"), "{markdown}");
    assert!(
        markdown.contains("correction started by \\#4"),
        "{markdown}"
    );
    let index: Value =
        serde_json::from_slice(&std::fs::read(fixture.out.join("index.json")).unwrap()).unwrap();
    let names = index["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, ["demo", "legacy", "old", "tool", "zlib"]);
    assert_eq!(index["head"]["size"], 14);

    fixture.append(
        "demo/1",
        "correction",
        json!({"corrects": rollback["id"], "reason": "the rollback was itself wrong"}),
    );
    fixture.project().expect("project again");
    let demo = package(&fixture.out, "demo");
    let by_index = events(&demo);
    assert_eq!(
        (&by_index[&5]["status"], &by_index[&3]["status"]),
        (&json!("corrected"), &json!("effective"))
    );
    assert_eq!(demo["modules"][0]["state"], "installed");
    assert_eq!(
        (
            &demo["modules"][0]["last_change"],
            &demo["modules"][0]["location"]
        ),
        (&json!(3), &json!("/sw/demo/1-gen2"))
    );
}

#[test]
fn projection_is_deterministic() {
    let mut fixture = Fixture::new();
    history(&mut fixture);
    let (one, two) = (fixture.path("one"), fixture.path("two"));
    fixture.project_with(&one, &fixture.head).expect("one");
    fixture.project_with(&two, &fixture.head).expect("two");
    fixture
        .project_with(&one, &fixture.head)
        .expect("one again");
    let (first, second) = (tree(&one), tree(&two));
    assert_eq!(first, second);
    assert_eq!(
        first.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "index.json",
            "index.md",
            "packages/demo.json",
            "packages/demo.md",
            "packages/legacy.json",
            "packages/legacy.md",
            "packages/old.json",
            "packages/old.md",
            "packages/tool.json",
            "packages/tool.md",
            "packages/zlib.json",
            "packages/zlib.md",
        ]
    );
}

#[test]
fn untrusted_text_is_escaped() {
    let mut fixture = Fixture::new();
    let hostile = "evil|pkg<script>alert(1)<\\script>`x`*[link](javascript:e)\\|";
    fixture.append(
        &format!("{hostile}/1"),
        "install",
        json!({"actor": "a\n# injected heading\n| x | y |", "reason": "\u{202e}txt.exe & <b>"}),
    );
    fixture.append("../../../etc/1", "install", json!({}));
    fixture.append("r\u{e9}sum\u{e9}/1", "access", json!({"reason": "ok"}));
    fixture.sign(&fixture.key, &fixture.head, Some("2026-11-03T12:00:00Z"));
    fixture.project().expect("project");
    let files = tree(&fixture.out);
    let out = format!("{}/", s(&fixture.out));
    for name in files.keys() {
        let real = fixture.out.join(name).canonicalize().unwrap();
        assert!(
            s(&real).starts_with(&out),
            "{name} resolves to {}",
            real.display()
        );
    }
    let markdown = files
        .iter()
        .filter(|(name, _)| name.ends_with(".md"))
        .map(|(_, data)| String::from_utf8(data.clone()).unwrap())
        .collect::<String>();
    assert!(!markdown.contains("<script>"));
    assert!(!markdown.contains("<b>"));
    assert!(!markdown.contains('\u{202e}'));
    assert!(!markdown.contains("\n# injected"));
    assert!(markdown.contains("U+202E"));
    for (name, data) in &files {
        if !name.ends_with(".md") {
            serde_json::from_slice::<Value>(data).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(data.is_ascii(), "{name} is not ASCII");
            continue;
        }
        let mut header = None;
        for line in std::str::from_utf8(data).unwrap().lines() {
            if !line.starts_with('|') {
                header = None;
                continue;
            }
            let expected = *header.get_or_insert(cell_separators(line));
            assert_eq!(cell_separators(line), expected, "{name}: {line}");
        }
    }
    let evil = files
        .keys()
        .filter(|name| name.starts_with("packages/evil"))
        .collect::<Vec<_>>();
    assert_eq!(evil.len(), 2);
    let json_name = evil.iter().find(|name| name.ends_with(".json")).unwrap();
    let document: Value = serde_json::from_slice(&files[*json_name]).unwrap();
    assert_eq!(document["package"], hostile);
    assert_eq!(
        document["events"][0]["actor"],
        "a\n# injected heading\n| x | y |"
    );
    let index = String::from_utf8(files["index.md"].clone()).unwrap();
    assert!(index.contains("(packages/evil%257Cpkg"), "{index}");
}

#[test]
fn a_tampered_log_is_refused_and_nothing_is_written() {
    let mut fixture = Fixture::new();
    for number in 0..3 {
        fixture.append(&format!("demo/{number}"), "install", json!({}));
    }
    let good = fixture.log_bytes();
    let text = String::from_utf8(good.clone()).unwrap();
    std::fs::write(&fixture.log, text.replacen("/sw/demo/1", "/sw/demo/X", 1)).unwrap();
    assert_eq!(failure(&fixture.project()), Failure::Verification);
    let other = fixture.path("work/rewritten.jsonl");
    let other_head = fixture.path("work/other-head.json");
    for number in 0..3 {
        fixture.append_to(
            &other,
            &other_head,
            &format!("demo/{number}"),
            "install",
            json!({"location": format!("/sw/rewritten/{number}")}),
        );
    }
    std::fs::write(&fixture.log, std::fs::read(&other).unwrap()).unwrap();
    let rewritten = fixture.project();
    assert_eq!(failure(&rewritten), Failure::Verification);
    assert_refused(rewritten, "is not in the head");
    let mut partial = good.clone();
    partial.extend_from_slice(b"{\"partial");
    std::fs::write(&fixture.log, partial).unwrap();
    assert_refused(fixture.project(), "partial write");
    assert!(!fixture.out.exists());
}

#[test]
fn a_tampered_or_foreign_head_is_refused() {
    let mut fixture = Fixture::new();
    for number in 0..2 {
        fixture.append(&format!("demo/{number}"), "install", json!({}));
    }
    let head: SignedHead = serde_json::from_slice(&std::fs::read(&fixture.head).unwrap()).unwrap();
    let (other_key, other_public) = (
        fixture.path("keys/other.pem"),
        fixture.path("keys/other.pub"),
    );
    write_key(&other_key, &other_public, 23);
    let foreign = fixture.path("heads/foreign.json");
    fixture.sign(&other_key, &foreign, None);
    let compact = |head: &SignedHead| serde_json::to_string(head).unwrap() + "\n";
    let cases = [
        (
            "root",
            compact(&SignedHead {
                root: "1".repeat(64),
                ..head.clone()
            }),
        ),
        (
            "size",
            compact(&SignedHead {
                size: 1,
                ..head.clone()
            }),
        ),
        (
            "ts",
            compact(&SignedHead {
                ts: "2030-01-01T00:00:00Z".to_string(),
                ..head.clone()
            }),
        ),
        (
            "domain",
            compact(&SignedHead {
                domain: "other-log".to_string(),
                ..head.clone()
            }),
        ),
        (
            "spaced",
            lm::dumps(&serde_json::to_value(&head).unwrap(), None) + "\n",
        ),
        ("not-json", "{".to_string()),
    ];
    for (name, text) in cases {
        let path = fixture.path(&format!("heads/{name}.json"));
        std::fs::write(&path, text).unwrap();
        assert!(
            failure(&fixture.project_with(&fixture.out, &path)).is_refusal(),
            "{name}"
        );
    }
    let refused = fixture.project_with(&fixture.out, &foreign);
    assert_eq!(failure(&refused), Failure::Verification);
    assert_refused(refused, "not by the trusted key");
    assert!(!fixture.out.exists());
    fixture.project().expect("project with the right head");
}

#[test]
fn a_stale_head_is_refused() {
    let mut fixture = Fixture::new();
    fixture.append("demo/1", "install", json!({}));
    let stale = fixture.path("heads/stale.json");
    std::fs::copy(&fixture.head, &stale).unwrap();
    fixture.append("demo/2", "install", json!({}));
    assert_eq!(
        failure(&fixture.project_with(&fixture.out, &stale)),
        Failure::StaleHead
    );
    assert!(!fixture.out.exists());
    assert_eq!(fixture.project().expect("project")["size"], 2);
}

#[test]
fn an_interrupted_projection_is_completed_by_a_rerun() {
    let mut fixture = Fixture::new();
    history(&mut fixture);
    let clean = fixture.path("clean");
    fixture.project_with(&clean, &fixture.head).expect("clean");
    // A run killed while it replaced the package files: some are in place,
    // one temporary file is half written, and no index was written.
    std::fs::create_dir_all(fixture.out.join("packages")).unwrap();
    for name in ["demo.json", "demo.md", "legacy.json"] {
        std::fs::copy(
            clean.join("packages").join(name),
            fixture.out.join("packages").join(name),
        )
        .unwrap();
    }
    std::fs::write(fixture.out.join("packages/.old.md.new"), b"# Package hist").unwrap();
    assert!(!fixture.out.join("index.json").exists());
    fixture.project().expect("rerun");
    assert_eq!(tree(&fixture.out), tree(&clean));
    fixture.append("demo/1", "access", json!({"reason": "later"}));
    // A run killed while it wrote the index: the previous index stays.
    std::fs::write(fixture.out.join(".index.json.new"), b"{\"schema\"").unwrap();
    let index = |out: &Path| -> Value {
        serde_json::from_slice(&std::fs::read(out.join("index.json")).unwrap()).unwrap()
    };
    assert_eq!(index(&fixture.out)["head"]["size"], 14);
    fixture.project().expect("rerun");
    assert_eq!(index(&fixture.out)["head"]["size"], 15);
    assert!(!fixture.out.join(".index.json.new").exists());
}

#[test]
fn output_directory_is_checked() {
    let mut fixture = Fixture::new();
    fixture.append("demo/1", "install", json!({}));
    std::fs::create_dir(&fixture.out).unwrap();
    std::fs::write(fixture.out.join("notes.txt"), "mine").unwrap();
    assert_refused(fixture.project(), "not a ledger history projection");
    assert_refused(
        fixture.project_with(&fixture.path("ledger"), &fixture.head),
        "must not hold the ledger",
    );
    std::os::unix::fs::symlink(fixture.path("work"), fixture.path("out-alias")).unwrap();
    assert_refused(
        fixture.project_with(&fixture.path("out-alias"), &fixture.head),
        "aliases are refused",
    );
}

// Command line

fn eb(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(args)
        .output()
        .expect("run eb-stack")
}

fn stdout_json(output: &Output) -> Value {
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("one JSON object on stdout")
}

#[test]
fn backfill_anchor_and_project_from_the_command_line() {
    let mut fixture = Fixture::new();
    let (manifest, digest) = fixture.write_manifest(
        "manifest.json",
        json!({"observed_at": "2026-01-02T03:04:05Z"}),
    );
    let text = |path: &Path| s(path).to_string();
    let (log, public, key, head) = (
        text(&fixture.log),
        text(&fixture.public),
        text(&fixture.key),
        text(&fixture.head),
    );
    let (anchors, out, manifest) = (text(&fixture.anchors), text(&fixture.out), text(&manifest));
    let common = ["--log", log.as_str(), "--public-key", public.as_str()];
    let backfill = |head: &str| {
        let mut args = vec!["ledger", "backfill"];
        args.extend(common);
        args.extend(["--key", &key, "--head", head, "--manifest", &manifest]);
        args.extend(["--manifest-sha256", &digest]);
        eb(&args)
    };
    let output = backfill(&head);
    assert_eq!(stdout_json(&output)["appended"], 3);
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("{\"appended\": 3, "));

    let mut args = vec!["ledger", "anchor"];
    args.extend(common);
    args.extend(["--key", &key, "--anchors", &anchors]);
    assert_eq!(stdout_json(&eb(&args))["status"], "published");

    let mut args = vec!["ledger", "verify-anchors"];
    args.extend(common);
    args.extend(["--anchors", &anchors]);
    assert_eq!(stdout_json(&eb(&args))["anchors"], 1);

    let project = |head: &str| {
        let mut args = vec!["ledger", "project"];
        args.extend(common);
        args.extend(["--head", head, "--out", &out]);
        eb(&args)
    };
    assert_eq!(stdout_json(&project(&head))["packages"], 2);

    let newer = fixture.path("work/newer.json");
    let log_path = fixture.log.clone();
    fixture.append_to(&log_path, &newer, "demo/1", "install", json!({}));
    let stale = project(&head);
    assert_eq!(stale.status.code(), Some(lm::REFUSED_EXIT));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("sign a current head"));

    // An append that does not go through is status 75 and leaves the log
    // as it was; the same command with a usable head then completes.
    let in_tiles = format!("{log}.tiles/head.json");
    let before = fixture.log_bytes();
    let pending = backfill(&in_tiles);
    assert_eq!(
        pending.status.code(),
        Some(lm::PENDING_EXIT),
        "stderr:\n{}",
        String::from_utf8_lossy(&pending.stderr)
    );
    assert!(String::from_utf8_lossy(&pending.stderr).contains("retry the same command"));
    assert_eq!(fixture.log_bytes(), before);
    assert_eq!(stdout_json(&backfill(&head))["replayed"], 3);
}

// Inputs

#[test]
fn markdown_escaping() {
    assert_eq!(lm::md(&json!("a|b")), "a\\|b");
    assert_eq!(lm::md(&json!("<i>&")), "&lt;i&gt;&amp;");
    assert_eq!(
        lm::md(&json!("x\ny\r\u{2028}\u{202e}\u{1b}")),
        "xU+000AyU+000DU+2028U+202EU+001B"
    );
    assert_eq!(
        lm::md(&json!("`*_[]()~!#\\")),
        "\\`\\*\\_\\[\\]\\(\\)\\~\\!\\#\\\\"
    );
    assert_eq!(
        lm::md(&json!({"b": 1, "a": "<"})),
        "{\"a\":\"&lt;\",\"b\":1}"
    );
    assert_eq!(lm::md(&Value::Null), "");
}

#[test]
fn file_stems_stay_in_one_directory() {
    for name in ["..", ".", "", "a/b", &"x".repeat(400), "a\u{0}b"] {
        let stem = lm::file_stem(name);
        assert!(!stem.contains('/'), "{stem}");
        assert!(!["", ".", ".."].contains(&stem.as_str()), "{stem}");
        assert!(stem.len() <= 150, "{stem}");
    }
    assert_ne!(
        lm::file_stem(&"x".repeat(400)),
        lm::file_stem(&"x".repeat(401))
    );
    assert_eq!(lm::file_stem("PETSc"), "PETSc");
}

#[test]
fn heads_must_be_exactly_what_eb_stack_writes() {
    let head = SignedHead {
        domain: ledger::DOMAIN.to_string(),
        size: 1,
        root: "a".repeat(64),
        ts: "2026-11-03T12:00:00Z".to_string(),
        public_key: "b".repeat(64),
        signature: "c".repeat(128),
    };
    let data = serde_json::to_string(&head).unwrap() + "\n";
    assert_eq!(
        lm::parse_head(data.as_bytes(), "head").expect("parse"),
        head
    );
    let compact = |head: &SignedHead| serde_json::to_string(head).unwrap() + "\n";
    let size_first = format!(
        "{{\"size\":1,\"domain\":\"{}\",\"root\":\"{}\",\"ts\":\"{}\",\"public_key\":\"{}\",\"signature\":\"{}\"}}\n",
        head.domain, head.root, head.ts, head.public_key, head.signature
    );
    let bad = [
        data[..data.len() - 1].to_string(),
        lm::dumps(&serde_json::to_value(&head).unwrap(), None) + "\n",
        data.replace("\"size\":1", "\"size\":true"),
        data.replace("\"size\":1", "\"size\":-1"),
        compact(&SignedHead {
            root: "A".repeat(64),
            ..head.clone()
        }),
        compact(&SignedHead {
            ts: "2026-11-03T12:00:00+00:00".to_string(),
            ..head.clone()
        }),
        size_first,
        data.replace("\"size\":1", "\"size\":1.0"),
        data.replace("\"size\":1", "\"size\":1,\"size\":1"),
    ];
    for case in bad {
        let result = lm::parse_head(case.as_bytes(), "head");
        assert!(result.is_err(), "accepted {case:?}");
        assert!(lm::failure_of(&result.unwrap_err()).is_some_and(Failure::is_refusal));
    }
}

#[test]
fn log_lines_must_be_canonical_ledger_records() {
    let record = json!({"action": "install", "actor": "a", "location": "/x", "module": "m/1",
                        "stack": "s", "target": "t", "ts": "2026-11-03T12:00:00Z"});
    let entry = ledger::seal_record(&record, 0, "").expect("seal");
    let line = format!("{}\n", entry.line);
    let parsed = lm::parse_log(line.as_bytes(), "ledger").expect("parse");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].record, entry.record);
    let other_stack = line
        .replace("\"stack\":\"s\"", "\"stack\":\"other\"")
        .replace("\"index\":0", "\"index\":1");
    let bad: Vec<Vec<u8>> = vec![
        line.trim_end().as_bytes().to_vec(),
        (lm::dumps(&Value::Object(entry.record.clone()), None) + "\n").into_bytes(),
        line.replace("\"index\":0", "\"index\":1").into_bytes(),
        line.replace("\"action\":\"install\"", "\"action\":\"reinstall\"")
            .into_bytes(),
        line.replace("\"actor\":\"a\",", "\"actor\":\"a\",\"licence\":\"x\",")
            .into_bytes(),
        format!("{line}{other_stack}").into_bytes(),
        line.replace("\"index\":0", "\"index\":0.0").into_bytes(),
        b"\xff\n".to_vec(),
    ];
    for case in bad {
        let result = lm::parse_log(&case, "ledger");
        assert!(
            result.is_err(),
            "accepted {:?}",
            String::from_utf8_lossy(&case)
        );
        assert!(lm::failure_of(&result.unwrap_err()).is_some_and(Failure::is_refusal));
    }
}
