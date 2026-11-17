//! Maintenance of a ledger: observed-history backfill, daily signed
//! anchors, and per-package history projections.
//!
//! * [`backfill`] appends `finding` records for software observed at an
//!   explicit time, from an operator-written manifest pinned by its SHA-256.
//! * [`anchor`] signs the day's head and adds it to a directory of immutable
//!   daily anchors once [`ledger::verify`] proves it extends the latest
//!   anchor; [`verify_anchors`] proves the whole chain against the log.
//! * [`project`] writes per-package JSON and Markdown history from a log
//!   whose signed head covers every record.
//!
//! Every signature comes from [`ledger::append`] or [`ledger::sign_head`]
//! and every proof from [`ledger::verify`]. The log and head files are read
//! once; the bytes read are copied into a private directory, verified there
//! with a tile log built from the copy, and parsed from the same buffer, so
//! what is anchored or projected is what was verified.
//!
//! The manifest schema, the projection schema and the backfill event ids
//! carry `eb-staged-rebuild/` names. Existing manifests, projections and
//! logs use those names, and a backfill rerun over such a log is
//! acknowledged as already recorded only while the event ids stay the same.
//!
//! A refusal is exit status [`REFUSED_EXIT`]. A ledger write that did not
//! finish is [`PENDING_EXIT`]: the record may or may not be in the log, and
//! the same command can be run again, because every backfill record is
//! built from the manifest alone and carries a `build.event_id`.

use crate::ledger::{self, Entry, SignedHead, VerifyMode, VerifyRequest};
use anyhow::Result;
use chrono::{DateTime, NaiveDateTime};
use regex::Regex;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::{self, Display, Write as _};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// Schema of an observed-history manifest.
pub const MANIFEST_SCHEMA: &str = "eb-staged-rebuild/observed-backfill/1";
/// Schema of the projection's JSON files.
pub const HISTORY_SCHEMA: &str = "eb-staged-rebuild/ledger-history/1";
/// `build.kind` of a backfilled record.
pub const BACKFILL_KIND: &str = "observed-backfill";
/// `build.tool` of a backfilled record.
pub const TOOL: &str = "eb-staged-rebuild/ledger-maintenance";
/// Exit status of a refusal.
pub const REFUSED_EXIT: i32 = 2;
/// Exit status when a ledger write did not finish; run the same command again.
pub const PENDING_EXIT: i32 = 75;

/// Seconds `anchor` waits for another anchor run to release the directory.
const LOCK_TIMEOUT: Duration = Duration::from_secs(60);
const LOCK_RETRY: Duration = Duration::from_millis(20);
const STAGING_PREFIX: &str = ".anchor-staging-";

/// Fields that name another record by its `id`, in ledger order.
const LINKS: &[&str] = &[
    "supersedes",
    "superseded_by",
    "moved_from",
    "moved_to",
    "corrects",
];
const HEAD_FIELDS: &[&str] = &["domain", "size", "root", "ts", "public_key", "signature"];

static HEX64: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{64}$").unwrap());
static HEX128: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{128}$").unwrap());
static HEAD_TS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]{1,9})?Z$")
        .unwrap()
});
static OBSERVED_TS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$").unwrap()
});
static MANIFEST_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$").unwrap());
static ANCHOR_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9]{4}-[0-9]{2}-[0-9]{2})\.head\.json$").unwrap());

// Errors

/// How a maintenance command failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The input was refused before anything was written.
    Refused,
    /// `ledger verify` did not accept the log, a head or a proof.
    Verification,
    /// The signed head covers fewer records than the log holds.
    StaleHead,
    /// Two signed heads disagree about the same history.
    Equivocation,
    /// A ledger write did not finish; the same command can be run again.
    Pending,
}

impl Failure {
    /// Every failure except [`Failure::Pending`] is a refusal.
    #[must_use]
    pub fn is_refusal(self) -> bool {
        self != Failure::Pending
    }
}

/// A maintenance command's failure and its message.
#[derive(Debug)]
pub struct MaintenanceError {
    /// What kind of failure.
    pub failure: Failure,
    /// The message printed for it.
    pub message: String,
}

impl Display for MaintenanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for MaintenanceError {}

fn fail(failure: Failure, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(MaintenanceError {
        failure,
        message: message.into(),
    })
}

fn refused(message: impl Into<String>) -> anyhow::Error {
    fail(Failure::Refused, message)
}

fn verification(message: impl Into<String>) -> anyhow::Error {
    fail(Failure::Verification, message)
}

fn equivocation(message: impl Into<String>) -> anyhow::Error {
    fail(Failure::Equivocation, message)
}

fn pending(message: impl Into<String>) -> anyhow::Error {
    fail(
        Failure::Pending,
        format!(
            "{}; nothing was acknowledged, retry the same command",
            message.into()
        ),
    )
}

/// The failure an error carries, when it is a maintenance error.
#[must_use]
pub fn failure_of(error: &anyhow::Error) -> Option<Failure> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<MaintenanceError>())
        .map(|error| error.failure)
}

/// The exit status of a maintenance error: [`PENDING_EXIT`] when a write
/// did not finish, else [`REFUSED_EXIT`]. `None` for any other error.
#[must_use]
pub fn exit_code(error: &anyhow::Error) -> Option<i32> {
    failure_of(error).map(|failure| match failure {
        Failure::Pending => PENDING_EXIT,
        _ => REFUSED_EXIT,
    })
}

/// `error` as a maintenance error: one that already carries a failure is
/// kept, any other (an unreadable file, a failed directory flush) is a
/// refusal.
#[must_use]
pub fn as_maintenance_error(error: anyhow::Error) -> anyhow::Error {
    if failure_of(&error).is_some() {
        error
    } else {
        refused(format!("{error:#}"))
    }
}

// Small helpers

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut text = String::with_capacity(64);
    for byte in digest {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn utc(seconds: i64) -> String {
    DateTime::from_timestamp(seconds, 0)
        .map(|time| time.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| seconds.to_string())
}

/// The current time in whole seconds since the epoch.
#[must_use]
pub fn now_seconds() -> i64 {
    chrono::Utc::now().timestamp()
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

fn io_refused(what: &str, path: &Path, error: &std::io::Error) -> anyhow::Error {
    refused(format!("cannot {what} {}: {error}", path.display()))
}

// JSON as the projection writes it

/// JSON text with keys sorted and every character outside printable ASCII
/// escaped. With `indent`, nested values go on their own lines indented by
/// that many spaces per level and items are separated by `,`; without, the
/// value is one line with `, ` and `: ` separators.
#[must_use]
pub fn dumps(value: &Value, indent: Option<usize>) -> String {
    let mut out = String::new();
    write_json(value, indent, 0, &mut out);
    out
}

fn write_json(value: &Value, indent: Option<usize>, level: usize, out: &mut String) {
    let separator = |out: &mut String, at: usize, depth: usize| match indent {
        Some(width) => {
            if at > 0 {
                out.push(',');
            }
            out.push('\n');
            out.push_str(&" ".repeat(width * depth));
        }
        None => {
            if at > 0 {
                out.push_str(", ");
            }
        }
    };
    let close = |out: &mut String| {
        if let Some(width) = indent {
            out.push('\n');
            out.push_str(&" ".repeat(width * level));
        }
    };
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::String(text) => write_ascii_string(text, out),
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items) => {
            out.push('[');
            for (at, item) in items.iter().enumerate() {
                separator(out, at, level + 1);
                write_json(item, indent, level + 1, out);
            }
            close(out);
            out.push(']');
        }
        Value::Object(map) if map.is_empty() => out.push_str("{}"),
        Value::Object(map) => {
            out.push('{');
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            for (at, key) in keys.into_iter().enumerate() {
                separator(out, at, level + 1);
                write_ascii_string(key, out);
                out.push_str(": ");
                write_json(&map[key], indent, level + 1, out);
            }
            close(out);
            out.push('}');
        }
    }
}

fn write_ascii_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(ch),
            _ => {
                let mut units = [0u16; 2];
                for unit in ch.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

/// JSON that refuses a repeated object key and any non-integer number.
struct Strict;

impl<'de> DeserializeSeed<'de> for Strict {
    type Value = Value;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Strict {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Err(E::custom(format!("holds the non-integer number {value}")))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        Strict.deserialize(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(Strict)? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(de::Error::custom(format!("repeats the key {key:?}")));
            }
            let value = map.next_value_seed(Strict)?;
            object.insert(key, value);
        }
        Ok(Value::Object(object))
    }
}

fn strict_json(text: &str, what: &str) -> Result<Value> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = Strict
        .deserialize(&mut deserializer)
        .and_then(|value| deserializer.end().map(|()| value))
        .map_err(|error| refused(format!("{what} is not strict JSON: {error}")))?;
    Ok(value)
}

// Paths

/// `path` as POSIX `normpath` spells it.
fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let initial = if path.starts_with("//") && !path.starts_with("///") {
        2
    } else if path.starts_with('/') {
        1
    } else {
        0
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part != ".." || (initial == 0 && parts.is_empty()) || parts.last() == Some(&"..") {
            parts.push(part);
        } else if !parts.is_empty() {
            parts.pop();
        }
    }
    let joined = format!("{}{}", "/".repeat(initial), parts.join("/"));
    if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

