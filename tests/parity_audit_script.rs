//! Reproducibility and failure-mode tests for `scripts/parity_audit.py`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

const EASYBUILD_BUILD_LOCK: &str = "scripts/easybuild-build-requirements.txt";
const EASYBUILD_LOCK: &str = "scripts/easybuild-requirements.txt";

/// Requirements in a pip lock file, continuation lines joined.
fn locked_requirements(rel: &str) -> Vec<String> {
    let path = repo_root().join(rel);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    let mut requirements = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.strip_suffix('\\') {
            Some(part) => current.push_str(part),
            None => {
                current.push_str(line);
                requirements.push(std::mem::take(&mut current));
            }
        }
    }
    assert!(current.is_empty(), "{rel} ends in a continuation");
    requirements
}

/// Create a virtual environment holding the locked EasyBuild.
fn locked_easybuild_venv(dir: &Path) -> PathBuf {
    let venv = dir.join("easybuild-venv");
    let status = Command::new("python3")
        .args(["-m", "venv"])
        .arg(&venv)
        .status()
        .expect("create EasyBuild virtual environment");
    assert!(status.success(), "create EasyBuild virtual environment");
    let python = venv.join("bin/python");
    install_locked_easybuild(&python);
    python
}

/// Install the locked EasyBuild into `python`'s environment.
fn install_locked_easybuild(python: &Path) {
    for args in [
        vec!["--only-binary", ":all:", "-r", EASYBUILD_BUILD_LOCK],
        vec!["--no-build-isolation", "-r", EASYBUILD_LOCK],
    ] {
        let status = Command::new(python)
            .current_dir(repo_root())
            .args(["-m", "pip", "install", "--disable-pip-version-check"])
            .arg("--require-hashes")
            .args(&args)
            .status()
            .expect("run pip");
        assert!(status.success(), "pip install {args:?}");
    }
}

#[test]
fn easybuild_locks_pin_every_package_with_a_hash() {
    for rel in [EASYBUILD_BUILD_LOCK, EASYBUILD_LOCK] {
        let requirements = locked_requirements(rel);
        assert!(!requirements.is_empty(), "{rel} pins nothing");
        for requirement in &requirements {
            let spec = requirement.split_whitespace().next().unwrap_or("");
            assert!(
                spec.contains("==") && !spec.contains('*'),
                "{rel}: {requirement} must pin an exact version"
            );
            let hashes: Vec<&str> = requirement
                .split_whitespace()
                .filter_map(|token| token.strip_prefix("--hash=sha256:"))
                .collect();
            assert!(
                !hashes.is_empty()
                    && hashes
                        .iter()
                        .all(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())),
                "{rel}: {requirement} must carry a sha256 hash"
            );
        }
    }
    let packages: Vec<String> = locked_requirements(EASYBUILD_LOCK)
        .iter()
        .filter_map(|requirement| requirement.split("==").next().map(str::to_string))
        .collect();
    assert_eq!(packages, ["easybuild-framework", "easybuild-easyblocks"]);
}

