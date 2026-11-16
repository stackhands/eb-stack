use eb_stack::target::{
    doctor_target, resolve_target_layers, BuildTarget, EasyBuildWorkload, TargetConfigLayer,
    TargetExecutor, TargetRuntime, TargetTransport, TARGET_CONFIG_SCHEMA_VERSION,
};
use std::collections::BTreeMap;
use std::process::Command;

include!("support/write_executable.rs");

#[test]
fn layered_target_config_composes_ssh_slurm_container_and_easybuild() {
    let base = TargetConfigLayer::from_toml_str(
        r#"
schema_version = 1

[[targets]]
name = "site-builder"

[targets.transport]
kind = "ssh"
host = "builder.example.org"

[targets.executor]
kind = "direct"

[targets.runtime]
kind = "host"

[targets.easybuild]
command = "eb"
robot_paths = ["/opt/easybuild/easyconfigs", "/work/overlay"]
work_root = "/work/campaign"
tmp_root = "/work/easybuild-tmp"
"#,
    )
    .expect("base target");
    let site = TargetConfigLayer::from_toml_str(
        r#"
schema_version = 1

[[targets]]
name = "site-builder"

[targets.executor]
kind = "slurm"
partition = "build"
cpus = 8
memory = "32G"
time = "02:00:00"

[targets.runtime]
kind = "podman"
image = "registry.example.org/easybuild:rocky9"
args = ["--security-opt", "label=disable", "--cap-add=NET_RAW"]
workdir = "/workspace"
mounts = ["/work:/workspace"]
"#,
    )
    .expect("site target");
    let targets = resolve_target_layers(&[base, site]).expect("resolve targets");
    assert_eq!(targets.len(), 1);
    let target = &targets[0];
    assert_eq!(target.name, "site-builder");
    assert!(matches!(target.runtime, TargetRuntime::Podman { .. }));
    assert_eq!(
        target.staged_bundle_path(std::path::Path::new("/control/eon-bundle")),
        "/work/campaign/bundles/eon-bundle"
    );

    let command = target.build_command("/work/campaign/bundles/eon-bundle/QMCPACK.eb");
    assert_eq!(command.program, "ssh");
    assert_eq!(command.args[0], "builder.example.org");
    let remote = command.args.last().expect("remote command");
    for token in [
        "srun",
        "--partition",
        "build",
        "podman",
        "registry.example.org/easybuild:rocky9",
        "EASYBUILD_TMPDIR=/work/easybuild-tmp",
        "eb",
        "--robot=/opt/easybuild/easyconfigs:/work/overlay",
        "/work/campaign/bundles/eon-bundle/QMCPACK.eb",
    ] {
        assert!(remote.contains(token), "missing {token}: {remote}");
    }
    let runtime = remote.find("podman run").expect("runtime command");
    let capability = remote.find("--cap-add=NET_RAW").expect("runtime args");
    let image = remote
        .find("registry.example.org/easybuild:rocky9")
        .expect("runtime image");
    let routed = remote
        .find("EASYBUILD_TMPDIR=/work/easybuild-tmp")
        .expect("routed command");
    assert!(runtime < capability && capability < image && image < routed);
}

#[test]
fn local_direct_host_target_doctor_is_executable_without_building() {
    let layer = TargetConfigLayer::from_toml_str(&format!(
        r#"
schema_version = {TARGET_CONFIG_SCHEMA_VERSION}

[[targets]]
name = "local-doctor"

[targets.transport]
kind = "local"

[targets.executor]
kind = "direct"

[targets.runtime]
kind = "host"

[targets.easybuild]
command = "true"
robot_paths = ["/tmp"]
work_root = "/tmp"
tmp_root = "/tmp"
"#
    ))
    .expect("local target");
    let mut targets = resolve_target_layers(&[layer]).expect("resolve");
    let target: BuildTarget = targets.pop().expect("target");
    let report = doctor_target(&target).expect("doctor execution");
    assert!(report.ok());
    assert_eq!(report.target, "local-doctor");
    assert!(report.checks.iter().all(|check| check.success));
}

