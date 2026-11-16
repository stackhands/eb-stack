//! eOn 2.17.10 core + rgpot recipes under fixtures/eon_core_rgpot.
//!
//! Frozen snapshot of the upstream draft PR set (easybuild-easyconfigs
//! #26480): eOn plus CapnProto, quill, readcon-core, and rgpot companions,
//! all on the single foss-2026.1 / GCCcore-15.2.0 generation. nanobind is
//! not here: develop already ships it at this generation, so the PR must
//! not re-add it.
//! This is the shape maintainers accept: conventional parameters, no staging
//! scripts, no cross-generation dependencies, and fat companions (rgpot
//! carries every optional feature whose dependencies exist in the
//! generation; casparvl review on #26480).

use eb_stack::{
    check_recipe_deps, packaging_gate, parse_easyconfig_trees, resolve_easyconfig_file,
};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/eon_core_rgpot")
}

fn drafts() -> PathBuf {
    root().join("easyconfigs")
}

#[test]
fn resolve_eon_core_rgpot_recipe() {
    let p = drafts().join("e/eOn/eOn-2.17.10-foss-2026.1.eb");
    let r = resolve_easyconfig_file(&p).expect("resolve eOn core+rgpot");
    assert_eq!(r.name, "eOn");
    assert_eq!(r.version, "2.17.10");
    assert_eq!(r.toolchain.name, "foss");
    assert_eq!(r.toolchain.version, "2026.1");
    assert_eq!(r.easyblock.as_deref(), Some("MesonNinja"));
    assert_eq!(r.moduleclass.as_deref(), Some("chem"));

    let opts = r.configopts.as_deref().unwrap_or("");
    assert!(opts.contains("-Dwith_rgpot=true"), "configopts: {opts}");
    for fat in ["-Dwith_metatomic", "-Dwith_xtb", "-Dwith_serve"] {
        assert!(!opts.contains(fat), "core product must not set {fat}");
    }
    packaging_gate(&r, &["-Dwith_rgpot=true"]).expect("packaging_gate");

    let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
    for need in [
        "Python",
        "SciPy-bundle",
        "PyYAML",
        "Eigen",
        "Highway",
        "inih",
        "nlohmann_json",
        "quill",
        "readcon-core",
        "rgpot",
    ] {
        assert!(names.contains(&need), "missing dep {need} in {names:?}");
    }

    // On foss apps, do not hard-code GCCcore dep toolchains (casparvl
    // #26480). Robot walks foss subtoolchains and finds GCCcore modules.
    for dep in &r.dependencies {
        assert!(
            dep.toolchain.is_none(),
            "do not hard-code dep toolchain on foss app: {} {:?}",
            dep.name,
            dep.toolchain
        );
    }
}

#[test]
fn readcon_core_no_patchelf_uses_check_readelf_rpath() {
    let text = std::fs::read_to_string(
        drafts().join("r/readcon-core/readcon-core-0.13.1-GCCcore-15.2.0.eb"),
    )
    .unwrap();
    assert!(
        !text.contains("('patchelf'") && !text.contains("patchelf --"),
        "cargo-c install must not depend on or invoke patchelf"
    );
    assert!(
        text.contains("check_readelf_rpath = False"),
        "cargo-c skips EB RPATH wrappers; disable readelf RPATH presence check"
    );
    assert!(
        !text.contains("postinstallcmds"),
        "no postinstall RPATH rewrite"
    );
}