fn mismatch_fields(report: &serde_json::Value) -> Vec<String> {
    report["mismatches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|mismatch| mismatch["field"].as_str().unwrap().to_string())
        .collect()
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
        "easybuild/base/__init__.py",
        "easybuild/framework/__init__.py",
        "easybuild/framework/easyconfig/__init__.py",
        "easybuild/tools/__init__.py",
        "easybuild/tools/module_naming_scheme/__init__.py",
    ] {
        write_file(&root.join(rel), "");
    }
    write_file(
        &root.join("easybuild/base/fancylogger.py"),
        r#"import logging

def getLogger(name=None, fname=True):
    return logging.getLogger(name)
"#,
    );
    write_file(
        &root.join("easybuild/tools/build_log.py"),
        "class EasyBuildError(Exception):\n    pass\n",
    );
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
        &root.join("easybuild/tools/filetools.py"),
        r#"def decode_class_name(name):
    if not name.startswith("EB_"):
        return name
    return name[3:].replace("_minus_", "-")
"#,
    );
    write_file(
        &root.join("easybuild/tools/modules.py"),
        r#"class NoModulesTool:
    def __init__(self, *args, **kwargs):
        self.version = None
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
        r#"import tempfile
from pathlib import Path
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
        self.setdefault("easyblock", None)
        self._robot = options.ROBOT_ROOT or str(Path(path).parents[2])

    def dependencies(self):
        return [_resolve_dep(d, self._robot) for d in self.get("dependencies", [])]

    def builddependencies(self):
        return [_resolve_dep(d, self._robot) for d in self.get("builddependencies", [])]

class _Toolchain:
    dependencies = []

class _ModuleGenerator:
    def __init__(self, app):
        self.app = app

    def get_module_filepath(self, fake=False):
        return str(Path(self.app.module_dir) / EasyBuildMNS().det_full_module_name(self.app.cfg))

class StubEasyBlock:
    """Writes the module file a ConfigureMake easyblock would: a load for
    the toolchain unless SYSTEM, then one per runtime dependency."""

    def __init__(self, ec):
        self.cfg = ec
        self.toolchain = _Toolchain()
        self.module_generator = _ModuleGenerator(self)

    def post_init(self):
        pass

    def update_config_template_run_step(self):
        pass

    def gen_installdir(self):
        self.installdir_mod = tempfile.mkdtemp()

    def make_module_step(self, fake=False):
        self.module_dir = tempfile.mkdtemp()
        tc = self.cfg.get("toolchain") or {"name": "system", "version": "system"}
        loads = [] if tc["name"].lower() == "system" else [f"{tc['name']}/{tc['version']}"]
        loads += [dep["full_mod_name"] for dep in self.toolchain.dependencies]
        path = Path(self.module_generator.get_module_filepath(fake=True))
        path.parent.mkdir(parents=True, exist_ok=True)
        text = '#%Module\n' + "".join(f"module load {mod}\n" for mod in loads)
        text += f'setenv\tEBVERSION{self.cfg["name"].upper()}\t\t"{self.cfg["version"]}"\n'
        path.write_text(text)
        return self.module_dir

    def close_log(self):
        pass

def get_easyblock_class(easyblock, name=None):
    return StubEasyBlock
"#,
    );
}

/// The generated Python stub covers offline fixture cases; the real framework
/// resolution path is exercised by `parity_audit_resolves_with_easybuild_framework`.
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

