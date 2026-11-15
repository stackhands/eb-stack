//! scripts/eessi-extend-eb.sh against a fake EESSI repository: an init
//! that defines `module` and sets EESSI_EPREFIX, and an `eb` that prints
//! the environment it was given.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/eessi-extend-eb.sh")
}

struct Fake {
    dir: tempfile::TempDir,
}

impl Fake {
    fn new(version: &str, eprefix: Option<&str>) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let init = dir
            .path()
            .join(format!("repo/versions/{version}/init/lmod"));
        fs::create_dir_all(&init).expect("init dir");
        let mut text = String::from("module() { :; }\n");
        if let Some(e) = eprefix {
            text.push_str(&format!("export EESSI_EPREFIX={e}\n"));
        }
        fs::write(init.join("bash"), text).expect("init");
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).expect("bin");
        let eb = bin.join("eb");
        fs::write(&eb, "#!/bin/sh\nenv\n").expect("eb");
        fs::set_permissions(&eb, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::create_dir_all(dir.path().join("home")).expect("home");
        Self { dir }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn run(&self, env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new("bash");
        cmd.arg(script())
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path("bin").display()),
            )
            .env("HOME", self.path("home"))
            .env("EESSI_CVMFS_REPO", self.path("repo"));
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().expect("run script")
    }
}

fn var<'a>(out: &'a Output, name: &str) -> Option<&'a str> {
    let text = std::str::from_utf8(&out.stdout).expect("utf8");
    let prefix = format!("{name}=");
    text.lines().find_map(|l| l.strip_prefix(prefix.as_str()))
}

fn ok(out: &Output) {
    assert!(
        out.status.success(),
        "status {:?}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn without_a_version_it_loads_the_current_eessi() {
    let fake = Fake::new("2026.06", None);
    let out = fake.run(&[]);
    ok(&out);
    let arch = String::from_utf8(Command::new("uname").arg("-m").output().unwrap().stdout).unwrap();
    let compat = fake.path(&format!(
        "repo/versions/2026.06/compat/linux/{}/usr/bin",
        arch.trim()
    ));
    let path = var(&out, "PATH").expect("PATH");
    assert!(
        path.starts_with(&format!("{}:", compat.display())),
        "{path}"
    );
}

#[test]
fn the_compat_layer_comes_from_the_init() {
    let fake = Fake::new("2026.06", Some("/opt/eessi-compat"));
    let out = fake.run(&[]);
    ok(&out);
    let path = var(&out, "PATH").expect("PATH");
    assert!(path.starts_with("/opt/eessi-compat/usr/bin:"), "{path}");
}

#[test]
fn a_named_user_prefix_is_created() {
    let fake = Fake::new("2026.06", None);
    let prefix = fake.path("home/eessi-mine");
    let out = fake.run(&[("EESSI_USER_INSTALL", prefix.to_str().unwrap())]);
    ok(&out);
    assert!(Path::new(&prefix).is_dir());
    assert_eq!(var(&out, "EESSI_USER_INSTALL"), prefix.to_str());
}

#[test]
fn the_default_user_prefix_is_home_eessi() {
    let fake = Fake::new("2026.06", None);
    let out = fake.run(&[]);
    ok(&out);
    assert!(fake.path("home/EESSI").is_dir());
}

#[test]
fn a_project_prefix_is_not_created() {
    let fake = Fake::new("2026.06", None);
    let project = fake.path("project");
    let out = fake.run(&[("EESSI_PROJECT_INSTALL", project.to_str().unwrap())]);
    ok(&out);
    assert!(!project.exists());
    assert_eq!(var(&out, "EESSI_USER_INSTALL"), None);
}

#[test]
fn two_prefixes_are_refused() {
    let fake = Fake::new("2026.06", None);
    let out = fake.run(&[
        ("EESSI_USER_INSTALL", "/x"),
        ("EESSI_PROJECT_INSTALL", "/y"),
    ]);
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn a_missing_version_is_reported() {
    let fake = Fake::new("2026.06", None);
    let out = fake.run(&[("EESSI_VERSION", "2025.06")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("versions/2025.06/init"));
}