/// `path` with every symbolic link in its existing part resolved; a part
/// that does not exist is kept as written.
fn real_path(path: &Path) -> PathBuf {
    match path.canonicalize() {
        Ok(resolved) => resolved,
        Err(_) => match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => real_path(parent).join(name),
            _ => path.to_path_buf(),
        },
    }
}

/// `path` as given, refused unless absolute, normalised and free of
/// symbolic links.
fn plain_path<'a>(path: &'a Path, what: &str) -> Result<&'a str> {
    let Some(text) = path.to_str().filter(|text| !text.is_empty()) else {
        return Err(refused(format!("{what} must be a path")));
    };
    if !text.starts_with('/') || normpath(text) != text || text.starts_with("//") {
        return Err(refused(format!(
            "{what} {text:?} must be an absolute, normalised path"
        )));
    }
    let real = real_path(path);
    if real != path {
        return Err(refused(format!(
            "{what} {text} resolves to {}; path aliases are refused",
            real.display()
        )));
    }
    Ok(text)
}

fn existing_file(path: &Path, what: &str, single_link: bool) -> Result<()> {
    plain_path(path, what)?;
    let info = match std::fs::symlink_metadata(path) {
        Ok(info) => info,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(refused(format!("{what} {} does not exist", path.display())))
        }
        Err(error) => return Err(io_refused(&format!("inspect {what}"), path, &error)),
    };
    if !info.file_type().is_file() {
        return Err(refused(format!(
            "{what} {} is not a regular file",
            path.display()
        )));
    }
    if single_link && info.nlink() != 1 {
        return Err(refused(format!(
            "{what} {} has {} hard links; aliases are refused",
            path.display(),
            info.nlink()
        )));
    }
    Ok(())
}

fn lexists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn new_or_file(path: &Path, what: &str) -> Result<()> {
    plain_path(path, what)?;
    let parent = path.parent().unwrap_or(Path::new("/"));
    let is_dir = std::fs::symlink_metadata(parent).is_ok_and(|info| info.file_type().is_dir());
    if !is_dir {
        return Err(refused(format!(
            "the directory of {what} {} does not exist",
            path.display()
        )));
    }
    if lexists(path) {
        existing_file(path, what, true)?;
    }
    Ok(())
}

fn existing_dir(path: &Path, what: &str) -> Result<()> {
    plain_path(path, what)?;
    let info = match std::fs::symlink_metadata(path) {
        Ok(info) => info,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(refused(format!("{what} {} does not exist", path.display())))
        }
        Err(error) => return Err(io_refused(&format!("inspect {what}"), path, &error)),
    };
    if !info.file_type().is_dir() {
        return Err(refused(format!(
            "{what} {} is not a directory",
            path.display()
        )));
    }
    Ok(())
}

/// Whether `child` is `parent` or lies under it, by spelling.
fn inside(child: &Path, parent: &Path) -> bool {
    let (child, parent) = (display(child), display(parent));
    child == parent || child.starts_with(&format!("{}/", parent.trim_end_matches('/')))
}

fn dirname(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new("/"))
}

/// Refuse two roles that name one file, by path or by inode.
fn distinct_files(named: &[(&str, Option<&Path>)]) -> Result<()> {
    #[derive(PartialEq)]
    enum Identity {
        Inode(u64, u64),
        Missing(PathBuf),
    }
    let mut seen: Vec<(Identity, &str)> = Vec::new();
    for (what, path) in named {
        let Some(path) = path else { continue };
        let identity = match std::fs::metadata(path) {
            Ok(info) => Identity::Inode(info.dev(), info.ino()),
            Err(_) => Identity::Missing(path.to_path_buf()),
        };
        if let Some((_, other)) = seen.iter().find(|(known, _)| *known == identity) {
            return Err(refused(format!(
                "{what} {} is the same file as {other}",
                path.display()
            )));
        }
        seen.push((identity, what));
    }
    Ok(())
}

fn key_outside(key: &Path, directories: &[(&str, &Path)]) -> Result<()> {
    for (what, directory) in directories {
        if inside(key, directory) {
            return Err(refused(format!(
                "signing key {} is inside the {what} {}; keep the key off that tree",
                key.display(),
                directory.display()
            )));
        }
    }
    Ok(())
}

/// The bytes of a regular file opened without following a symbolic link.
fn read_regular(path: &Path, what: &str) -> Result<Vec<u8>> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return Err(refused(format!(
                "{what} {} is a symbolic link",
                path.display()
            )))
        }
        Err(error) => return Err(io_refused(&format!("open {what}"), path, &error)),
    };
    let info = file
        .metadata()
        .map_err(|error| io_refused(&format!("inspect {what}"), path, &error))?;
    if !info.is_file() {
        return Err(refused(format!(
            "{what} {} is not a regular file",
            path.display()
        )));
    }
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|error| io_refused(&format!("read {what}"), path, &error))?;
    Ok(data)
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| io_refused("create", path, &error))?;
    file.write_all(data)
        .and_then(|()| file.sync_all())
        .map_err(|error| io_refused("write", path, &error))
}

fn pin(directory: &Path, name: &str, data: &[u8]) -> Result<PathBuf> {
    let path = directory.join(name);
    write_private(&path, data)?;
    Ok(path)
}

fn fsync_dir(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_refused("flush directory", path, &error))
}

fn private_dir(prefix: &str) -> Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir()
        .map_err(|error| refused(format!("cannot create a private directory: {error}")))
}

// Log and head readers

/// A signed head file exactly as `ledger head` and `ledger append` write it.
pub fn parse_head(data: &[u8], what: &str) -> Result<SignedHead> {
    let text = std::str::from_utf8(data).map_err(|_| refused(format!("{what} is not UTF-8")))?;
    let value = strict_json(text, what)?;
    let exact = || {
        refused(format!(
            "{what} must hold exactly {}, in that order",
            HEAD_FIELDS.join(", ")
        ))
    };
    let Value::Object(map) = &value else {
        return Err(exact());
    };
    let mut keys = map.keys().map(String::as_str).collect::<Vec<_>>();
    let mut expected = HEAD_FIELDS.to_vec();
    keys.sort_unstable();
    expected.sort_unstable();
    if keys != expected {
        return Err(exact());
    }
    if map["domain"] != ledger::DOMAIN {
        return Err(refused(format!(
            "{what} domain is {}, not {}",
            map["domain"],
            ledger::DOMAIN
        )));
    }
    let size = map["size"]
        .as_u64()
        .and_then(|size| usize::try_from(size).ok())
        .ok_or_else(|| refused(format!("{what} size must be a non-negative integer")))?;
    let hex = |field: &str, pattern: &Regex, length: usize| -> Result<String> {
        match map[field].as_str() {
            Some(text) if pattern.is_match(text) => Ok(text.to_string()),
            _ => Err(refused(format!(
                "{what} {field} must be {length} lower-case hex characters"
            ))),
        }
    };
    let root = hex("root", &HEX64, 64)?;
    let public_key = hex("public_key", &HEX64, 64)?;
    let signature = hex("signature", &HEX128, 128)?;
    let ts = match map["ts"].as_str() {
        Some(ts) if HEAD_TS.is_match(ts) => ts.to_string(),
        _ => {
            return Err(refused(format!(
                "{what} ts must be RFC 3339 UTC ending in Z"
            )))
        }
    };
    let head = SignedHead {
        domain: ledger::DOMAIN.to_string(),
        size,
        root,
        ts,
        public_key,
        signature,
    };
    let form = serde_json::to_string(&head)? + "\n";
    if form != text {
        return Err(refused(format!(
            "{what} is not in the form eb-stack writes"
        )));
    }
    Ok(head)
}

/// Records of a ledger log, every line checked as [`ledger::verify`] reads
/// it. A log ending in a partial line is refused rather than repaired.
pub fn parse_log(data: &[u8], what: &str) -> Result<Vec<Entry>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    if !data.ends_with(b"\n") {
        return Err(refused(format!(
            "the last line of {what} has no newline; it ends in a partial write"
        )));
    }
    let text = std::str::from_utf8(data).map_err(|_| refused(format!("{what} is not UTF-8")))?;
    ledger::parse_log(text).map_err(|error| {
        verification(format!(
            "{what} is not a log `ledger verify` accepts: {error:#}"
        ))
    })
}

/// The bytes up to the last newline: a concurrent append may be mid-line.
fn committed_prefix(data: &[u8]) -> &[u8] {
    match data.iter().rposition(|byte| *byte == b'\n') {
        Some(end) => &data[..=end],
        None => &[],
    }
}

/// Write the tile log beside a private copy of a log; verify audits it.
fn tiles(log: &Path, entries: &[Entry]) -> Result<()> {
    crate::ledger_tiles::sync(entries, &crate::ledger_tiles::tiles_dir(log, None))
        .map_err(|error| verification(format!("`ledger tiles` failed: {error:#}")))?;
    Ok(())
}

/// [`ledger::verify`] against private copies of the bytes read.
struct Verifier<'a> {
    public_key: &'a Path,
}