/// Resolves a GCCcore dependency and an empty toolchain through the real
/// EasyBuild framework, installed from the hashed locks in `scripts/`.
#[test]
fn parity_audit_resolves_with_easybuild_framework() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ec_root = repo_root().join("fixtures/parity_audit_framework/easyconfigs");
    let report = temp.path().join("report.json");
    let python = locked_easybuild_venv(temp.path());

    let output = Command::new(&python)
        .arg(repo_root().join("scripts/parity_audit.py"))
        .arg("--easyconfigs")
        .arg(&ec_root)
        .arg("--seed")
        .arg("42")
        .arg("--sample-size")
        .arg("10")
        .arg("--eb-stack")
        .arg(env!("CARGO_BIN_EXE_eb-stack"))
        .arg("--out")
        .arg(&report)
        .output()
        .expect("run parity_audit.py with easybuild-framework");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap_or_default())
            .unwrap_or(serde_json::Value::Null);
    assert!(report.is_file(), "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert_eq!(
        doc["checked"], 4,
        "stdout:\n{stdout}\nstderr:\n{stderr}\n{doc}"
    );
    let fields = mismatch_fields(&doc);
    assert!(
        fields
            .iter()
            .any(|field| field == "runtime_dep:orphan:missing_toolchain"),
        "missing toolchain must be reported, got {fields:?}\n{doc}"
    );
    let app_mismatches: Vec<_> = doc["mismatches"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|mismatch| {
            mismatch["rel"]
                .as_str()
                .unwrap()
                .contains("App-1.0-GCCcore-13.3.0.eb")
        })
        .collect();
    assert!(
        app_mismatches.is_empty(),
        "GCCcore dependency resolution mismatched: {app_mismatches:?}"
    );
    assert!(
        !output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn parity_audit_compares_modules_generated_by_targeted_easyblocks() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = repo_root().join("fixtures/parity_audit_hooks");
    let report = temp.path().join("report.json");
    let python = locked_easybuild_venv(temp.path());

    let output = Command::new(&python)
        .arg(repo_root().join("scripts/parity_audit.py"))
        .arg("--easyconfigs")
        .arg(fixture.join("easyconfigs"))
        .arg("--sample-size")
        .arg("0")
        .arg("--include-easyblocks")
        .arg(fixture.join("easyblocks/hookload.py"))
        .arg("--include-easyblocks")
        .arg(fixture.join("easyblocks/hookversion.py"))
        .arg("--eb-stack")
        .arg(env!("CARGO_BIN_EXE_eb-stack"))
        .arg("--out")
        .arg(&report)
        .output()
        .expect("run parity_audit.py with easybuild-framework");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap_or_default())
            .unwrap_or(serde_json::Value::Null);
    assert_eq!(
        doc["targeted_count"], 2,
        "stdout:\n{stdout}\nstderr:\n{stderr}\n{doc}"
    );
    let mut found: Vec<(String, String)> = doc["mismatches"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|m| {
            (
                m["rel"].as_str().unwrap_or_default().to_string(),
                m["field"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            (
                "h/HookLoad/HookLoad-1.0.eb".to_string(),
                "generated_module_loads".to_string()
            ),
            (
                "h/HookVersion/HookVersion-1.0.eb".to_string(),
                "generated_module_version".to_string()
            ),
        ],
        "stdout:\n{stdout}\nstderr:\n{stderr}\n{doc}"
    );
    assert!(
        !output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// `PythonPackage` asks a bare `python` for its library directory while it
/// writes the module. With a `python` that always fails first on `PATH`,
/// the audit must still run the easyblock with its own interpreter.
#[test]
fn parity_audit_runs_python_easyblocks_with_its_own_interpreter() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = repo_root().join("fixtures/parity_audit_python");
    let report = temp.path().join("report.json");
    let truth = temp.path().join("truth.json");
    let python = locked_easybuild_venv(temp.path());
    let broken = temp.path().join("broken-python");
    std::fs::create_dir_all(&broken).expect("create broken python dir");
    std::os::unix::fs::symlink("/bin/false", broken.join("python")).expect("link python");
    let path = std::env::join_paths(std::iter::once(broken).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("join PATH");

    let output = Command::new(&python)
        .arg(repo_root().join("scripts/parity_audit.py"))
        .arg("--easyconfigs")
        .arg(fixture.join("easyconfigs"))
        .arg("--sample-size")
        .arg("0")
        .arg("--eb-stack")
        .arg(env!("CARGO_BIN_EXE_eb-stack"))
        .arg("--emit-truth")
        .arg(&truth)
        .arg("--out")
        .arg(&report)
        .env("PATH", path)
        .output()
        .expect("run parity_audit.py with easybuild-framework");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap_or_default())
            .unwrap_or(serde_json::Value::Null);
    assert!(
        doc["targeted_count"].as_u64().unwrap_or_default() >= 1,
        "stdout:\n{stdout}\nstderr:\n{stderr}\n{doc}"
    );
    assert_eq!(
        doc["mismatches"],
        serde_json::json!([]),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let truth: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&truth).unwrap_or_default())
            .unwrap_or(serde_json::Value::Null);
    assert_eq!(
        truth["results"][0]["generated_module_name"], "PyPkg/1.0",
        "the PythonPackage easyblock must write the module: {truth}"
    );
    assert!(
        output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// Targeted easyblocks get the preparation `eb --module-only --force` gives
/// them before `make_module_step`, so modules that resolve
/// `%(installdir)s`, read the parallelism or expect no installation are
/// audited instead of failing.
#[test]
fn parity_audit_prepares_targeted_easyblocks_like_easybuild() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = repo_root().join("fixtures/parity_audit_prepare");
    let report = temp.path().join("report.json");
    let python = locked_easybuild_venv(temp.path());

    let mut command = Command::new(&python);
    command
        .arg(repo_root().join("scripts/parity_audit.py"))
        .arg("--easyconfigs")
        .arg(fixture.join("easyconfigs"))
        .arg("--sample-size")
        .arg("0");
    for easyblock in ["hookinstalled.py", "hookparallel.py", "hooktemplate.py"] {
        command
            .arg("--include-easyblocks")
            .arg(fixture.join("easyblocks").join(easyblock));
    }
    let output = command
        .arg("--eb-stack")
        .arg(env!("CARGO_BIN_EXE_eb-stack"))
        .arg("--out")
        .arg(&report)
        .output()
        .expect("run parity_audit.py with easybuild-framework");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap_or_default())
            .unwrap_or(serde_json::Value::Null);
    assert_eq!(
        doc["targeted_count"], 3,
        "stdout:\n{stdout}\nstderr:\n{stderr}\n{doc}"
    );
    assert_eq!(
        doc["mismatches"],
        serde_json::json!([]),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn parity_audit_targets_easyblocks_by_their_decoded_class_names() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = repo_root().join("fixtures/parity_audit_class_names");
    let report = temp.path().join("report.json");
    let python = locked_easybuild_venv(temp.path());

    let output = Command::new(&python)
        .arg(repo_root().join("scripts/parity_audit.py"))
        .arg("--easyconfigs")
        .arg(fixture.join("easyconfigs"))
        .arg("--sample-size")
        .arg("0")
        .arg("--include-easyblocks")
        .arg(fixture.join("easyblocks/fftwmpi.py"))
        .arg("--eb-stack")
        .arg(env!("CARGO_BIN_EXE_eb-stack"))
        .arg("--out")
        .arg(&report)
        .output()
        .expect("run parity_audit.py with easybuild-framework");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap_or_default())
            .unwrap_or(serde_json::Value::Null);
    assert_eq!(
        doc["targeted_count"], 1,
        "FFTW.MPI must be targeted through EB_FFTW_period_MPI\nstdout:\n{stdout}\nstderr:\n{stderr}\n{doc}"
    );
    assert_eq!(
        doc["mismatches"],
        serde_json::json!([]),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn parity_audit_fails_closed_when_easybuild_configuration_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ec_root = temp.path().join("easyconfigs");
    let py_stub = temp.path().join("py_stub");
    let report = temp.path().join("report.json");
    generate_fixture_tree(&ec_root);
    generate_easybuild_python_stub(&py_stub);
    write_file(
        &py_stub.join("easybuild/tools/options.py"),
        r#"ROBOT_ROOT = None

def set_up_configuration(args=None, silent=True):
    raise RuntimeError("broken EasyBuild configuration file")
"#,
    );

    let output = Command::new("python3")
        .arg(repo_root().join("scripts/parity_audit.py"))
        .arg("--easyconfigs")
        .arg(&ec_root)
        .arg("--sample-size")
        .arg("4")
        .arg("--pythonpath")
        .arg(&py_stub)
        .arg("--eb-stack")
        .arg(env!("CARGO_BIN_EXE_eb-stack"))
        .arg("--out")
        .arg(&report)
        .output()
        .expect("run parity_audit.py");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr:\n{stderr}");
    assert!(
        stderr.contains("EasyBuild configuration failed")
            && stderr.contains("broken EasyBuild configuration file"),
        "stderr:\n{stderr}"
    );
    assert!(!report.exists(), "no report from a partial configuration");
}

#[test]
fn parity_audit_exits_nonzero_on_mismatch_and_dropped_exts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ec_root = temp.path().join("easyconfigs");
    generate_fixture_tree(&ec_root);

    let mut truth = tool_a_truth(serde_json::json!([{
        "name": "zlib",
        "version": "1.3.1",
        "versionsuffix": "",
        "toolchain_name": "GCCcore",
        "toolchain_version": "13.3.0",
        "external_module": false
    }]));
    truth["results"][0]["versionsuffix"] = "-wrong".into();
    truth["results"][0]["exts_list"] = serde_json::json!([{"name": "extone", "version": "0.1.0"}]);

    let (output, report) = run_comparator_against_eb_stack(temp.path(), &ec_root, &truth);

    assert!(
        !output.status.success(),
        "parity_audit.py must fail when versionsuffix/exts_count disagree"
    );
    assert_eq!(
        mismatch_fields(&report),
        ["versionsuffix", "exts_count", "extra_build_dep:CMake"]
    );
}

#[test]
fn parity_audit_reports_dropped_extensions_missing_toolchains_and_extra_deps() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ec_root = temp.path().join("easyconfigs");
    generate_fixture_tree(&ec_root);

    let mut truth = tool_a_truth(serde_json::json!([{
        "name": "zlib",
        "toolchain_name": null,
        "external_module": false
    }]));
    truth["results"][0]["exts_list"] = serde_json::json!([]);
    let mut stack = tool_a_stack(serde_json::json!([
        {"kind": "runtime", "name": "zlib", "resolved_module": null, "external_module": false},
        {"kind": "build", "name": "CMake", "resolved_module": "CMake/3.29.3", "external_module": false}
    ]));
    stack["recipes"][0]["exts_count"] = 0.into();
    stack["recipes"][0]["dropped_exts"] = true.into();
    stack["recipes"][0]["skipped_statements"] = serde_json::json!(["exts_list"]);

    let (output, report) = run_comparator(temp.path(), &ec_root, &truth, &stack);

    assert!(!output.status.success());
    let fields = mismatch_fields(&report);
    assert!(fields.iter().any(|field| field == "dropped_exts"));
    assert!(fields
        .iter()
        .any(|field| field == "runtime_dep:zlib:missing_toolchain"));
    assert!(fields.iter().any(|field| field == "extra_build_dep:CMake"));
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

#[test]
fn parity_audit_ignores_external_modules_on_both_sides() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ec_root = temp.path().join("easyconfigs");
    generate_fixture_tree(&ec_root);

    let truth = tool_a_truth(serde_json::json!([
        {
            "name": "cray-mpich",
            "version": "8.1.25",
            "external_module": true,
            "resolved_module": "cray-mpich/8.1.25",
            "toolchain_name": ""
        },
        {
            "name": "zlib",
            "version": "1.3.1",
            "toolchain_name": "GCCcore",
            "toolchain_version": "13.3.0",
            "external_module": false,
            "resolved_module": "zlib/1.3.1-GCCcore-13.3.0"
        }
    ]));
    let stack = tool_a_stack(serde_json::json!([
        {
            "kind": "runtime",
            "name": "cray-mpich",
            "version_req": "==8.1.25",
            "external_module": true,
            "resolved_module": "cray-mpich/8.1.25"
        },
        {
            "kind": "runtime",
            "name": "zlib",
            "version_req": "==1.3.1",
            "toolchain_name": "GCCcore",
            "toolchain_version": "13.3.0",
            "resolved_module": "zlib/1.3.1-GCCcore-13.3.0",
            "external_module": false
        },
        {
            "kind": "build",
            "name": "Ninja",
            "version_req": "==1.11.1",
            "resolved_module": "Ninja/1.11.1",
            "external_module": false
        }
    ]));

    let (output, report) = run_comparator(temp.path(), &ec_root, &truth, &stack);

    let fields = mismatch_fields(&report);
    assert!(
        !fields
            .iter()
            .any(|field| field == "extra_runtime_dep:cray-mpich"),
        "external module must not be an extra dependency: {fields:?}"
    );
    assert!(
        fields.iter().any(|field| field == "extra_build_dep:Ninja"),
        "a non-external extra dependency must still be reported: {fields:?}"
    );
    assert!(!output.status.success());
}