#[test]
fn target_cli_lists_and_doctors_named_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = temp.path().join("targets.toml");
    std::fs::write(
        &config,
        r#"
schema_version = 1
[[targets]]
name = "local-doctor"
[targets.transport]
kind = "local"
[targets.executor]
kind = "direct"
[targets.runtime]
kind = "host"
[targets.easybuild]
command = "true"
robot_paths = ["/tmp"]
work_root = "/tmp"
tmp_root = "/tmp"
"#,
    )
    .expect("config");
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let list = Command::new(binary)
        .args(["target", "list", "--config", config.to_str().unwrap()])
        .output()
        .expect("target list");
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    assert!(String::from_utf8_lossy(&list.stdout).contains("local-doctor"));

    let doctor = Command::new(binary)
        .args([
            "target",
            "doctor",
            "--config",
            config.to_str().unwrap(),
            "--target",
            "local-doctor",
        ])
        .output()
        .expect("target doctor");
    assert!(
        doctor.status.success(),
        "{}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&doctor.stdout).expect("doctor JSON");
    assert_eq!(report["target"], "local-doctor");
    assert_eq!(report["ok"], true);
}

#[test]
fn verification_command_sets_tmpdir_and_workload_environment() {
    let mut environment = BTreeMap::new();
    environment.insert("MUST".into(), "1".into());
    environment.insert(
        "EASYBUILD_ALLOW_USE_AS_ROOT_AND_ACCEPT_CONSEQUENCES".into(),
        "1".into(),
    );
    let target = BuildTarget {
        name: "local".into(),
        transport: TargetTransport::Local,
        executor: TargetExecutor::Direct,
        runtime: TargetRuntime::Host,
        easybuild: EasyBuildWorkload {
            command: "eb".into(),
            robot_paths: Vec::new(),
            work_root: "/work".into(),
            tmp_root: "/work/tmp".into(),
            environment,
        },
    };
    let plan = target.verification_command("module", &["load".into(), "App/1.0".into()]);
    assert_eq!(plan.program, "env");
    assert!(
        plan.args
            .iter()
            .any(|token| token == "EASYBUILD_TMPDIR=/work/tmp"),
        "{:?}",
        plan.args
    );
    assert!(
        plan.args.iter().any(|token| token == "MUST=1"),
        "{:?}",
        plan.args
    );
    assert!(
        plan.args
            .iter()
            .any(|token| { token == "EASYBUILD_ALLOW_USE_AS_ROOT_AND_ACCEPT_CONSEQUENCES=1" }),
        "{:?}",
        plan.args
    );
    let module = plan
        .args
        .iter()
        .position(|token| token == "module")
        .expect("module");
    assert!(
        plan.args[..module]
            .iter()
            .any(|token| token == "EASYBUILD_TMPDIR=/work/tmp"),
        "assignments must precede the program: {:?}",
        plan.args
    );
    assert_eq!(plan.args.get(module + 1).map(String::as_str), Some("load"));
}

#[test]
fn verification_command_puts_the_installed_module_tree_first() {
    let paths = |environment: &[(&str, &str)]| {
        let target = BuildTarget {
            name: "local".into(),
            transport: TargetTransport::Local,
            executor: TargetExecutor::Direct,
            runtime: TargetRuntime::Host,
            easybuild: EasyBuildWorkload {
                command: "eb".into(),
                robot_paths: Vec::new(),
                work_root: "/work".into(),
                tmp_root: "/tmp".into(),
                environment: environment
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect(),
            },
        };
        let plan = target.verification_command(
            "sh",
            &[
                "-c".into(),
                r#"printf '%s\n' "$MODULEPATH" "$EESSI_EXTRA_MODULEPATH""#.into(),
            ],
        );
        let output = plan.execute().expect("verification command");
        assert!(output.status.success(), "{plan:?}");
        String::from_utf8(output.stdout).expect("utf-8")
    };
    let site = [
        ("MODULEPATH", "/site/modules"),
        ("EESSI_EXTRA_MODULEPATH", "/site/extra"),
    ];
    let with = |extra: &[(&'static str, &'static str)]| {
        let mut environment = site.to_vec();
        environment.extend_from_slice(extra);
        paths(&environment)
    };
    assert_eq!(
        with(&[("EASYBUILD_INSTALLPATH", "/work/attempt-1")]),
        "/work/attempt-1/modules/all:/site/modules\n/work/attempt-1/modules/all:/site/extra\n"
    );
    assert_eq!(
        with(&[
            ("EASYBUILD_INSTALLPATH", "/work/attempt-1"),
            ("EASYBUILD_SUBDIR_MODULES", "mods"),
            ("EASYBUILD_SUFFIX_MODULES_PATH", "collection"),
        ]),
        "/work/attempt-1/mods/collection:/site/modules\n/work/attempt-1/mods/collection:/site/extra\n"
    );
    assert_eq!(
        with(&[
            ("EASYBUILD_INSTALLPATH", "/work/attempt-1"),
            ("EASYBUILD_INSTALLPATH_MODULES", "/work/modules"),
        ]),
        "/work/modules/all:/site/modules\n/work/modules/all:/site/extra\n"
    );
    assert_eq!(with(&[]), "/site/modules\n/site/extra\n");
}

