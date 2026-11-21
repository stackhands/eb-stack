//! Reproducibility and failure-mode tests for `scripts/parity_audit.py`
//! using generated fixtures only.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn write_file(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(path, body).expect("write generated fixture");
}

fn generate_fixture_tree(root: &Path) {
    write_file(
        &root.join("g/GCCcore/GCCcore-13.3.0.eb"),
        r#"easyblock = 'Toolchain'
name = 'GCCcore'
version = '13.3.0'
homepage = 'https://gcc.gnu.org/'
description = 'GCCcore'
toolchain = SYSTEM
moduleclass = 'compiler'
"#,
    );
    write_file(
        &root.join("g/GCC/GCC-13.3.0.eb"),
        r#"easyblock = 'Toolchain'
name = 'GCC'
version = '13.3.0'
homepage = 'https://gcc.gnu.org/'
description = 'GCC'
toolchain = SYSTEM
dependencies = [('GCCcore', '13.3.0')]
moduleclass = 'toolchain'
"#,
    );
    write_file(
        &root.join("f/foss/foss-2024a.eb"),
        r#"easyblock = 'Toolchain'
name = 'foss'
version = '2024a'
homepage = 'https://easybuild.io'
description = 'foss'
toolchain = SYSTEM
dependencies = [('GCC', '13.3.0')]
moduleclass = 'toolchain'
"#,
    );
    write_file(
        &root.join("z/zlib/zlib-1.3.1-GCCcore-13.3.0.eb"),
        r#"easyblock = 'ConfigureMake'
name = 'zlib'
version = '1.3.1'
homepage = 'https://zlib.net/'
description = 'zlib'
toolchain = {'name': 'GCCcore', 'version': '13.3.0'}
moduleclass = 'lib'
"#,
    );
    write_file(
        &root.join("c/CMake/CMake-3.29.3-GCCcore-13.3.0.eb"),
        r#"easyblock = 'ConfigureMake'
name = 'CMake'
version = '3.29.3'
homepage = 'https://cmake.org/'
description = 'CMake'
toolchain = {'name': 'GCCcore', 'version': '13.3.0'}
moduleclass = 'devel'
"#,
    );
    write_file(
        &root.join("t/ToolA/ToolA-1.0-foss-2024a-extra.eb"),
        r#"easyblock = 'PythonBundle'
name = 'ToolA'
version = '1.0'
versionprefix = 'pre-'
versionsuffix = '-extra'
homepage = 'https://example.org/'
description = 'ToolA'
toolchain = {'name': 'foss', 'version': '2024a'}
builddependencies = [('CMake', '3.29.3')]
dependencies = [('zlib', '1.3.1')]
exts_list = [
    ('extone', '0.1.0'),
    ('exttwo', '0.2.0'),
]
moduleclass = 'tools'
"#,
    );
    write_file(
        &root.join("b/BundleB/BundleB-2.0-foss-2024a.eb"),
        r#"easyblock = 'Bundle'
name = 'BundleB'
version = '2.0'
homepage = 'https://example.org/'
description = 'BundleB'
toolchain = {'name': 'foss', 'version': '2024a'}
dependencies = [('zlib', '1.3.1')]
moduleclass = 'tools'
"#,
    );
}

fn generate_easyblocks_stub(root: &Path) {
    write_file(
        &root.join("easybuild/easyblocks/t/toola.py"),
        r#"class EB_ToolA(object):
    def det_full_module_name(self, ec):
        return None
"#,
    );
}