#[test]
fn parity_audit_reports_extra_dependency_version_and_toolchain() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ec_root = temp.path().join("easyconfigs");
    generate_fixture_tree(&ec_root);

    let truth = tool_a_truth(serde_json::json!([{
        "name": "zlib",
        "version": "1.3.1",
        "toolchain_name": "GCCcore",
        "toolchain_version": "13.3.0",
        "external_module": false,
        "resolved_module": "zlib/1.3.1-GCCcore-13.3.0"
    }]));
    let stack = tool_a_stack(serde_json::json!([
        {
            "kind": "runtime",
            "name": "zlib",
            "version_req": "==1.3.1",
            "toolchain_name": "GCCcore",
            "toolchain_version": "13.3.0",
            "resolved_module": "zlib/1.3.1-GCCcore-13.3.0",
            "external_module": false
        },
        {
            "kind": "runtime",
            "name": "zlib",
            "version_req": "==1.2.13",
            "toolchain_name": "GCCcore",
            "toolchain_version": "12.3.0",
            "resolved_module": "zlib/1.2.13-GCCcore-12.3.0",
            "external_module": false
        },
        {
            "kind": "runtime",
            "name": "zlib",
            "version_req": "==1.3.1",
            "toolchain_name": "GCC",
            "toolchain_version": "13.3.0",
            "resolved_module": "zlib/1.3.1-GCC-13.3.0",
            "external_module": false
        }
    ]));

    let (output, report) = run_comparator(temp.path(), &ec_root, &truth, &stack);

    let fields = mismatch_fields(&report);
    assert!(
        !fields.iter().any(|field| field == "runtime_dep:zlib"),
        "the shared zlib 1.3.1 GCCcore candidate must match: {report}"
    );
    let extra_modules: Vec<String> = report["mismatches"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|mismatch| mismatch["field"] == "extra_runtime_dep:zlib")
        .map(|mismatch| mismatch["eb_stack"].to_string())
        .collect();
    assert!(
        extra_modules
            .iter()
            .any(|text| text.contains("zlib/1.2.13-GCCcore-12.3.0")),
        "extra version under zlib must be reported: {extra_modules:?}"
    );
    assert!(
        extra_modules
            .iter()
            .any(|text| text.contains("zlib/1.3.1-GCC-13.3.0")),
        "extra toolchain under zlib must be reported: {extra_modules:?}"
    );
    assert!(!output.status.success());
}