#[test]
fn doctor_easybuild_probe_sets_tmpdir_and_workload_environment() {
    let mut environment = BTreeMap::new();
    environment.insert("MUST".into(), "1".into());
    environment.insert(
        "EASYBUILD_ALLOW_USE_AS_ROOT_AND_ACCEPT_CONSEQUENCES".into(),
        "1".into(),
    );
    let target = BuildTarget {
        name: "local".into(),
        transport: TargetTransport::Local,
        executor: TargetExecutor::Direct,
        runtime: TargetRuntime::Host,
        easybuild: EasyBuildWorkload {
            command: "true".into(),
            robot_paths: Vec::new(),
            work_root: "/work".into(),
            tmp_root: "/work/tmp".into(),
            environment,
        },
    };
    let report = doctor_target(&target).expect("doctor");
    let easybuild = report
        .checks
        .iter()
        .find(|check| check.layer == "easybuild")
        .expect("easybuild check");
    assert!(easybuild.success, "{easybuild:?}");
    let tokens = std::iter::once(easybuild.command.program.as_str())
        .chain(easybuild.command.args.iter().map(String::as_str))
        .collect::<Vec<_>>();
    assert_eq!(tokens.first().copied(), Some("env"), "{tokens:?}");
    let eb = tokens
        .iter()
        .position(|token| *token == "true")
        .expect("eb program");
    assert!(
        tokens[..eb].contains(&"EASYBUILD_TMPDIR=/work/tmp"),
        "assignments must precede eb: {tokens:?}"
    );
    assert!(tokens[..eb].contains(&"MUST=1"), "{tokens:?}");
    assert!(
        tokens[..eb].contains(&"EASYBUILD_ALLOW_USE_AS_ROOT_AND_ACCEPT_CONSEQUENCES=1"),
        "{tokens:?}"
    );
    assert_eq!(tokens.get(eb + 1).copied(), Some("--version"));
}

#[test]
fn doctor_reports_every_layer_when_a_probe_command_is_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = temp.path().join("targets.toml");
    std::fs::write(
        &config,
        r#"
schema_version = 1
[[targets]]
name = "missing-runtime"
[targets.transport]
kind = "local"
[targets.executor]
kind = "direct"
[targets.runtime]
kind = "podman"
image = "localhost/missing:latest"
command = "eb-stack-no-such-podman"
[targets.easybuild]
command = "true"
robot_paths = ["/tmp"]
work_root = "/tmp"
tmp_root = "/tmp"
"#,
    )
    .expect("config");
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let doctor = Command::new(binary)
        .args([
            "target",
            "doctor",
            "--config",
            config.to_str().unwrap(),
            "--target",
            "missing-runtime",
        ])
        .output()
        .expect("target doctor");
    assert!(
        !doctor.status.success(),
        "a failed probe must fail the command"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&doctor.stdout).expect("doctor JSON on stdout");
    assert_eq!(report["ok"], false);
    let checks = report["checks"].as_array().expect("checks");
    assert_eq!(checks.len(), 4, "every layer is reported: {report}");
    let by_layer = |name: &str| {
        checks
            .iter()
            .find(|check| check["layer"] == name)
            .unwrap_or_else(|| panic!("missing {name}"))
    };
    assert_eq!(by_layer("transport")["success"], true);
    assert_eq!(by_layer("executor")["success"], true);
    assert_eq!(by_layer("runtime")["success"], false);
    assert!(
        by_layer("runtime")["stderr"]
            .as_str()
            .unwrap_or("")
            .contains("eb-stack-no-such-podman"),
        "{}",
        by_layer("runtime")
    );
    assert_eq!(by_layer("easybuild")["success"], false);
    assert!(by_layer("runtime")["command"]["program"].is_string());
}