fn generate_easybuild_python_stub(root: &Path) {
    for rel in [
        "easybuild/__init__.py",
        "easybuild/framework/__init__.py",
        "easybuild/framework/easyconfig/__init__.py",
        "easybuild/tools/__init__.py",
        "easybuild/tools/module_naming_scheme/__init__.py",
    ] {
        write_file(&root.join(rel), "");
    }
    write_file(
        &root.join("easybuild/tools/options.py"),
        r#"ROBOT_ROOT = None

def set_up_configuration(args=None, silent=True):
    global ROBOT_ROOT
    for arg in args or []:
        if arg.startswith("--robot="):
            ROBOT_ROOT = arg.split("=", 1)[1]
"#,
    );
    write_file(
        &root.join("easybuild/tools/modules.py"),
        r#"class NoModulesTool:
    def __init__(self, testing=False):
        self.testing = testing
"#,
    );
    write_file(
        &root.join("easybuild/tools/module_naming_scheme/easybuild_mns.py"),
        r#"class EasyBuildMNS:
    def det_full_module_name(self, ec):
        tc = ec.get("toolchain") or {"name": "system", "version": "system"}
        tc_part = "" if tc["name"].lower() == "system" else f"-{tc['name']}-{tc['version']}"
        vp = ec.get("versionprefix") or ""
        vs = ec.get("versionsuffix") or ""
        return f"{ec['name']}/{vp}{ec['version']}{tc_part}{vs}"
"#,
    );
    write_file(
        &root.join("easybuild/framework/easyconfig/easyconfig.py"),
        r#"from pathlib import Path
from easybuild.tools import options
from easybuild.tools.module_naming_scheme.easybuild_mns import EasyBuildMNS

def _parse_eb(path):
    ns = {"SYSTEM": {"name": "system", "version": "system"}}
    ns["name"] = ""
    ns["version"] = ""
    exec(Path(path).read_text(encoding="utf-8"), ns)
    return ns

def _resolve_dep(dep_tuple, robot_root):
    name, version = dep_tuple[0], dep_tuple[1]
    mns = EasyBuildMNS()
    for p in sorted(Path(robot_root).rglob("*.eb")):
        ns = _parse_eb(p)
        if ns.get("name") == name and ns.get("version") == version:
            tc = ns.get("toolchain") or {"name": "system", "version": "system"}
            return {
                "name": name,
                "version": version,
                "versionprefix": ns.get("versionprefix") or "",
                "versionsuffix": ns.get("versionsuffix") or "",
                "toolchain": tc,
                "full_mod_name": mns.det_full_module_name(ns),
                "external_module": False,
                "hidden": False,
                "build_only": False,
            }
    return {
        "name": name,
        "version": version,
        "toolchain": {"name": "system", "version": "system"},
        "external_module": False,
    }

class EasyConfig(dict):
    def __init__(self, path, validate=False):
        super().__init__(_parse_eb(path))
        self._robot = options.ROBOT_ROOT or str(Path(path).parents[2])

    def dependencies(self):
        return [_resolve_dep(d, self._robot) for d in self.get("dependencies", [])]

    def builddependencies(self):
        return [_resolve_dep(d, self._robot) for d in self.get("builddependencies", [])]
"#,
    );
}

#[test]
fn parity_audit_passes_against_generated_easybuild_stub_and_eb_stack() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ec_root = temp.path().join("easyconfigs");
    let py_stub = temp.path().join("py_stub");
    let eblocks = temp.path().join("easyblocks");
    let report_a = temp.path().join("report-a.json");
    let report_b = temp.path().join("report-b.json");
    let truth_out = temp.path().join("truth.json");

    generate_fixture_tree(&ec_root);
    generate_easybuild_python_stub(&py_stub);
    generate_easyblocks_stub(&eblocks);

    let script = repo_root().join("scripts/parity_audit.py");
    let bin = env!("CARGO_BIN_EXE_eb-stack");

    for (report, emit_truth) in [(&report_a, Some(&truth_out)), (&report_b, None)] {
        let mut cmd = Command::new("python3");
        cmd.arg(&script)
            .arg("--easyconfigs")
            .arg(&ec_root)
            .arg("--seed")
            .arg("42")
            .arg("--sample-size")
            .arg("4")
            .arg("--easybuild-easyblocks")
            .arg(&eblocks)
            .arg("--pythonpath")
            .arg(&py_stub)
            .arg("--eb-stack")
            .arg(bin)
            .arg("--out")
            .arg(report);
        if let Some(truth_path) = emit_truth {
            cmd.arg("--emit-truth").arg(truth_path);
        }
        let output = cmd.output().expect("run parity_audit.py");
        assert!(
            output.status.success(),
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let doc_a: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report_a).unwrap()).unwrap();
    let doc_b: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report_b).unwrap()).unwrap();
    assert_eq!(doc_a["mismatch_count"], 0);
    assert_eq!(doc_a["targeted_count"], 5);
    assert!(doc_a["checked"].as_u64().unwrap() >= 5);
    assert_eq!(doc_a, doc_b, "seeded parity runs must be deterministic");
    assert!(truth_out.is_file());
}