fn run_comparator(
    temp: &Path,
    ec_root: &Path,
    truth: &serde_json::Value,
    stack: &serde_json::Value,
) -> (std::process::Output, serde_json::Value) {
    let stack_path = temp.join("stack.json");
    write_file(&stack_path, &serde_json::to_string_pretty(stack).unwrap());
    run_parity_audit(
        temp,
        ec_root,
        truth,
        &["--eb-stack-json".as_ref(), stack_path.as_os_str()],
    )
}

fn run_comparator_against_eb_stack(
    temp: &Path,
    ec_root: &Path,
    truth: &serde_json::Value,
) -> (std::process::Output, serde_json::Value) {
    run_parity_audit(
        temp,
        ec_root,
        truth,
        &[
            "--eb-stack".as_ref(),
            env!("CARGO_BIN_EXE_eb-stack").as_ref(),
        ],
    )
}

fn run_parity_audit(
    temp: &Path,
    ec_root: &Path,
    truth: &serde_json::Value,
    stack_args: &[&std::ffi::OsStr],
) -> (std::process::Output, serde_json::Value) {
    let truth_path = temp.join("truth.json");
    let report_path = temp.join("report.json");
    write_file(&truth_path, &serde_json::to_string_pretty(truth).unwrap());
    let output = Command::new("python3")
        .arg(repo_root().join("scripts/parity_audit.py"))
        .arg("--easyconfigs")
        .arg(ec_root)
        .arg("--truth-json")
        .arg(&truth_path)
        .args(stack_args)
        .arg("--out")
        .arg(&report_path)
        .output()
        .expect("run parity_audit.py");
    let report = serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();
    (output, report)
}