impl Verifier<'_> {
    /// Run one proof. The receipt must be `expected`, then one tile-log line
    /// for `size` and `root`. A size-0 head over an empty log was signed
    /// without a checkpoint and no tile log is audited for it, so the
    /// receipt stands alone.
    fn run(
        &self,
        log: &Path,
        mode: VerifyMode,
        expected: &str,
        size: usize,
        root: &str,
    ) -> Result<()> {
        let report = ledger::verify(&VerifyRequest {
            log: log.to_path_buf(),
            public_key: self.public_key.to_path_buf(),
            mode,
            tiles: None,
            origin: None,
        })
        .map_err(|error| verification(format!("`ledger verify` refused: {error:#}")))?;
        let receipt = if size == 0 {
            expected.to_string()
        } else {
            format!(
                "{expected}tile log size {size} root {root} extends the head and holds the same lines\n"
            )
        };
        if report.stdout != receipt {
            return Err(verification(format!(
                "`ledger verify` printed an unexpected receipt: {:?}",
                report.stdout
            )));
        }
        Ok(())
    }

    /// The signed head is the root of the first `head.size` records of `log`.
    fn binds(
        &self,
        log: &Path,
        head_path: &Path,
        head: &SignedHead,
        records: &[Entry],
    ) -> Result<()> {
        let (size, root) = (head.size, head.root.as_str());
        if size > records.len() {
            return Err(verification(format!(
                "head covers {size} records but the ledger has {}",
                records.len()
            )));
        }
        if size == 0 {
            return self.run(
                log,
                VerifyMode::Consistency {
                    from: head_path.to_path_buf(),
                    to: head_path.to_path_buf(),
                },
                &format!("the head of size 0 root {root} extends the head of size 0 root {root}\n"),
                size,
                root,
            );
        }
        let last = size - 1;
        self.run(
            log,
            VerifyMode::Inclusion {
                record: last,
                head: head_path.to_path_buf(),
            },
            &format!(
                "record {last} id {} is in the head of size {size} root {root}\n",
                records[last].id
            ),
            size,
            root,
        )
    }

    fn extends(
        &self,
        log: &Path,
        older_path: &Path,
        older: &SignedHead,
        newer_path: &Path,
        newer: &SignedHead,
    ) -> Result<()> {
        if older.size == newer.size && older.root != newer.root {
            return Err(equivocation(format!(
                "two signed heads of size {} have roots {} and {}",
                older.size, older.root, newer.root
            )));
        }
        if older.size > newer.size {
            return Err(equivocation(format!(
                "the head of size {} is older than an anchored head of size {}",
                newer.size, older.size
            )));
        }
        self.run(
            log,
            VerifyMode::Consistency {
                from: older_path.to_path_buf(),
                to: newer_path.to_path_buf(),
            },
            &format!(
                "the head of size {} root {} extends the head of size {} root {}\n",
                newer.size, newer.root, older.size, older.root
            ),
            newer.size,
            &newer.root,
        )
        .map_err(|error| {
            equivocation(format!(
                "the head of size {} does not extend the anchored head: {error}",
                newer.size
            ))
        })
    }
}

/// Records and the signed head [`ledger::verify`] accepted for them.
#[derive(Debug, Clone)]
pub struct VerifiedLog {
    /// The signed head, which covers every record.
    pub head: SignedHead,
    /// The records.
    pub records: Vec<Entry>,
}

/// Read `log` and `head` once and verify those bytes.
///
/// The head must be signed by `public_key` and cover every record: a head
/// that covers fewer is stale and refused, so nothing unverified is read.
pub fn read_verified(log: &Path, head: &Path, public_key: &Path) -> Result<VerifiedLog> {
    existing_file(log, "ledger", true)?;
    existing_file(head, "head", true)?;
    existing_file(public_key, "public key", false)?;
    distinct_files(&[
        ("ledger", Some(log)),
        ("head", Some(head)),
        ("public key", Some(public_key)),
    ])?;
    let log_bytes = read_regular(log, "ledger")?;
    let head_bytes = read_regular(head, "head")?;
    let signed = parse_head(&head_bytes, &format!("head {}", head.display()))?;
    let records = parse_log(&log_bytes, &format!("ledger {}", log.display()))?;
    if signed.size > records.len() {
        return Err(verification(format!(
            "head {} covers {} records but ledger {} has {}",
            head.display(),
            signed.size,
            log.display(),
            records.len()
        )));
    }
    if signed.size < records.len() {
        return Err(fail(
            Failure::StaleHead,
            format!(
                "head {} covers {} of the {} records in {}; sign a current head before reading",
                head.display(),
                signed.size,
                records.len(),
                log.display()
            ),
        ));
    }
    let private = private_dir("eb-ledger-verify-")?;
    let copy_log = pin(private.path(), "ledger.jsonl", &log_bytes)?;
    let copy_head = pin(private.path(), "head.json", &head_bytes)?;
    tiles(&copy_log, &records)?;
    Verifier { public_key }.binds(&copy_log, &copy_head, &signed, &records)?;
    Ok(VerifiedLog {
        head: signed,
        records,
    })
}

// Observed-history backfill

fn is_control(ch: char) -> bool {
    matches!(
        u32::from(ch),
        0x00..=0x1f | 0x7f..=0x9f | 0x2028 | 0x2029 | 0x202a..=0x202e | 0x2066..=0x2069
    )
}

fn label(what: &str) -> String {
    if what.is_empty() {
        "manifest".to_string()
    } else {
        format!("manifest {what}")
    }
}

fn manifest_text(value: Option<&Value>, what: &str, path: bool) -> Result<String> {
    let text = match value {
        Some(Value::String(text))
            if !text.is_empty() && text.trim() == text && !text.chars().any(is_control) =>
        {
            text
        }
        _ => {
            return Err(refused(format!(
            "manifest {what} must be a non-empty string without control characters or edge spaces"
        )))
        }
    };
    if path && (!text.starts_with('/') || normpath(text) != *text) {
        return Err(refused(format!(
            "manifest {what} must be an absolute, normalised path"
        )));
    }
    Ok(text.clone())
}