/// A stand-in EasyBuild command that prints `answer` and exits 0.
fn version_stub(dir: &std::path::Path, answer: &str) -> String {
    let path = dir.join("eb-local");
    write_executable(&path, &format!("#!/bin/sh\nprintf '%s\\n' '{answer}'\n")).expect("stub");
    path.to_str().expect("utf-8 path").into()
}

fn host_target(command: String, eessi_extend: Option<&str>) -> BuildTarget {
    let mut environment = BTreeMap::new();
    if let Some(value) = eessi_extend {
        environment.insert("EB_LOCAL_EESSI_EXTEND".into(), value.into());
    }
    BuildTarget {
        name: "eessi-extend-doctor".into(),
        transport: TargetTransport::Local,
        executor: TargetExecutor::Direct,
        runtime: TargetRuntime::Host,
        easybuild: EasyBuildWorkload {
            command,
            robot_paths: Vec::new(),
            work_root: "/tmp".into(),
            tmp_root: "/tmp".into(),
            environment,
        },
    }
}

#[test]
fn doctor_fails_an_eb_local_that_answers_version_itself_under_extend_mode() {
    let temp = tempfile::tempdir().expect("tempdir");
    let eb_local = version_stub(temp.path(), "eb-local 2.4.0 (x86_64-linux-musl)");
    let config = temp.path().join("targets.toml");
    std::fs::write(
        &config,
        format!(
            r#"
schema_version = 1
[[targets]]
name = "eessi-extend-old-eb-local"
[targets.transport]
kind = "local"
[targets.executor]
kind = "direct"
[targets.runtime]
kind = "host"
[targets.easybuild]
command = "{eb_local}"
work_root = "/tmp"
tmp_root = "/tmp"
[targets.easybuild.environment]
EB_LOCAL_EESSI_EXTEND = "1"
"#
        ),
    )
    .expect("config");
    let doctor = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args([
            "target",
            "doctor",
            "--config",
            config.to_str().unwrap(),
            "--target",
            "eessi-extend-old-eb-local",
        ])
        .output()
        .expect("target doctor");
    assert!(
        !doctor.status.success(),
        "an eb-local without extend mode must fail the doctor"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&doctor.stdout).expect("doctor JSON on stdout");
    assert_eq!(report["ok"], false);
    let checks = report["checks"].as_array().expect("checks");
    for check in checks.iter().filter(|check| check["layer"] != "easybuild") {
        assert_eq!(check["success"], true, "{check}");
        assert!(check.get("reason").is_none(), "{check}");
    }
    let easybuild = checks
        .iter()
        .find(|check| check["layer"] == "easybuild")
        .expect("easybuild check");
    assert_eq!(easybuild["success"], false, "{easybuild}");
    assert_eq!(easybuild["exit_code"], 0, "{easybuild}");
    let reason = easybuild["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("EB_LOCAL_EESSI_EXTEND") && reason.contains("2.5.0"),
        "{easybuild}"
    );
}

#[test]
fn doctor_passes_an_eb_local_that_reaches_easybuild_under_extend_mode() {
    let temp = tempfile::tempdir().expect("tempdir");
    let eb_local = version_stub(
        temp.path(),
        "Environment set up to use EESSI (2026.06), have fun!\n\
         This is EasyBuild 5.1.2 (framework: 5.1.2, easyblocks: 5.1.2) on host builder.",
    );
    let report = doctor_target(&host_target(eb_local, Some("1"))).expect("doctor");
    assert!(report.ok(), "{report:?}");
    assert!(report.checks.iter().all(|check| check.reason.is_none()));
}

#[test]
fn doctor_reads_the_extend_variable_as_eb_local_does() {
    let temp = tempfile::tempdir().expect("tempdir");
    let eb_local = version_stub(temp.path(), "eb-local 2.4.0 (x86_64-linux-musl)");
    for eessi_extend in [None, Some(""), Some("0")] {
        let report = doctor_target(&host_target(eb_local.clone(), eessi_extend)).expect("doctor");
        assert!(
            report.ok(),
            "EB_LOCAL_EESSI_EXTEND={eessi_extend:?} leaves extend mode off: {report:?}"
        );
    }
    let report = doctor_target(&host_target(eb_local, Some("yes"))).expect("doctor");
    assert!(!report.ok(), "{report:?}");
}