#[test]
fn rgpot_companion_builds_fat() {
    // casparvl on #26480: install packages as fat as possible; features stay
    // off only when their dependencies are missing from the generation.
    let r = resolve_easyconfig_file(&drafts().join("r/rgpot/rgpot-2.5.3-GCCcore-15.2.0.eb"))
        .expect("resolve fat rgpot");
    let opts = r.configopts.as_deref().unwrap_or("");
    for on in [
        "-Dpure_lib=false",
        "-Dwith_rpc=true",
        "-Dwith_eigen=true",
        "-Dwith_python=true",
        "-Dwith_tests=true",
        "-Dwith_examples=true",
    ] {
        assert!(opts.contains(on), "fat rgpot must set {on}: {opts}");
    }
    assert!(
        !opts.contains("-Dwith_rpc_client_only=true"),
        "client-only is the thin shape casparvl flagged: {opts}"
    );

    let deps: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
    for need in ["CapnProto", "Eigen", "fmt", "Python"] {
        assert!(deps.contains(&need), "fat rgpot needs dep {need}: {deps:?}");
    }
    let build_deps: Vec<_> = r
        .builddependencies
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    for need in ["nanobind", "Catch2"] {
        assert!(
            build_deps.contains(&need),
            "fat rgpot needs builddep {need}: {build_deps:?}"
        );
    }

    let text =
        std::fs::read_to_string(drafts().join("r/rgpot/rgpot-2.5.3-GCCcore-15.2.0.eb")).unwrap();
    for surface in ["bin/potserv", "librgpot", "site-packages/rgpot", "runtest"] {
        assert!(text.contains(surface), "fat rgpot sanity covers {surface}");
    }
    // Off-flags must each carry a why in the comment block above configopts.
    for justified_off in [
        "with_xtb",
        "with_tblite",
        "with_metatomic",
        "with_xtensor",
        "with_cache",
    ] {
        assert!(
            text.contains(justified_off),
            "off feature {justified_off} needs an in-recipe justification"
        );
    }
}

#[test]
fn eon_core_recipe_stays_conventional() {
    let text = std::fs::read_to_string(drafts().join("e/eOn/eOn-2.17.10-foss-2026.1.eb")).unwrap();
    for banned in [
        "preconfigopts",
        "postinstallcmds",
        "readcon-stage",
        "patchelf",
        "EBROOTPYTORCH",
        "patches =",
    ] {
        assert!(
            !text.contains(banned),
            "core eOn recipe must not need {banned}"
        );
    }
}

#[test]
fn resolve_core_rgpot_companions() {
    let cases = [
        (
            "r/rgpot/rgpot-2.5.3-GCCcore-15.2.0.eb",
            "rgpot",
            "2.5.3",
            "GCCcore",
            "15.2.0",
        ),
        (
            "r/readcon-core/readcon-core-0.13.1-GCCcore-15.2.0.eb",
            "readcon-core",
            "0.13.1",
            "GCCcore",
            "15.2.0",
        ),
        (
            "c/CapnProto/CapnProto-1.4.0-GCCcore-15.2.0.eb",
            "CapnProto",
            "1.4.0",
            "GCCcore",
            "15.2.0",
        ),
        (
            "q/quill/quill-11.1.0-GCCcore-15.2.0.eb",
            "quill",
            "11.1.0",
            "GCCcore",
            "15.2.0",
        ),
    ];
    for (rel, name, ver, tc_name, tc_ver) in cases {
        let r = resolve_easyconfig_file(&drafts().join(rel))
            .unwrap_or_else(|e| panic!("resolve {rel}: {e}"));
        assert_eq!(r.name, name, "{rel}");
        assert_eq!(r.version, ver, "{rel}");
        assert_eq!(r.toolchain.name, tc_name, "{rel}");
        assert_eq!(r.toolchain.version, tc_ver, "{rel}");
    }
}

#[test]
fn pr_fixture_does_not_re_add_what_the_robot_tree_already_has() {
    // Do/don't #8: never re-add a package develop already ships at the target
    // generation. nanobind-2.13.0-GCCcore-15.2.0 was written from scratch and
    // pushed over develop's copy before this guard existed.
    let Some(robot) = common::require_easyconfigs_tree("eon_core_rgpot::duplicate_upstream_guard")
    else {
        return;
    };
    let mut upstream = std::collections::HashSet::new();
    for entry in walk_eb_files(&robot) {
        if let Some(name) = entry.file_name().and_then(|n| n.to_str()) {
            upstream.insert(name.to_string());
        }
    }
    let mut duplicates = Vec::new();
    for entry in walk_eb_files(&drafts()) {
        if let Some(name) = entry.file_name().and_then(|n| n.to_str()) {
            if upstream.contains(name) {
                duplicates.push(name.to_string());
            }
        }
    }
    assert!(
        duplicates.is_empty(),
        "PR fixture re-adds easyconfigs the robot tree already has: {duplicates:?}"
    );
}