fn tool_a_truth(dependencies: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "results": [{
            "rel": "t/ToolA/ToolA-1.0-foss-2024a-extra.eb",
            "ok": true,
            "module_name": "ToolA/pre-1.0-foss-2024a-extra",
            "versionsuffix": "-extra",
            "exts_list": [
                {"name": "extone", "version": "0.1.0"},
                {"name": "exttwo", "version": "0.2.0"}
            ],
            "dependencies": dependencies,
            "builddependencies": []
        }]
    })
}

fn tool_a_stack(deps: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "recipes": [{
            "rel": "t/ToolA/ToolA-1.0-foss-2024a-extra.eb",
            "ok": true,
            "module_name": "ToolA/pre-1.0-foss-2024a-extra",
            "versionsuffix": "-extra",
            "exts_count": 2,
            "dropped_exts": false,
            "deps": deps
        }]
    })
}

#[test]
fn parity_audit_reports_an_external_module_resolved_in_tree() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ec_root = temp.path().join("easyconfigs");
    generate_fixture_tree(&ec_root);
    let truth = tool_a_truth(serde_json::json!([{
        "name": "cray-mpich",
        "version": "8.1.25",
        "external_module": true,
        "resolved_module": "cray-mpich/8.1.25",
        "toolchain_name": ""
    }]));
    let stack = tool_a_stack(serde_json::json!([{
        "kind": "runtime",
        "name": "cray-mpich",
        "version_req": "==8.1.25",
        "toolchain_name": "GCCcore",
        "toolchain_version": "13.3.0",
        "resolved_module": "cray-mpich/8.1.25-GCCcore-13.3.0",
        "external_module": false
    }]));

    let (output, report) = run_comparator(temp.path(), &ec_root, &truth, &stack);
    let fields = mismatch_fields(&report);
    assert_eq!(
        fields,
        ["runtime_dep:cray-mpich:external_resolved_in_tree"],
        "{report}"
    );
    assert!(!output.status.success());
}