#[test]
fn public_target_examples_form_a_complete_layered_target() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let base = TargetConfigLayer::from_path(&root.join("examples/targets/base.toml"))
        .expect("base target");
    let site = TargetConfigLayer::from_path(&root.join("examples/targets/site.example.toml"))
        .expect("site target");
    let targets = resolve_target_layers(&[base, site]).expect("layer examples");
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].name, "site-builder");
    assert!(matches!(targets[0].runtime, TargetRuntime::Podman { .. }));
    assert_eq!(targets[0].easybuild.command, "eb");
}

#[test]
fn public_eessi_extend_target_routes_through_eessi_container() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let layer = TargetConfigLayer::from_path(&root.join("examples/targets/eessi-extend.toml"))
        .expect("eessi-extend target");
    let targets = resolve_target_layers(&[layer]).expect("resolve eessi-extend");
    assert_eq!(targets.len(), 1);
    let target = &targets[0];
    assert_eq!(target.name, "eessi-extend-user");
    assert!(matches!(target.runtime, TargetRuntime::Eessi { .. }));
    assert_eq!(target.easybuild.command, "/usr/local/bin/eb-local");
    assert_eq!(
        target
            .easybuild
            .environment
            .get("EB_LOCAL_EESSI_EXTEND")
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        target
            .easybuild
            .environment
            .get("EESSI_USER_INSTALL")
            .map(String::as_str),
        Some("/tmp/eb-stack-eessi/install")
    );
    let command = target.build_command("/tmp/eb-stack-eessi/work/bundles/art/art.eb");
    assert_eq!(command.program, "eessi_container.sh");
    let joined = command.args.join(" ");
    assert!(joined.contains("--mode exec"), "{joined}");
    assert!(joined.contains("--access ro"), "{joined}");
    assert!(joined.contains("/usr/local/bin/eb-local"), "{joined}");
    assert!(joined.contains("EB_LOCAL_EESSI_EXTEND=1"), "{joined}");
    assert!(!joined.contains("eessi-extend-eb.sh"), "{joined}");
    assert!(joined.contains("EESSI_USER_INSTALL="), "{joined}");
    assert!(joined.contains("art.eb"), "{joined}");
}

#[test]
fn public_local_podman_target_is_complete_and_abi_isolated() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let layer = TargetConfigLayer::from_path(&root.join("examples/targets/local-podman.toml"))
        .expect("local Podman target");
    let targets = resolve_target_layers(&[layer]).expect("resolve local Podman target");
    assert_eq!(targets.len(), 1);
    let target = &targets[0];
    assert_eq!(target.name, "local-rocky9");
    assert!(matches!(target.runtime, TargetRuntime::Podman { .. }));
    assert_eq!(target.easybuild.command, "eb");
    assert_eq!(
        target.easybuild.work_root,
        "/tmp/eb-stack/targets/rocky9/campaigns"
    );
    assert_eq!(
        target.easybuild.tmp_root,
        "/tmp/eb-stack/targets/rocky9/tmp"
    );
    assert_eq!(
        target
            .easybuild
            .environment
            .get("EASYBUILD_INSTALLPATH")
            .map(String::as_str),
        Some("/tmp/eb-stack/targets/rocky9/easybuild")
    );
    assert_eq!(
        target
            .easybuild
            .environment
            .get("EASYBUILD_SOURCEPATH")
            .map(String::as_str),
        Some("/tmp/eb-stack/sources")
    );
}

#[test]
fn shipped_easyblocks_reach_eb_as_explicit_include_easyblocks_files() {
    let layer = TargetConfigLayer::from_toml_str(
        r#"
schema_version = 1

[[targets]]
name = "eessi-user"

[targets.transport]
kind = "local"

[targets.executor]
kind = "direct"

[targets.runtime]
kind = "host"

[targets.easybuild]
command = "eb-local"
robot_paths = []
work_root = "/work/campaign"
tmp_root = "/work/tmp"
"#,
    )
    .expect("target");
    let targets = resolve_target_layers(&[layer]).expect("resolve targets");
    let command = targets[0].build_command_with_overlay(
        "/work/bundle/easyconfigs/s/SeisSol/SeisSol-1.3.2-foss-2025a.eb",
        &["/work/bundle/easyconfigs".into()],
        &[
            "/work/bundle/easyblocks/p/pspamm.py".into(),
            "/work/bundle/easyblocks/s/seissol.py".into(),
        ],
    );
    let tokens = std::iter::once(command.program.clone())
        .chain(command.args.iter().cloned())
        .collect::<Vec<_>>();
    let include = tokens
        .iter()
        .position(|token| {
            token
                == "--include-easyblocks=/work/bundle/easyblocks/p/pspamm.py,/work/bundle/easyblocks/s/seissol.py"
        })
        .unwrap_or_else(|| panic!("no --include-easyblocks in {tokens:?}"));
    let recipe = tokens
        .iter()
        .position(|token| token.ends_with("SeisSol-1.3.2-foss-2025a.eb"))
        .expect("recipe token");
    assert!(
        include < recipe,
        "options must precede the recipe: {tokens:?}"
    );
    assert!(tokens.contains(&"--robot=/work/bundle/easyconfigs".to_string()));

    let plain = targets[0].build_command_with_robot_paths("/work/bundle/x.eb", &[]);
    assert!(
        !plain
            .args
            .iter()
            .any(|token| token.starts_with("--include-easyblocks")),
        "no easyblocks means no option: {:?}",
        plain.args
    );
}