fn walk_eb_files(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("eb") {
                out.push(path);
            }
        }
    }
    out
}

#[test]
fn eon_core_check_recipe_drafts_plus_robot() {
    let recipe =
        resolve_easyconfig_file(&drafts().join("e/eOn/eOn-2.17.10-foss-2026.1.eb")).unwrap();
    let drafts_root = drafts();
    let draft_tree = parse_easyconfig_trees(&[drafts_root.as_path()]).unwrap();
    let incomplete = check_recipe_deps(&recipe, &draft_tree.candidates);
    assert!(
        !incomplete.ok(),
        "drafts alone cannot supply Python/foss stack"
    );
    for companion in ["rgpot", "readcon-core", "quill"] {
        assert!(
            incomplete.found.iter().any(|f| f.contains(companion)),
            "drafts must supply {companion}: found={:?}",
            incomplete.found
        );
    }

    let Some(real) = common::require_easyconfigs_tree("eon_core_rgpot::full_robot_overlay") else {
        return;
    };
    let drafts_root2 = drafts();
    let merged =
        parse_easyconfig_trees(&[real.as_path(), drafts_root2.as_path()]).expect("overlay");
    let check = check_recipe_deps(&recipe, &merged.candidates);
    eprintln!(
        "eOn core+rgpot robot check: found={} missing={:?} coverage={:.2}%",
        check.found.len(),
        check
            .missing
            .iter()
            .map(|m| format!("{}-{}", m.name, m.version))
            .collect::<Vec<_>>(),
        100.0 * merged.coverage()
    );
    assert!(
        check.ok(),
        "eOn core fixture must resolve with overlay and robot: missing={:?}",
        check.missing
    );
    assert!(check.found.iter().any(|f| f.contains("rgpot")));
    assert!(check.found.iter().any(|f| f.contains("readcon-core")));
}

#[test]
fn eon_fat_2024a_and_core_2026_1_fixtures_coexist() {
    // Generation split stays explicit: the 2024a site-parity surface keeps
    // the full product, the 2026.1 tree carries the core + rgpot product.
    let a24 = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/eon_packaging/easyconfigs/e/eOn/eOn-2.16.0-foss-2024a.eb");
    let a26 = drafts().join("e/eOn/eOn-2.17.10-foss-2026.1.eb");
    assert!(a24.is_file(), "2024a site-parity fixture missing");
    assert!(a26.is_file(), "2026.1 core fixture missing");
    let r24 = resolve_easyconfig_file(&a24).unwrap();
    let r26 = resolve_easyconfig_file(&a26).unwrap();
    assert_eq!(r24.toolchain.version, "2024a");
    assert_eq!(r26.toolchain.version, "2026.1");
}

#[test]
fn eon_core_recipe_copies_do_not_drift() {
    // The same recipe seeds the tutorial overlay tree, and the catalog
    // companions under examples/packages/companions close robot holes in
    // package plans. Every copy must stay identical to the draft-PR
    // snapshot.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let pairs = [
        (
            "fixtures/eon_core_rgpot/easyconfigs/e/eOn/eOn-2.17.10-foss-2026.1.eb",
            "fixtures/eon_foss_2026_1/easyconfigs/e/eOn/eOn-2.17.10-foss-2026.1.eb",
        ),
        (
            "fixtures/eon_core_rgpot/easyconfigs/r/readcon-core/readcon-core-0.13.1-GCCcore-15.2.0.eb",
            "examples/packages/companions/r/readcon-core/readcon-core-0.13.1-GCCcore-15.2.0.eb",
        ),
        (
            "fixtures/eon_core_rgpot/easyconfigs/r/rgpot/rgpot-2.5.3-GCCcore-15.2.0.eb",
            "examples/packages/companions/r/rgpot/rgpot-2.5.3-GCCcore-15.2.0.eb",
        ),
    ];
    for (canonical_rel, copy_rel) in pairs {
        let canonical = std::fs::read_to_string(manifest.join(canonical_rel)).unwrap();
        let copy = std::fs::read_to_string(manifest.join(copy_rel)).unwrap();
        assert_eq!(canonical, copy, "{copy_rel} drifted from {canonical_rel}");
    }
}