#[test]
fn parity_audit_ignores_the_order_of_generated_module_loads() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ec_root = temp.path().join("easyconfigs");
    generate_fixture_tree(&ec_root);
    let mut truth = tool_a_truth(serde_json::json!([]));
    let row = &mut truth["results"][0];
    row["generated_module_name"] = "ToolA/pre-1.0-foss-2024a-extra".into();
    row["generated_module_loads"] = serde_json::json!(["foss/2024a", "zlib/1.3.1", "bzip2/1.0.8"]);
    row["generated_module_version"] = "1.0".into();
    let mut stack = tool_a_stack(serde_json::json!([
        {"kind": "runtime", "name": "bzip2", "version_req": "==1.0.8", "resolved_module": "bzip2/1.0.8", "external_module": false},
        {"kind": "runtime", "name": "zlib", "version_req": "==1.3.1", "resolved_module": "zlib/1.3.1", "external_module": false}
    ]));
    let recipe = &mut stack["recipes"][0];
    recipe["toolchain"] = serde_json::json!({"name": "foss", "version": "2024a"});
    recipe["version"] = "1.0".into();
    truth["results"][0]["dependencies"] = serde_json::json!([
        {"name": "zlib", "version": "1.3.1", "toolchain_name": "SYSTEM", "resolved_module": "zlib/1.3.1"},
        {"name": "bzip2", "version": "1.0.8", "toolchain_name": "SYSTEM", "resolved_module": "bzip2/1.0.8"}
    ]);

    let (output, report) = run_comparator(temp.path(), &ec_root, &truth, &stack);
    assert_eq!(report["mismatch_count"], 0, "{report}");
    assert!(output.status.success(), "{report}");
}