#[test]
fn eb_commands_scrub_compiler_wrapper_env_by_default() {
    let layer = TargetConfigLayer::from_toml_str(
        r#"
schema_version = 1

[[targets]]
name = "local"

[targets.transport]
kind = "local"

[targets.executor]
kind = "direct"

[targets.runtime]
kind = "host"

[targets.easybuild]
command = "eb"
robot_paths = []
work_root = "/tmp/work"
tmp_root = "/tmp/tmp"

[targets.easybuild.environment]
CMAKE_C_COMPILER_LAUNCHER = "sccache"
RUSTC_WRAPPER = "sccache"
EB_STACK_MARKER = "kept"
"#,
    )
    .expect("target");
    let targets = resolve_target_layers(&[layer]).expect("resolve");
    let command = targets[0].build_command("/tmp/work/bundle/recipe.eb");
    let joined = std::iter::once(command.program.clone())
        .chain(command.args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(joined.contains("-u CMAKE_C_COMPILER_LAUNCHER"), "{joined}");
    assert!(joined.contains("-u RUSTC_WRAPPER"), "{joined}");
    assert!(
        !joined.contains("CMAKE_C_COMPILER_LAUNCHER=sccache"),
        "{joined}"
    );
    assert!(!joined.contains("RUSTC_WRAPPER=sccache"), "{joined}");
    assert!(joined.contains("EB_STACK_MARKER=kept"), "{joined}");
}

#[test]
fn eb_commands_keep_wrapper_env_when_ccache_is_explicitly_requested() {
    let layer = TargetConfigLayer::from_toml_str(
        r#"
schema_version = 1

[[targets]]
name = "local"

[targets.transport]
kind = "local"

[targets.executor]
kind = "direct"

[targets.runtime]
kind = "host"

[targets.easybuild]
command = "eb"
robot_paths = []
work_root = "/tmp/work"
tmp_root = "/tmp/tmp"

[targets.easybuild.environment]
EASYBUILD_USE_CCACHE = "true"
CMAKE_C_COMPILER_LAUNCHER = "sccache"
RUSTC_WRAPPER = "sccache"
"#,
    )
    .expect("target");
    let targets = resolve_target_layers(&[layer]).expect("resolve");
    let command = targets[0].build_command("/tmp/work/bundle/recipe.eb");
    let joined = std::iter::once(command.program.clone())
        .chain(command.args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("CMAKE_C_COMPILER_LAUNCHER=sccache"),
        "{joined}"
    );
    assert!(joined.contains("RUSTC_WRAPPER=sccache"), "{joined}");
    assert!(!joined.contains("-u CMAKE_C_COMPILER_LAUNCHER"), "{joined}");
    assert!(!joined.contains("-u RUSTC_WRAPPER"), "{joined}");
}

// Ported from develop.

#[test]
fn empty_runtime_image_is_rejected() {
    let layer = TargetConfigLayer::from_toml_str(
        r#"
schema_version = 1

[[targets]]
name = "broken-podman"

[targets.transport]
kind = "local"

[targets.executor]
kind = "direct"

[targets.runtime]
kind = "podman"
image = ""

[targets.easybuild]
command = "eb"
work_root = "/work"
tmp_root = "/tmp"
"#,
    )
    .expect("layer");
    let error = resolve_target_layers(&[layer]).expect_err("empty image");
    assert!(error.to_string().contains("runtime image"), "{error}");
}