const SMOKE_STEP: &str = "recipe packaging gate on eOn core fixture";

/// The `run: |` script of the ci_test.yml step named `name`, dedented.
fn ci_step(name: &str) -> String {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ci = std::fs::read_to_string(manifest.join(".github/workflows/ci_test.yml"))
        .expect("read ci_test.yml");
    let header = format!("- name: {name}");
    let mut lines = ci
        .lines()
        .skip_while(|line| line.trim() != header)
        .skip(1)
        .peekable();
    assert_eq!(
        lines.next().map(str::trim),
        Some("run: |"),
        "ci_test.yml has no step {name:?} with a run block"
    );
    let indent = lines
        .peek()
        .map(|line| line.len() - line.trim_start().len())
        .expect("the run block has a body");
    lines
        .take_while(|line| line.trim().is_empty() || line.len() - line.trim_start().len() >= indent)
        .map(|line| format!("{}\n", line.get(indent..).unwrap_or("")))
        .collect()
}

/// Run a CI step's script as the runner's bash does, with `BIN` set to `bin`
/// instead of the release build.
fn run_step(script: &str, bin: &Path) -> std::process::Output {
    let release = "BIN=./target/release/eb-stack\n";
    assert!(script.contains(release), "the step sets {release:?}");
    let quoted = bin.display().to_string().replace('\'', r"'\''");
    Command::new("bash")
        .arg("-ec")
        .arg(script.replace(release, &format!("BIN='{quoted}'\n")))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run bash")
}

/// A smoke step that expects `recipe check` to fail passes on any error, so a
/// recipe renamed under it leaves it passing on "No such file or directory".
#[test]
fn ci_smoke_step_names_paths_that_exist() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let step = ci_step(SMOKE_STEP);
    let named: Vec<_> = step
        .split_whitespace()
        .filter(|word| word.starts_with("fixtures/"))
        .collect();
    assert!(!named.is_empty(), "the step names no fixture:\n{step}");
    for path in named {
        assert!(
            manifest.join(path).exists(),
            "{SMOKE_STEP:?} names {path}, which does not exist"
        );
    }
}

/// The step expects `recipe check` to fail only because the drafts leave the
/// stack's own dependencies unresolved. Run as CI runs it, it passes with
/// eb-stack and fails when the check fails for any other reason.
#[test]
fn ci_smoke_step_accepts_only_the_unresolved_stack_dependencies() {
    let step = ci_step(SMOKE_STEP);
    let real = run_step(&step, Path::new(env!("CARGO_BIN_EXE_eb-stack")));
    assert!(
        real.status.success(),
        "the step fails with eb-stack:\n{}{}",
        String::from_utf8_lossy(&real.stdout),
        String::from_utf8_lossy(&real.stderr)
    );

    let impostors = [
        (
            "a recipe that does not exist",
            "",
            "error: --recipe fixtures/eon_core_rgpot/easyconfigs/e/eOn/eOn-2.17.2-foss-2026.1.eb: No such file or directory (os error 2)",
        ),
        (
            "a failed packaging gate",
            "",
            "error: packaging gate failed: configopts lack -Dwith_rgpot=true",
        ),
        (
            "drafts that do not resolve",
            r#"  "found": []"#,
            "error: recipe has 14 unresolved dependencies",
        ),
    ];
    let dir = tempfile::tempdir().expect("tempdir");
    let stubs: Vec<_> = impostors
        .iter()
        .enumerate()
        .map(|(i, (_, stdout, stderr))| {
            let stub = dir.path().join(format!("eb-stack-{i}"));
            std::fs::write(
                &stub,
                format!("#!/bin/sh\necho '{stdout}'\necho '{stderr}' >&2\nexit 1\n"),
            )
            .expect("write stub");
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
                .expect("chmod stub");
            stub
        })
        .collect();
    for ((why, _, _), stub) in impostors.iter().zip(&stubs) {
        assert!(
            !run_step(&step, stub).status.success(),
            "the step passes when recipe check fails on {why}"
        );
    }
}