fn only<'a>(value: &'a Value, allowed: &[&str], what: &str) -> Result<&'a Map<String, Value>> {
    let Value::Object(map) = value else {
        return Err(refused(format!("{} must be a JSON object", label(what))));
    };
    let extra = map
        .keys()
        .filter(|key| !allowed.contains(&key.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>();
    if !extra.is_empty() {
        return Err(refused(format!(
            "{} has unknown fields {}",
            label(what),
            extra.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    Ok(map)
}

fn hex64(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| HEX64.is_match(text))
}

/// An observed-history manifest, refused unless its bytes hash to
/// `expected_sha256` and it holds nothing after `now`. Returns the manifest
/// and its SHA-256.
pub fn load_manifest(
    path: &Path,
    expected_sha256: &str,
    now: i64,
) -> Result<(Map<String, Value>, String)> {
    existing_file(path, "backfill manifest", true)?;
    if !HEX64.is_match(expected_sha256) {
        return Err(refused(
            "the manifest SHA-256 must be 64 lower-case hex characters",
        ));
    }
    let data = read_regular(path, "backfill manifest")?;
    let digest = sha256_hex(&data);
    if digest != expected_sha256 {
        return Err(refused(format!(
            "manifest {} hashes to {digest}, not the pinned {expected_sha256}",
            path.display()
        )));
    }
    let text = std::str::from_utf8(&data)
        .map_err(|_| refused(format!("manifest {} is not UTF-8", path.display())))?;
    let value = strict_json(text, &format!("manifest {}", path.display()))?;
    let manifest = only(
        &value,
        &[
            "schema",
            "manifest_id",
            "observed_at",
            "observer",
            "stack",
            "target",
            "observations",
        ],
        "",
    )?;
    if manifest.get("schema").and_then(Value::as_str) != Some(MANIFEST_SCHEMA) {
        return Err(refused(format!(
            "manifest schema must be {MANIFEST_SCHEMA}"
        )));
    }
    if !manifest
        .get("manifest_id")
        .and_then(Value::as_str)
        .is_some_and(|id| MANIFEST_ID.is_match(id))
    {
        return Err(refused(
            "manifest manifest_id must be 1-64 of A-Z a-z 0-9 . _ - starting alphanumeric",
        ));
    }
    let observed = match manifest.get("observed_at").and_then(Value::as_str) {
        Some(observed) if OBSERVED_TS.is_match(observed) => observed,
        _ => return Err(refused("manifest observed_at must be YYYY-MM-DDTHH:MM:SSZ")),
    };
    let seconds = NaiveDateTime::parse_from_str(observed, "%Y-%m-%dT%H:%M:%SZ")
        .map_err(|_| refused(format!("manifest observed_at {observed} is not a time")))?
        .and_utc()
        .timestamp();
    if seconds > now {
        return Err(refused(format!(
            "manifest observed_at {observed} is after the current time {}",
            utc(now)
        )));
    }
    for key in ["observer", "stack", "target"] {
        manifest_text(manifest.get(key), key, false)?;
    }
    let observations = match manifest.get("observations") {
        Some(Value::Array(items)) if !items.is_empty() => items,
        _ => return Err(refused("manifest observations must be a non-empty list")),
    };
    let mut seen = HashSet::new();
    for (number, item) in observations.iter().enumerate() {
        let place = format!("observations[{number}]");
        let item = only(
            item,
            &["module", "location", "modulefile", "provenance", "note"],
            &place,
        )?;
        let module = manifest_text(item.get("module"), &format!("{place}.module"), false)?;
        let location = manifest_text(item.get("location"), &format!("{place}.location"), true)?;
        if !seen.insert((module.clone(), location.clone())) {
            return Err(refused(format!(
                "manifest {place} repeats {module} at {location}"
            )));
        }
        if let Some(note) = item.get("note") {
            manifest_text(Some(note), &format!("{place}.note"), false)?;
        }
        if let Some(modulefile) = item.get("modulefile") {
            let where_ = format!("{place}.modulefile");
            let modulefile = only(modulefile, &["path", "sha256"], &where_)?;
            manifest_text(modulefile.get("path"), &format!("{where_}.path"), true)?;
            if !hex64(modulefile.get("sha256")) {
                return Err(refused(format!(
                    "manifest {where_}.sha256 must be 64 lower-case hex characters"
                )));
            }
        }
        let provenance = match item.get("provenance") {
            Some(Value::Array(refs)) if !refs.is_empty() => refs,
            _ => {
                return Err(refused(format!(
                    "manifest {place}.provenance must name at least one original reference"
                )))
            }
        };
        for (ref_number, reference) in provenance.iter().enumerate() {
            let where_ = format!("{place}.provenance[{ref_number}]");
            let reference = only(reference, &["ref", "kind", "sha256"], &where_)?;
            manifest_text(reference.get("ref"), &format!("{where_}.ref"), false)?;
            if let Some(kind) = reference.get("kind") {
                manifest_text(Some(kind), &format!("{where_}.kind"), false)?;
            }
            if reference.contains_key("sha256") && !hex64(reference.get("sha256")) {
                return Err(refused(format!(
                    "manifest {where_}.sha256 must be 64 lower-case hex characters"
                )));
            }
        }
    }
    Ok((manifest.clone(), digest))
}

/// One `finding` record per observation, stamped with the observation time.
///
/// Nothing claims an install or a build happened: the record says the
/// software was seen at `observed_at` and names where the evidence is.
#[must_use]
pub fn backfill_records(manifest: &Map<String, Value>, digest: &str) -> Vec<Value> {
    let field = |name: &str| manifest[name].as_str().unwrap_or_default().to_string();
    let (manifest_id, observed_at) = (field("manifest_id"), field("observed_at"));
    let observations = manifest["observations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    observations
        .iter()
        .map(|item| {
            let module = item["module"].as_str().unwrap_or_default();
            let location = item["location"].as_str().unwrap_or_default();
            let place = &sha256_hex(location.as_bytes())[..16];
            let mut build = json!({
                "event_id": format!("eb-staged-rebuild/backfill/{manifest_id}/{module}@{place}"),
                "tool": TOOL,
                "kind": BACKFILL_KIND,
                "manifest_id": manifest_id,
                "manifest_sha256": digest,
                "observed_at": observed_at,
                "provenance": item["provenance"],
            });
            if let Some(note) = item.get("note") {
                build["note"] = note.clone();
            }
            let mut record = json!({
                "ts": observed_at,
                "actor": field("observer"),
                "stack": field("stack"),
                "target": field("target"),
                "module": module,
                "location": location,
                "action": "finding",
                "reason": format!(
                    "observed by backfill manifest {manifest_id} at {observed_at}; \
                     no history before the observation is recorded"
                ),
                "build": build,
            });
            if let Some(modulefile) = item.get("modulefile") {
                record["modulefile"] = modulefile.clone();
            }
            record
        })
        .collect()
}

/// What `ledger backfill` needs.
#[derive(Debug, Clone)]
pub struct BackfillRequest {
    /// The JSON Lines log. Created when absent.
    pub log: PathBuf,
    /// Ed25519 PKCS#8 PEM key, off the log's and the head's trees.
    pub key: PathBuf,
    /// File for the signed head after each append.
    pub head: PathBuf,
    /// The public key the head must verify under.
    pub public_key: PathBuf,
    /// The observed-history manifest.
    pub manifest: PathBuf,
    /// The manifest's SHA-256, lower-case hex.
    pub manifest_sha256: String,
    /// The current time, seconds since the epoch.
    pub now: i64,
}

fn body(record: &Map<String, Value>) -> Value {
    let mut body = record.clone();
    for key in ["id", "index", "sig"] {
        body.remove(key);
    }
    Value::Object(body)
}

/// Append the manifest's observations, each at most once.
///
/// Every record is built from the manifest alone, so a retry sends the same
/// bytes and the ledger acknowledges it as already recorded. The log is
/// checked first: a manifest id already backfilled from other bytes, or an
/// event id recorded with other content, is refused before anything is
/// appended. Each receipt needs [`ledger::verify`] to prove the record is in
/// the signed head written by the append.
pub fn backfill(request: &BackfillRequest) -> Result<Value> {
    let (log, key, head) = (&request.log, &request.key, &request.head);
    new_or_file(log, "ledger")?;
    new_or_file(head, "head")?;
    existing_file(key, "signing key", true)?;
    existing_file(&request.public_key, "public key", false)?;
    distinct_files(&[
        ("ledger", Some(log.as_path())),
        ("head", Some(head.as_path())),
        ("signing key", Some(key.as_path())),
        ("public key", Some(request.public_key.as_path())),
        ("backfill manifest", Some(request.manifest.as_path())),
    ])?;
    key_outside(
        key,
        &[
            ("ledger directory", dirname(log)),
            ("head directory", dirname(head)),
        ],
    )?;
    let (manifest, digest) =
        load_manifest(&request.manifest, &request.manifest_sha256, request.now)?;
    let records = backfill_records(&manifest, &digest);
    let manifest_id = manifest["manifest_id"].as_str().unwrap_or_default();
    let stack = manifest["stack"].as_str().unwrap_or_default();
    // The ledger quarantines an interrupted tail on the next append.
    let existing = if lexists(log) {
        let data = read_regular(log, "ledger")?;
        parse_log(
            committed_prefix(&data),
            &format!("ledger {}", log.display()),
        )?
    } else {
        Vec::new()
    };
    if let Some(first) = existing.first() {
        if first.record["stack"] != stack {
            return Err(refused(format!(
                "manifest stack {stack:?} is not the ledger's stack {}",
                first.record["stack"]
            )));
        }
    }
    let mut by_event: HashMap<String, &Entry> = HashMap::new();
    for entry in &existing {
        let Some(build) = entry.record.get("build").and_then(Value::as_object) else {
            continue;
        };
        if build.get("kind").and_then(Value::as_str) == Some(BACKFILL_KIND)
            && build.get("manifest_id").and_then(Value::as_str) == Some(manifest_id)
            && build.get("manifest_sha256").and_then(Value::as_str) != Some(digest.as_str())
        {
            return Err(refused(format!(
                "manifest {manifest_id} was backfilled from bytes hashing to {}, not {digest}; \
                 a backfill manifest is immutable",
                build.get("manifest_sha256").unwrap_or(&Value::Null)
            )));
        }
        if let Some(event_id) = build.get("event_id").and_then(Value::as_str) {
            by_event.insert(event_id.to_string(), entry);
        }
    }
    for record in &records {
        let event_id = record["build"]["event_id"].as_str().unwrap_or_default();
        if let Some(entry) = by_event.get(event_id) {
            if body(&entry.record) != *record {
                return Err(refused(format!(
                    "ledger record {} holds {event_id} with other content",
                    entry.index
                )));
            }
        }
    }
    let private = private_dir("eb-ledger-backfill-")?;
    let mut receipts = Vec::new();
    for (number, record) in records.iter().enumerate() {
        let event_id = record["build"]["event_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let path = private.path().join(format!("{number:06}.json"));
        write_private(&path, (dumps(record, None) + "\n").as_bytes())?;
        let mut receipt = append_one(request, &path, &event_id)?;
        receipt["event_id"] = Value::from(event_id);
        receipts.push(receipt);
    }
    let replayed = receipts
        .iter()
        .filter(|receipt| receipt["replayed"] == true)
        .count();
    Ok(json!({
        "manifest_id": manifest_id,
        "manifest_sha256": digest,
        "appended": receipts.len() - replayed,
        "replayed": replayed,
        "receipts": receipts,
    }))
}

/// Append one record and prove it is in the head the append signed.
fn append_one(request: &BackfillRequest, record: &Path, event_id: &str) -> Result<Value> {
    let report = ledger::append(&ledger::AppendRequest {
        log: request.log.clone(),
        record: record.to_path_buf(),
        expect_head: None,
        key: Some(request.key.clone()),
        head_out: Some(request.head.clone()),
        tiles: None,
        origin: None,
    })
    .map_err(|error| pending(format!("`ledger append` failed: {error:#}")))?;
    let (index, size, root) = (report.index, report.head.size, &report.head.root);
    let written = SignedHead::read(&request.head).map_err(|error| {
        pending(format!(
            "cannot read head {}: {error:#}",
            request.head.display()
        ))
    })?;
    if written.size != size || written.root != *root || index >= size {
        return Err(pending(format!(
            "head {} is size {} root {}, not the reported size {size} root {root} covering record {index}",
            request.head.display(),
            written.size,
            written.root
        )));
    }
    let log = std::fs::read(&request.log).map_err(|error| {
        pending(format!(
            "cannot read ledger {}: {error}",
            request.log.display()
        ))
    })?;
    let logged = log
        .split(|byte| *byte == b'\n')
        .nth(index)
        .and_then(|line| serde_json::from_slice::<Value>(line).ok())
        .and_then(|line| line["build"]["event_id"].as_str().map(str::to_string));
    if logged.as_deref() != Some(event_id) {
        return Err(pending(format!(
            "ledger record {index} is not event {event_id}"
        )));
    }
    ledger::verify(&VerifyRequest {
        log: request.log.clone(),
        public_key: request.public_key.clone(),
        mode: VerifyMode::Inclusion {
            record: index,
            head: request.head.clone(),
        },
        tiles: None,
        origin: None,
    })
    .map_err(|error| pending(format!("`ledger verify` failed: {error:#}")))?;
    Ok(json!({
        "index": index,
        "id": report.id,
        "head_size": size,
        "head_root": root,
        "replayed": report.replayed,
        "verified": true,
    }))
}

// Daily anchors

/// One anchor file and the head it holds.
#[derive(Debug, Clone)]
struct Anchor {
    day: String,
    path: PathBuf,
    head: SignedHead,
    bytes: Vec<u8>,
}

/// Anchors by day, each a single-link regular file whose head is signed
/// that day, each extending the one before it.
fn list_anchors(anchors: &Path) -> Result<Vec<Anchor>> {
    let mut names = std::fs::read_dir(anchors)
        .map_err(|error| io_refused("list anchor directory", anchors, &error))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect::<Vec<_>>();
    names.sort();
    let mut found: Vec<Anchor> = Vec::new();
    for name in names {
        let Some(day) = ANCHOR_NAME
            .captures(&name)
            .map(|groups| groups[1].to_string())
        else {
            continue;
        };
        let path = anchors.join(&name);
        let info = std::fs::symlink_metadata(&path)
            .map_err(|error| io_refused("inspect anchor", &path, &error))?;
        if !info.file_type().is_file() || info.nlink() != 1 {
            return Err(refused(format!(
                "anchor {} must be a regular file with one link",
                path.display()
            )));
        }
        let bytes = read_regular(&path, "anchor")?;
        let head = parse_head(&bytes, &format!("anchor {}", path.display()))?;
        if head.ts[..10] != day {
            return Err(refused(format!(
                "anchor {} holds a head signed at {}, not on {day}",
                path.display(),
                head.ts
            )));
        }
        found.push(Anchor {
            day,
            path,
            head,
            bytes,
        });
    }
    for pair in found.windows(2) {
        let (older, newer) = (&pair[0], &pair[1]);
        let (a, b) = (&older.head, &newer.head);
        if a.public_key != b.public_key {
            return Err(equivocation(format!(
                "anchors {} and {} are signed by different keys",
                older.path.display(),
                newer.path.display()
            )));
        }
        if b.ts < a.ts {
            return Err(refused(format!(
                "anchor {} is signed before {}",
                newer.path.display(),
                older.path.display()
            )));
        }
        if b.size < a.size || (b.size == a.size && b.root != a.root) {
            return Err(equivocation(format!(
                "anchor {} does not extend {}",
                newer.path.display(),
                older.path.display()
            )));
        }
    }
    Ok(found)
}

/// Remove what an interrupted run left in the anchor directory.
fn clear_staging(anchors: &Path) -> Result<()> {
    let entries = std::fs::read_dir(anchors)
        .map_err(|error| io_refused("list anchor directory", anchors, &error))?;
    for entry in entries {
        let entry = entry.map_err(|error| io_refused("list anchor directory", anchors, &error))?;
        if !entry
            .file_name()
            .as_encoded_bytes()
            .starts_with(STAGING_PREFIX.as_bytes())
        {
            continue;
        }
        let path = entry.path();
        let is_dir = std::fs::symlink_metadata(&path).is_ok_and(|info| info.file_type().is_dir());
        let removed = if is_dir {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        removed.map_err(|error| io_refused("remove", &path, &error))?;
    }
    fsync_dir(anchors)
}

fn anchor_paths(log: &Path, key: Option<&Path>, public_key: &Path, anchors: &Path) -> Result<()> {
    existing_file(log, "ledger", true)?;
    existing_file(public_key, "public key", false)?;
    existing_dir(anchors, "anchor directory")?;
    if let Some(key) = key {
        existing_file(key, "signing key", true)?;
        key_outside(
            key,
            &[
                ("ledger directory", dirname(log)),
                ("anchor directory", anchors),
            ],
        )?;
    }
    if anchors == dirname(log) || inside(log, anchors) {
        return Err(refused(format!(
            "anchor directory {} must not hold the ledger {}",
            anchors.display(),
            log.display()
        )));
    }
    distinct_files(&[
        ("ledger", Some(log)),
        ("signing key", key),
        ("public key", Some(public_key)),
    ])
}

/// The anchor directory's lock: a `flock` gate on `.lock` and a POSIX
/// record lock on `.lock.posix`, released in the reverse order.
struct DirectoryLock {
    gate: Option<File>,
    record: Option<File>,
}

impl DirectoryLock {
    fn open(directory: &Path, name: &str) -> Result<File> {
        let path = directory.join(name);
        let private = |info: &std::fs::Metadata| info.file_type().is_file() && info.nlink() == 1;
        if let Ok(info) = std::fs::symlink_metadata(&path) {
            if !private(&info) {
                return Err(refused(format!(
                    "lock file must be a private regular file: {}",
                    path.display()
                )));
            }
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|error| io_refused("open lock", &path, &error))?;
        let info = file
            .metadata()
            .map_err(|error| io_refused("inspect lock", &path, &error))?;
        if !private(&info) {
            return Err(refused(format!(
                "lock file must be a private regular file: {}",
                path.display()
            )));
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| io_refused("set the mode of lock", &path, &error))?;
        Ok(file)
    }

    /// Retry `take` until it succeeds or `deadline` passes.
    fn wait(
        path: &Path,
        deadline: Instant,
        mut take: impl FnMut() -> std::io::Result<()>,
    ) -> Result<()> {
        loop {
            match take() {
                Ok(()) => return Ok(()),
                Err(error)
                    if error.raw_os_error() == Some(libc::EWOULDBLOCK)
                        || error.raw_os_error() == Some(libc::EAGAIN)
                        || error.raw_os_error() == Some(libc::EACCES)
                        || error.kind() == std::io::ErrorKind::Interrupted =>
                {
                    if Instant::now() >= deadline {
                        return Err(refused(format!(
                            "another anchor run holds the lock at {}",
                            path.display()
                        )));
                    }
                    std::thread::sleep(LOCK_RETRY);
                }
                Err(error) => return Err(io_refused("lock", path, &error)),
            }
        }
    }

    fn acquire(directory: &Path, timeout: Duration) -> Result<Self> {
        let deadline = Instant::now() + timeout;
        let mut lock = Self {
            gate: None,
            record: None,
        };
        let gate = Self::open(directory, ".lock")?;
        Self::wait(&directory.join(".lock"), deadline, || {
            // The descriptor is open for the duration of the call.
            let result = unsafe { libc::flock(gate.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        })?;
        lock.gate = Some(gate);
        let record = Self::open(directory, ".lock.posix")?;
        Self::wait(&directory.join(".lock.posix"), deadline, || {
            // All-zero offsets cover the whole file; flock's scalar fields
            // have valid zero representations on the supported Unix targets.
            let mut request: libc::flock = unsafe { std::mem::zeroed() };
            request.l_type = libc::F_WRLCK as _;
            request.l_whence = libc::SEEK_SET as _;
            // The descriptor stays open and request is fully initialized.
            let result = unsafe { libc::fcntl(record.as_raw_fd(), libc::F_SETLK, &request) };
            if result == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        })?;
        lock.record = Some(record);
        if let Some(gate) = &mut lock.gate {
            let path = directory.join(".lock");
            gate.set_len(0)
                .and_then(|()| gate.write_all(format!("{}\n", std::process::id()).as_bytes()))
                .map_err(|error| io_refused("write lock", &path, &error))?;
        }
        Ok(lock)
    }
}

impl Drop for DirectoryLock {
    fn drop(&mut self) {
        // Closing the record lock's descriptor releases it before the gate.
        drop(self.record.take());
        if let Some(gate) = self.gate.take() {
            // The descriptor is open until it is dropped below.
            unsafe { libc::flock(gate.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

/// What `ledger anchor` needs.
#[derive(Debug, Clone)]
pub struct AnchorRequest {
    /// The JSON Lines log.
    pub log: PathBuf,
    /// Ed25519 PKCS#8 PEM key, off the log's and the anchors' trees.
    pub key: PathBuf,
    /// The public key every anchor must verify under.
    pub public_key: PathBuf,
    /// The directory of daily anchors, outside the log's directory.
    pub anchors: PathBuf,
    /// The current time, seconds since the epoch.
    pub now: i64,
}

/// Publish today's signed head as `<anchors>/<YYYY-MM-DD>.head.json`, once.
///
/// The head is signed now, for the day it is signed on: no anchor is made
/// for another day. It must bind to the log and extend the latest anchor,
/// both proved by [`ledger::verify`], before it is linked into place. An
/// existing file is never replaced: when today's anchor exists the call
/// proves it still extends into the log and reports it.
pub fn anchor(request: &AnchorRequest) -> Result<Value> {
    let (log, key, anchors) = (&request.log, &request.key, &request.anchors);
    anchor_paths(log, Some(key.as_path()), &request.public_key, anchors)?;
    let lock = DirectoryLock::acquire(anchors, LOCK_TIMEOUT)?;
    clear_staging(anchors)?;
    let ts = utc(request.now);
    let day = ts[..10].to_string();
    let existing = list_anchors(anchors)?;
    if let Some(later) = existing.iter().rev().find(|item| item.day > day) {
        return Err(refused(format!(
            "anchor {} is for a day after {day}; check the clock",
            later.path.display()
        )));
    }
    let today = existing.iter().find(|item| item.day == day);
    let prior = today.or(existing.last());
    let staging = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(anchors)
        .map_err(|error| io_refused("create a staging directory in", anchors, &error))?;
    let result = publish(request, staging.path(), &ts, &day, today, prior);
    let _ = staging.close();
    let synced = fsync_dir(anchors);
    drop(lock);
    let value = result?;
    synced?;
    Ok(value)
}

fn publish(
    request: &AnchorRequest,
    staging: &Path,
    ts: &str,
    day: &str,
    today: Option<&Anchor>,
    prior: Option<&Anchor>,
) -> Result<Value> {
    let (log, anchors) = (&request.log, &request.anchors);
    let candidate = staging.join("head.json");
    let report = ledger::sign_head(&ledger::HeadRequest {
        log: log.clone(),
        key: request.key.clone(),
        out: candidate.clone(),
        ts: Some(ts.to_string()),
        tiles: None,
        origin: None,
    })
    .map_err(|error| pending(format!("`ledger head` failed: {error:#}")))?;
    let signed_bytes = read_regular(&candidate, "signed head")?;
    let signed = parse_head(&signed_bytes, "the signed head")?;
    // An empty log has no record to derive a checkpoint origin from and is
    // signed alone; any other head comes with a checkpoint of the same tree.
    let checkpoint_agrees = match &report.checkpoint {
        None => signed.size == 0,
        Some(checkpoint) => checkpoint.size == signed.size as u64 && checkpoint.root == signed.root,
    };
    if report.head != signed || !checkpoint_agrees {
        return Err(pending(format!(
            "`ledger head` reported size {} root {} but wrote {} with size {} root {}",
            report.head.size,
            report.head.root,
            candidate.display(),
            signed.size,
            signed.root
        )));
    }
    if signed.ts != ts {
        return Err(pending(format!(
            "`ledger head` signed at {}, not {ts}",
            signed.ts
        )));
    }
    let data = read_regular(log, "ledger")?;
    let snapshot = committed_prefix(&data);
    let records = parse_log(snapshot, &format!("ledger {}", log.display()))?;
    let verifier = Verifier {
        public_key: &request.public_key,
    };
    let private_log = pin(staging, "ledger.jsonl", snapshot)?;
    tiles(&private_log, &records)?;
    verifier.binds(&private_log, &candidate, &signed, &records)?;
    if let Some(prior) = prior {
        let prior_path = pin(staging, "prior.json", &prior.bytes)?;
        verifier.extends(&private_log, &prior_path, &prior.head, &candidate, &signed)?;
    }
    if let Some(today) = today {
        return Ok(json!({
            "status": "exists",
            "day": day,
            "path": display(&today.path),
            "size": today.head.size,
            "root": today.head.root,
        }));
    }
    let target = anchors.join(format!("{day}.head.json"));
    std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o444))
        .map_err(|error| io_refused("set the mode of", &candidate, &error))?;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&candidate)
        .and_then(|file| file.sync_all())
        .map_err(|error| io_refused("flush", &candidate, &error))?;
    match std::fs::hard_link(&candidate, &target) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(refused(format!(
                "anchor {} appeared while this one was prepared; rerun to verify it",
                target.display()
            )))
        }
        Err(error) => return Err(io_refused("link", &target, &error)),
    }
    fsync_dir(anchors)?;
    Ok(json!({
        "status": "published",
        "day": day,
        "path": display(&target),
        "size": signed.size,
        "root": signed.root,
    }))
}

/// Prove every anchor extends the one before it and the latest binds to
/// the log.
pub fn verify_anchors(log: &Path, public_key: &Path, anchors: &Path) -> Result<Value> {
    anchor_paths(log, None, public_key, anchors)?;
    let found = list_anchors(anchors)?;
    let Some(latest) = found.last() else {
        return Ok(json!({"anchors": 0}));
    };
    let data = read_regular(log, "ledger")?;
    let snapshot = committed_prefix(&data);
    let records = parse_log(snapshot, &format!("ledger {}", log.display()))?;
    let verifier = Verifier { public_key };
    let private = private_dir("eb-ledger-anchors-")?;
    let private_log = pin(private.path(), "ledger.jsonl", snapshot)?;
    tiles(&private_log, &records)?;
    let pinned = found
        .iter()
        .enumerate()
        .map(|(number, item)| pin(private.path(), &format!("{number:06}.json"), &item.bytes))
        .collect::<Result<Vec<_>>>()?;
    for number in 1..found.len() {
        verifier.extends(
            &private_log,
            &pinned[number - 1],
            &found[number - 1].head,
            &pinned[number],
            &found[number].head,
        )?;
    }
    verifier.binds(
        &private_log,
        &pinned[found.len() - 1],
        &latest.head,
        &records,
    )?;
    Ok(json!({
        "anchors": found.len(),
        "latest": display(&latest.path),
        "size": latest.head.size,
        "root": latest.head.root,
    }))
}

// Package history projections

/// The package of a module name: the part before the first `/`.
#[must_use]
pub fn package_of(module: &str) -> &str {
    module.split('/').next().unwrap_or(module)
}

struct Effective {
    voided: HashSet<usize>,
    corrected_by: HashMap<usize, Vec<usize>>,
    pending_by: HashMap<usize, Vec<usize>>,
    problems: Vec<Value>,
}

fn problem(index: usize, field: &str, target: &Value, text: &str) -> Value {
    json!({"index": index, "field": field, "target": target, "problem": text})
}

/// Which records a later applied correction voided, and why.
///
/// A correction applies when it is stageless or `finished`, names an
/// earlier record, and is not itself voided. Walking from the newest record
/// settles corrections of corrections.
fn effective(records: &[Entry]) -> Effective {
    let by_id = records
        .iter()
        .map(|entry| (entry.id.as_str(), entry.index))
        .collect::<HashMap<_, _>>();
    let mut result = Effective {
        voided: HashSet::new(),
        corrected_by: HashMap::new(),
        pending_by: HashMap::new(),
        problems: Vec::new(),
    };
    for entry in records {
        for field in LINKS {
            if let Some(target) = entry.record.get(*field) {
                if !target.as_str().is_some_and(|id| by_id.contains_key(id)) {
                    result.problems.push(problem(
                        entry.index,
                        field,
                        target,
                        "names no record in this log",
                    ));
                }
            }
        }
    }
    for entry in records.iter().rev() {
        if entry.record["action"] != "correction" {
            continue;
        }
        let corrects = &entry.record["corrects"];
        let Some(&target) = corrects.as_str().and_then(|id| by_id.get(id)) else {
            continue;
        };
        if target >= entry.index {
            result.problems.push(problem(
                entry.index,
                "corrects",
                corrects,
                "names a record that is not earlier",
            ));
            continue;
        }
        let stage = entry.record.get("stage").and_then(Value::as_str);
        if result.voided.contains(&entry.index) || stage == Some("aborted") {
            continue;
        }
        if stage == Some("started") {
            result
                .pending_by
                .entry(target)
                .or_default()
                .push(entry.index);
            continue;
        }
        result.voided.insert(target);
        result
            .corrected_by
            .entry(target)
            .or_default()
            .push(entry.index);
    }
    result
}

fn state_of(action: &str) -> Option<&'static str> {
    Some(match action {
        "install" | "rebuild-inplace" | "rebuild-bump" | "unretire" => "installed",
        "retire" => "retired",
        "remove" => "removed",
        "move" => "moved",
        _ => return None,
    })
}

/// The deterministic history model of a verified log, grouped by package.
#[must_use]
pub fn build_history(verified: &VerifiedLog) -> Value {
    let records = &verified.records;
    let effective = effective(records);
    let by_id = records
        .iter()
        .map(|entry| (entry.id.as_str(), entry.index))
        .collect::<HashMap<_, _>>();
    let mut packages: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for entry in records {
        let record = &entry.record;
        let index = entry.index;
        let empty = Map::new();
        let build = record
            .get("build")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let observed = build.get("kind").and_then(Value::as_str) == Some(BACKFILL_KIND);
        let get =
            |map: &Map<String, Value>, key: &str| map.get(key).cloned().unwrap_or(Value::Null);
        let mut links = Map::new();
        for field in LINKS {
            if let Some(target) = record.get(*field) {
                let linked = target
                    .as_str()
                    .and_then(|id| by_id.get(id))
                    .map_or(Value::Null, |index| Value::from(*index));
                links.insert((*field).to_string(), json!({"id": target, "index": linked}));
            }
        }
        let (provenance, manifest_id) = if observed {
            (get(build, "provenance"), get(build, "manifest_id"))
        } else {
            (Value::Null, Value::Null)
        };
        let status = if effective.voided.contains(&index) {
            "corrected"
        } else {
            "effective"
        };
        let event = json!({
            "index": index,
            "id": entry.id,
            "ts": record["ts"],
            "action": record["action"],
            "stage": get(record, "stage"),
            "actor": record["actor"],
            "target": record["target"],
            "module": record["module"],
            "location": record["location"],
            "event_id": get(build, "event_id"),
            "reason": get(record, "reason"),
            "modulefile": get(record, "modulefile"),
            "links": links,
            "observed": observed,
            "provenance": provenance,
            "manifest_id": manifest_id,
            "status": status,
            "corrected_by": effective.corrected_by.get(&index).cloned().unwrap_or_default(),
            "correction_pending_by": effective.pending_by.get(&index).cloned().unwrap_or_default(),
        });
        packages
            .entry(package_of(entry.module()).to_string())
            .or_default()
            .push(event);
    }
    let mut model = Vec::new();
    for (name, events) in packages {
        let mut modules: BTreeMap<String, Value> = BTreeMap::new();
        for event in &events {
            let module = event["module"].as_str().unwrap_or_default().to_string();
            let state = modules.entry(module.clone()).or_insert_with(|| {
                json!({
                    "module": module, "state": "unknown", "location": null,
                    "last_change": null, "access": [], "findings": [],
                })
            });
            if event["status"] != "effective"
                || !(event["stage"].is_null() || event["stage"] == "finished")
            {
                continue;
            }
            let action = event["action"].as_str().unwrap_or_default();
            let index = event["index"].clone();
            if action == "access" {
                state["access"]
                    .as_array_mut()
                    .expect("access list")
                    .push(index);
            } else if action == "finding" {
                state["findings"]
                    .as_array_mut()
                    .expect("finding list")
                    .push(index.clone());
                if state["state"] == "unknown" {
                    state["state"] = Value::from("observed");
                    state["location"] = event["location"].clone();
                    state["last_change"] = index;
                }
            } else if let Some(next) = state_of(action) {
                state["state"] = Value::from(next);
                state["location"] = event["location"].clone();
                state["last_change"] = index;
            }
        }
        let package_problems = effective
            .problems
            .iter()
            .filter(|problem| {
                problem["index"]
                    .as_u64()
                    .and_then(|index| records.get(usize::try_from(index).ok()?))
                    .is_some_and(|entry| package_of(entry.module()) == name)
            })
            .cloned()
            .collect::<Vec<_>>();
        model.push(json!({
            "name": name,
            "modules": modules.into_values().collect::<Vec<_>>(),
            "events": events,
            "problems": package_problems,
        }));
    }
    let head = &verified.head;
    json!({
        "schema": HISTORY_SCHEMA,
        "head": {"size": head.size, "root": head.root, "ts": head.ts, "public_key": head.public_key},
        "stack": records.first().map_or(Value::Null, |entry| entry.record["stack"].clone()),
        "packages": model,
        "problems": effective.problems,
    })
}

/// `text` percent-encoded as URL quoting does: letters, digits, `_.-~` and
/// the characters in `safe` are kept, every other UTF-8 byte is `%XX`.
fn quote(text: &str, safe: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        let ch = char::from(byte);
        if byte.is_ascii_alphanumeric()
            || "_.-~".contains(ch)
            || (byte.is_ascii() && safe.contains(ch))
        {
            out.push(ch);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// A file name for a package that cannot leave the output directory.
#[must_use]
pub fn file_stem(name: &str) -> String {
    let stem = quote(name, "");
    if stem.is_empty() || stem == "." || stem == ".." || stem.len() > 150 {
        let keep = stem.len().min(100);
        return format!("{}-{}", &stem[..keep], &sha256_hex(name.as_bytes())[..16]);
    }
    stem
}

fn md_text(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ if is_control(ch) => {
                let _ = write!(out, "U+{:04X}", u32::from(ch));
            }
            '\\' | '`' | '*' | '_' | '[' | ']' | '(' | ')' | '|' | '~' | '!' | '#' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Text safe in a Markdown table cell: no markup, HTML, line breaks or
/// bidirectional controls. A value that is not a string is its canonical
/// JSON; `null` is empty.
#[must_use]
pub fn md(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => md_text(text),
        other => md_text(&ledger::canonical(other).unwrap_or_else(|_| other.to_string())),
    }
}

fn index_text(value: &Value) -> String {
    value
        .as_u64()
        .map_or_else(|| value.to_string(), |index| index.to_string())
}

fn details(event: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(links) = event["links"].as_object() {
        for (field, link) in links {
            let where_ = if link["index"].is_null() {
                format!("unknown record {}", link["id"].as_str().unwrap_or_default())
            } else {
                format!("#{}", index_text(&link["index"]))
            };
            parts.push(format!("{field} {where_}"));
        }
    }
    let indices = |key: &str| {
        event[key]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|index| format!("#{}", index_text(index)))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let corrected = indices("corrected_by");
    if !corrected.is_empty() {
        parts.push(format!("corrected by {}", corrected.join(", ")));
    }
    let started = indices("correction_pending_by");
    if !started.is_empty() {
        parts.push(format!("correction started by {}", started.join(", ")));
    }
    if event["observed"] == true {
        let refs = event["provenance"]
            .as_array()
            .map(|refs| {
                refs.iter()
                    .map(|reference| {
                        let mut text = reference["ref"].as_str().unwrap_or_default().to_string();
                        if let Some(sha) = reference.get("sha256").and_then(Value::as_str) {
                            text.push_str(&format!(" sha256 {sha}"));
                        }
                        text
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default();
        parts.push(format!(
            "observed by manifest {}; provenance {refs}",
            event["manifest_id"].as_str().unwrap_or_default()
        ));
    }
    match &event["reason"] {
        Value::Null => {}
        Value::String(reason) => parts.push(format!("reason {reason}")),
        other => parts.push(format!(
            "reason {}",
            ledger::canonical(other).unwrap_or_else(|_| other.to_string())
        )),
    }
    md_text(&parts.join("; "))
}

fn head_lines(model: &Value) -> Vec<String> {
    let head = &model["head"];
    vec![
        format!("Stack: {}", md(&model["stack"])),
        String::new(),
        format!(
            "Verified head: size {}, root {}, signed {} by {}.",
            index_text(&head["size"]),
            head["root"].as_str().unwrap_or_default(),
            head["ts"].as_str().unwrap_or_default(),
            head["public_key"].as_str().unwrap_or_default()
        ),
        String::new(),
    ]
}

fn plain(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// The Markdown history page of one package of the model [`build_history`]
/// makes.
#[must_use]
pub fn render_package(model: &Value, package: &Value) -> String {
    let mut lines = vec![
        format!("# Package history: {}", md(&package["name"])),
        String::new(),
    ];
    lines.extend(head_lines(model));
    lines.extend(
        [
            "## Modules",
            "",
            "| Module | State | Last change | Location |",
            "| --- | --- | --- | --- |",
        ]
        .map(String::from),
    );
    for module in package["modules"].as_array().into_iter().flatten() {
        let change = if module["last_change"].is_null() {
            String::new()
        } else {
            format!("#{}", index_text(&module["last_change"]))
        };
        lines.push(format!(
            "| {} | {} | {change} | {} |",
            md(&module["module"]),
            plain(&module["state"]),
            md(&module["location"])
        ));
    }
    lines.extend(
        [
            "",
            "## Events",
            "",
            "| # | Time (UTC) | Action | Stage | Module | Actor | Status | Details |",
            "| --- | --- | --- | --- | --- | --- | --- | --- |",
        ]
        .map(String::from),
    );
    for event in package["events"].as_array().into_iter().flatten() {
        lines.push(format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            index_text(&event["index"]),
            md(&event["ts"]),
            plain(&event["action"]),
            plain(&event["stage"]),
            md(&event["module"]),
            md(&event["actor"]),
            plain(&event["status"]),
            details(event)
        ));
    }
    let problems = package["problems"].as_array().cloned().unwrap_or_default();
    if !problems.is_empty() {
        lines.extend(["", "## Problems", ""].map(String::from));
        for problem in problems {
            lines.push(format!(
                "* record #{} {} {}: {}",
                index_text(&problem["index"]),
                plain(&problem["field"]),
                md(&problem["target"]),
                plain(&problem["problem"])
            ));
        }
    }
    lines.join("\n") + "\n"
}

fn json_bytes(value: &Value) -> Vec<u8> {
    (dumps(value, Some(2)) + "\n").into_bytes()
}

/// Every output file, by path relative to the output directory.
pub fn render(model: &Value) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    let mut index = Vec::new();
    let mut stems = HashSet::new();
    for package in model["packages"].as_array().into_iter().flatten() {
        let name = plain(&package["name"]);
        let stem = file_stem(name);
        if !stems.insert(stem.clone()) {
            return Err(refused(format!("two packages map to the file name {stem}")));
        }
        let document = json!({
            "schema": model["schema"], "head": model["head"], "stack": model["stack"],
            "package": name, "modules": package["modules"], "events": package["events"],
            "problems": package["problems"],
        });
        files.insert(format!("packages/{stem}.json"), json_bytes(&document));
        files.insert(
            format!("packages/{stem}.md"),
            render_package(model, package).into_bytes(),
        );
        let count = |key: &str| package[key].as_array().map_or(0, Vec::len);
        index.push(json!({
            "name": name, "json": format!("packages/{stem}.json"),
            "markdown": format!("packages/{stem}.md"),
            "modules": count("modules"), "events": count("events"),
        }));
    }
    let mut lines = vec!["# Ledger package history".to_string(), String::new()];
    lines.extend(head_lines(model));
    lines.push("| Package | Modules | Events |".to_string());
    lines.push("| --- | --- | --- |".to_string());
    for entry in &index {
        lines.push(format!(
            "| [{}]({}) | {} | {} |",
            md(&entry["name"]),
            quote(plain(&entry["markdown"]), "/"),
            entry["modules"],
            entry["events"]
        ));
    }
    files.insert(
        "index.json".to_string(),
        json_bytes(&json!({
            "schema": model["schema"], "head": model["head"], "stack": model["stack"],
            "packages": index, "problems": model["problems"],
        })),
    );
    files.insert(
        "index.md".to_string(),
        (lines.join("\n") + "\n").into_bytes(),
    );
    Ok(files)
}

fn list_names(directory: &Path) -> Result<BTreeSet<String>> {
    Ok(std::fs::read_dir(directory)
        .map_err(|error| io_refused("list", directory, &error))?
        .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
        .collect())
}

fn output_dir(out: &Path, log: &Path, head: &Path, public_key: &Path) -> Result<()> {
    plain_path(out, "output directory")?;
    for (what, path) in [("ledger", log), ("head", head), ("public key", public_key)] {
        if inside(path, out) || out == dirname(path) {
            return Err(refused(format!(
                "output directory {} must not hold the {what} {}",
                out.display(),
                path.display()
            )));
        }
    }
    if !lexists(out) {
        return Ok(());
    }
    existing_dir(out, "output directory")?;
    let names = list_names(out)?;
    let allowed = [
        "index.json",
        "index.md",
        ".index.json.new",
        ".index.md.new",
        "packages",
    ];
    let mut foreign = names
        .iter()
        .filter(|name| !allowed.contains(&name.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>();
    if names.contains("packages") {
        let packages = out.join("packages");
        if !std::fs::symlink_metadata(&packages).is_ok_and(|info| info.file_type().is_dir()) {
            return Err(refused(format!(
                "{} must be a directory",
                packages.display()
            )));
        }
        foreign.extend(list_names(&packages)?.into_iter().filter(|name| {
            ![".json", ".md", ".json.new", ".md.new"]
                .iter()
                .any(|suffix| name.ends_with(suffix))
        }));
    }
    if !foreign.is_empty() {
        return Err(refused(format!(
            "output directory {} holds files that are not a ledger history projection: {}",
            out.display(),
            foreign.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    if names.contains("index.json") {
        let data = read_regular(&out.join("index.json"), "projection index")?;
        let schema = serde_json::from_slice::<Value>(&data)
            .ok()
            .and_then(|index| {
                index
                    .get("schema")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        if schema.as_deref() != Some(HISTORY_SCHEMA) {
            return Err(refused(format!(
                "output directory {} holds an index that is not a ledger history projection",
                out.display()
            )));
        }
    }
    Ok(())
}

fn identity(info: &std::fs::Metadata) -> (u64, u64, u32, u32, u32, i64, i64) {
    (
        info.dev(),
        info.ino(),
        info.uid(),
        info.gid(),
        info.mode(),
        info.ctime(),
        info.ctime_nsec(),
    )
}

/// Replace `path` with `data` only once the bytes are durable. An existing
/// file keeps its owner and mode; a new one gets `0644` less the umask. The
/// bytes go to `.<name>.new` beside it first, and a stale one of those is
/// removed when it is a regular file of this user or the file's owner.
fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let parent = dirname(path);
    std::fs::create_dir_all(parent).map_err(|error| io_refused("create", parent, &error))?;
    let existing = match std::fs::symlink_metadata(path) {
        Ok(info) if info.file_type().is_symlink() => {
            return Err(refused(format!(
                "refusing to replace a symlink: {}",
                path.display()
            )))
        }
        Ok(info) if !info.file_type().is_file() => {
            return Err(refused(format!(
                "cannot read access metadata for {}: not a regular file",
                path.display()
            )))
        }
        Ok(info) => Some(info),
        Err(_) => None,
    };
    // mode_t is u32 on Linux and u16 on macOS.
    #[allow(clippy::useless_conversion)]
    let mode = match &existing {
        Some(info) => info.mode() & 0o7777,
        None => {
            // umask can only be read by setting it; it is restored at once.
            let mask = unsafe { libc::umask(0) };
            unsafe { libc::umask(mask) };
            0o644 & !u32::from(mask)
        }
    };
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temporary = parent.join(format!(".{name}.new"));
    if let Ok(stale) = std::fs::symlink_metadata(&temporary) {
        // geteuid has no preconditions.
        let euid = unsafe { libc::geteuid() };
        let owned = stale.uid() == euid
            || existing
                .as_ref()
                .is_some_and(|info| info.uid() == stale.uid());
        if !stale.file_type().is_file() || !owned || stale.nlink() != 1 {
            return Err(refused(format!(
                "temporary file must be an owned regular file with one link: {}",
                temporary.display()
            )));
        }
        std::fs::remove_file(&temporary)
            .map_err(|error| io_refused("remove", &temporary, &error))?;
    }
    let written = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)
            .map_err(|error| io_refused("create", &temporary, &error))?;
        file.write_all(data)
            .map_err(|error| io_refused("write", &temporary, &error))?;
        if let Some(info) = &existing {
            // The descriptor is open for the duration of the call.
            if unsafe { libc::fchown(file.as_raw_fd(), info.uid(), info.gid()) } != 0 {
                return Err(refused(format!(
                    "cannot preserve access metadata for {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                )));
            }
        }
        file.set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|error| io_refused("set the mode of", &temporary, &error))?;
        file.sync_all()
            .map_err(|error| io_refused("flush", &temporary, &error))?;
        drop(file);
        if let Some(info) = &existing {
            let current = std::fs::symlink_metadata(path)
                .map_err(|error| io_refused("recheck", path, &error))?;
            if identity(&current) != identity(info) {
                return Err(refused(format!(
                    "access metadata of {} changed before it was replaced",
                    path.display()
                )));
            }
        }
        std::fs::rename(&temporary, path).map_err(|error| io_refused("replace", path, &error))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written?;
    fsync_dir(parent)
}

/// What `ledger project` needs.
#[derive(Debug, Clone)]
pub struct ProjectRequest {
    /// The JSON Lines log.
    pub log: PathBuf,
    /// A signed head that covers every record.
    pub head: PathBuf,
    /// The public key the head must verify under.
    pub public_key: PathBuf,
    /// The output directory, outside the log's, the head's and the key's.
    pub out: PathBuf,
}

/// Write JSON and Markdown history of every package in a verified log.
///
/// The same verified log and head always give the same bytes. Package files
/// are replaced one at a time and the index last, so an interrupted run
/// leaves the previous index; files of packages no longer listed are
/// removed.
pub fn project(request: &ProjectRequest) -> Result<Value> {
    let out = &request.out;
    output_dir(out, &request.log, &request.head, &request.public_key)?;
    let verified = read_verified(&request.log, &request.head, &request.public_key)?;
    let files = render(&build_history(&verified))?;
    let packages = out.join("packages");
    std::fs::create_dir_all(&packages).map_err(|error| io_refused("create", &packages, &error))?;
    if !std::fs::symlink_metadata(&packages).is_ok_and(|info| info.file_type().is_dir()) {
        return Err(refused(format!(
            "{} must be a directory",
            packages.display()
        )));
    }
    for (relative, data) in &files {
        if relative.starts_with("packages/") {
            atomic_write(&out.join(relative), data)?;
        }
    }
    let keep = files
        .keys()
        .filter_map(|relative| relative.strip_prefix("packages/"))
        .collect::<HashSet<_>>();
    for name in list_names(&packages)? {
        if !keep.contains(name.as_str()) && (name.ends_with(".json") || name.ends_with(".md")) {
            let path = packages.join(&name);
            std::fs::remove_file(&path).map_err(|error| io_refused("remove", &path, &error))?;
        }
    }
    fsync_dir(&packages)?;
    for relative in ["index.md", "index.json"] {
        atomic_write(&out.join(relative), &files[relative])?;
    }
    Ok(json!({
        "out": display(out),
        "size": verified.head.size,
        "root": verified.head.root,
        "packages": keep.len() / 2,
        "files": files.keys().collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dumps_matches_sorted_ascii_json() {
        let value = json!({"b": [1, {"y": "é\n", "x": []}], "a": null, "c": {}});
        assert_eq!(
            dumps(&value, None),
            r#"{"a": null, "b": [1, {"x": [], "y": "é\n"}], "c": {}}"#
        );
        assert_eq!(
            dumps(&value, Some(2)),
            "{\n  \"a\": null,\n  \"b\": [\n    1,\n    {\n      \"x\": [],\n      \"y\": \"\\u00e9\\n\"\n    }\n  ],\n  \"c\": {}\n}"
        );
        assert_eq!(dumps(&json!("\u{1f600}\u{7f}"), None), r#""😀\u007f""#);
    }

    #[test]
    fn normpath_follows_posix() {
        for (path, expected) in [
            ("/a/b", "/a/b"),
            ("/a/b/", "/a/b"),
            ("/a/./b", "/a/b"),
            ("/a/../b", "/b"),
            ("//a", "//a"),
            ("///a", "/a"),
            ("/", "/"),
            ("/..", "/"),
            ("a/../..", ".."),
        ] {
            assert_eq!(normpath(path), expected, "{path}");
        }
    }

    #[test]
    fn strict_json_refuses_repeated_keys_and_floats() {
        assert!(strict_json(r#"{"a":1,"a":1}"#, "x").is_err());
        assert!(strict_json(r#"{"a":1.0}"#, "x").is_err());
        assert!(strict_json(r#"{"a":[1,{"b":null}]}"#, "x").is_ok());
    }

    #[test]
    fn quote_keeps_unreserved_characters() {
        assert_eq!(quote("evil|pkg", ""), "evil%7Cpkg");
        assert_eq!(
            quote("packages/evil%7Cpkg.md", "/"),
            "packages/evil%257Cpkg.md"
        );
        assert_eq!(quote("café~", ""), "caf%C3%A9~");
    }
}