#[test]
fn parity_audit_exits_nonzero_on_mismatch_and_dropped_exts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ec_root = temp.path().join("easyconfigs");
    generate_fixture_tree(&ec_root);

    let truth_path = temp.path().join("bad-truth.json");
    let truth = serde_json::json!({
        "results": [
            {
                "rel": "t/ToolA/ToolA-1.0-foss-2024a-extra.eb",
                "bucket": "sample:exts_list",
                "ok": true,
                "name": "ToolA",
                "version": "1.0",
                "versionprefix": "pre-",
                "versionsuffix": "-wrong",
                "toolchain_name": "foss",
                "toolchain_version": "2024a",
                "module_name": "ToolA/pre-1.0-foss-2024a-extra",
                "dependencies": [
                    {
                        "name": "zlib",
                        "version": "1.3.1",
                        "versionsuffix": "",
                        "toolchain_name": "GCCcore",
                        "toolchain_version": "13.3.0",
                        "external_module": false
                    }
                ],
                "builddependencies": [],
                "exts_list": [
                    {"name": "extone", "version": "0.1.0"}
                ]
            }
        ]
    });
    write_file(&truth_path, &serde_json::to_string_pretty(&truth).unwrap());

    let report_path = temp.path().join("bad-report.json");
    let output = Command::new("python3")
        .arg(repo_root().join("scripts/parity_audit.py"))
        .arg("--easyconfigs")
        .arg(&ec_root)
        .arg("--truth-json")
        .arg(&truth_path)
        .arg("--eb-stack")
        .arg(env!("CARGO_BIN_EXE_eb-stack"))
        .arg("--out")
        .arg(&report_path)
        .output()
        .expect("run parity_audit.py mismatch check");

    assert!(
        !output.status.success(),
        "parity_audit.py must fail when versionsuffix/exts_count disagree"
    );
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();
    assert_eq!(report["mismatch_count"], 2);
}

#[test]
fn parity_audit_discovers_all_hook_categories_and_prints_seed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ec_root = temp.path().join("easyconfigs");
    let py_stub = temp.path().join("py_stub");
    let eblocks = temp.path().join("easyblocks");
    let report = temp.path().join("report.json");

    generate_fixture_tree(&ec_root);
    generate_easybuild_python_stub(&py_stub);

    // Add one easyconfig per hook family beyond det_full_module_name.
    for (rel, name, easyblock) in [
        ("m/ModReqApp/ModReqApp-1.0.eb", "ModReqApp", None),
        ("n/ModNameApp/ModNameApp-1.0.eb", "ModNameApp", None),
        ("v/VerTplApp/VerTplApp-1.0.eb", "VerTplApp", None),
        ("t/TcHookApp/TcHookApp-1.0.eb", "TcHookApp", None),
        ("f/FooBar/FooBar-1.0.eb", "Foo-Bar", Some("Foo-Bar")),
    ] {
        let eb_line = match easyblock {
            Some(eb) => format!("easyblock = '{eb}'\n"),
            None => String::new(),
        };
        write_file(
            &ec_root.join(rel),
            &format!(
                "{eb_line}name = '{name}'\nversion = '1.0'\n\
                 homepage = 'https://example.org/'\ndescription = '{name}'\n\
                 toolchain = SYSTEM\nmoduleclass = 'tools'\n"
            ),
        );
    }

    write_file(
        &eblocks.join("easybuild/easyblocks/m/modreqapp.py"),
        "class EB_ModReqApp:\n    def make_module_req_guess(self):\n        return {}\n",
    );
    write_file(
        &eblocks.join("easybuild/easyblocks/n/modnameapp.py"),
        "class EB_ModNameApp:\n    def module_name(self):\n        return 'alt'\n",
    );
    write_file(
        &eblocks.join("easybuild/easyblocks/v/vertplapp.py"),
        "class EB_VerTplApp:\n    def generate_template_values(self):\n        self.cfg.template_values['x'] = '1'\n",
    );
    write_file(
        &eblocks.join("easybuild/easyblocks/t/tchookapp.py"),
        "class EB_TcHookApp:\n    def comp_family(self):\n        return 'GCC'\n",
    );
    write_file(
        &eblocks.join("easybuild/easyblocks/f/foo_minus_bar.py"),
        "class EB_Foo_minus_Bar:\n    def det_full_module_name(self, ec):\n        return None\n",
    );

    let output = Command::new("python3")
        .arg(repo_root().join("scripts/parity_audit.py"))
        .arg("--easyconfigs")
        .arg(&ec_root)
        .arg("--seed")
        .arg("20261122")
        .arg("--sample-size")
        .arg("0")
        .arg("--easybuild-easyblocks")
        .arg(&eblocks)
        .arg("--pythonpath")
        .arg(&py_stub)
        .arg("--eb-stack")
        .arg(env!("CARGO_BIN_EXE_eb-stack"))
        .arg("--out")
        .arg(&report)
        .output()
        .expect("run parity_audit.py");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("seed=20261122"),
        "stdout must print seed=20261122, got: {stdout}"
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    // GCCcore/GCC/foss (Toolchain) + ToolA (versionprefix) + BundleB (Bundle) = 5,
    // plus all 5 hook apps = 10 targeted.
    assert_eq!(doc["targeted_count"], 10);
    assert_eq!(doc["checked"], 10);
}