#[test]
fn parity_audit_holds_mismatches_to_a_baseline() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ec_root = temp.path().join("easyconfigs");
    generate_fixture_tree(&ec_root);
    let truth_path = temp.path().join("truth.json");
    let stack_path = temp.path().join("stack.json");
    let baseline = temp.path().join("baseline.json");
    let report_path = temp.path().join("report.json");
    let truth = tool_a_truth(serde_json::json!([{
        "name": "zlib",
        "version": "1.3.1",
        "toolchain_name": "GCCcore",
        "toolchain_version": "13.3.0",
        "resolved_module": "zlib/1.3.1-GCCcore-13.3.0"
    }]));
    write_file(&truth_path, &serde_json::to_string_pretty(&truth).unwrap());
    write_file(
        &stack_path,
        &serde_json::to_string_pretty(&tool_a_stack(serde_json::json!([]))).unwrap(),
    );
    let audit = |extra: &[&str]| {
        let output = Command::new("python3")
            .arg(repo_root().join("scripts/parity_audit.py"))
            .arg("--easyconfigs")
            .arg(&ec_root)
            .arg("--truth-json")
            .arg(&truth_path)
            .arg("--eb-stack-json")
            .arg(&stack_path)
            .arg("--out")
            .arg(&report_path)
            .args(extra)
            .output()
            .expect("run parity_audit.py");
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();
        (output.status.success(), report)
    };
    let baseline_arg = baseline.to_str().unwrap();

    let (ok, _) = audit(&[
        "--write-baseline",
        baseline_arg,
        "--easyconfigs-commit",
        "abc123",
    ]);
    assert!(!ok, "a mismatch without --baseline still fails");
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&baseline).unwrap()).unwrap();
    assert_eq!(
        written,
        serde_json::json!({
            "easyconfigs_commit": "abc123",
            "seed": 20261121,
            "sample_size": 2000,
            "mismatches": [{"rel": "t/ToolA/ToolA-1.0-foss-2024a-extra.eb", "field": "runtime_dep:zlib"}]
        })
    );

    let (ok, report) = audit(&["--baseline", baseline_arg]);
    assert!(ok, "{report}");
    assert_eq!(report["baseline"]["new"], serde_json::json!([]));
    assert_eq!(report["baseline"]["resolved"], serde_json::json!([]));

    let mut stale = written.clone();
    stale["mismatches"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"rel": "g/Gone/Gone-1.0.eb", "field": "easybuild_error"}));
    write_file(&baseline, &serde_json::to_string_pretty(&stale).unwrap());
    let (ok, report) = audit(&["--baseline", baseline_arg]);
    assert!(!ok, "{report}");
    assert_eq!(
        report["baseline"]["resolved"],
        serde_json::json!([{"rel": "g/Gone/Gone-1.0.eb", "field": "easybuild_error"}])
    );

    let mut empty = written.clone();
    empty["mismatches"] = serde_json::json!([]);
    write_file(&baseline, &serde_json::to_string_pretty(&empty).unwrap());
    let (ok, report) = audit(&["--baseline", baseline_arg]);
    assert!(!ok, "{report}");
    assert_eq!(report["baseline"]["new"], written["mismatches"]);

    let mut reseeded = written.clone();
    reseeded["seed"] = 7.into();
    write_file(&baseline, &serde_json::to_string_pretty(&reseeded).unwrap());
    let (ok, report) = audit(&["--baseline", baseline_arg]);
    assert!(!ok, "{report}");
    assert_eq!(
        report["baseline"]["problems"],
        serde_json::json!(["baseline seed is 7, this run used 20261121"])
    );
}

#[test]
fn parity_audit_requires_the_external_marker_on_every_stack_dep() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ec_root = temp.path().join("easyconfigs");
    generate_fixture_tree(&ec_root);
    let zlib = serde_json::json!({
        "name": "zlib",
        "version": "1.3.1",
        "toolchain_name": "GCCcore",
        "toolchain_version": "13.3.0",
        "external_module": false,
        "resolved_module": "zlib/1.3.1-GCCcore-13.3.0"
    });
    let truth = tool_a_truth(serde_json::json!([zlib]));
    let stack = tool_a_stack(serde_json::json!([{
        "kind": "runtime",
        "name": "zlib",
        "version_req": "==1.3.1",
        "toolchain_name": "GCCcore",
        "toolchain_version": "13.3.0",
        "resolved_module": "zlib/1.3.1-GCCcore-13.3.0"
    }]));

    let (output, report) = run_comparator(temp.path(), &ec_root, &truth, &stack);
    assert_eq!(
        mismatch_fields(&report),
        ["runtime_dep:zlib:missing_external_marker"],
        "{report}"
    );
    assert!(!output.status.success());
}

#[test]
fn recipe_parity_audit_marks_every_dependency_record() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ec_root = temp.path().join("easyconfigs");
    generate_fixture_tree(&ec_root);
    let recipes = temp.path().join("recipes.txt");
    write_file(&recipes, "t/ToolA/ToolA-1.0-foss-2024a-extra.eb\n");

    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["recipe", "parity-audit", "--easyconfigs"])
        .arg(&ec_root)
        .arg("--recipes-file")
        .arg(&recipes)
        .output()
        .expect("run eb-stack recipe parity-audit");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let deps = doc["recipes"][0]["deps"].as_array().unwrap();
    assert_eq!(deps.len(), 2, "{doc}");
    for dep in deps {
        assert_eq!(dep["external_module"], false, "{dep}");
    }
}
