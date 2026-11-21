#!/usr/bin/env python3
"""Reproducible parity audit between EasyBuild and ``eb-stack``.

Selects a deterministic stratified sample across an easyconfigs tree
(seeded via ``--seed``) plus a targeted set of recipes whose module
EasyBuild's easyblock hooks may shape: recipes using the ``ModuleRC``,
``Toolchain``, ``Bundle`` or ``Amber`` easyblocks, recipes setting
``versionprefix`` or ``modaltsoftname``, and recipes using an easyblock
found in the installed ``easybuild.easyblocks`` package,
``--easybuild-easyblocks`` or ``--include-easyblocks`` that overrides a
module-writing method (``module_naming``), a template method
(``template``), a toolchain hook (``toolchain``), or touches a naming
attribute such as ``modaltsoftname`` (``naming_attr``).

Compares ``eb-stack recipe parity-audit`` against EasyBuild (or a
pre-generated ``--truth-json`` oracle) across parse status, dropped
``exts_list``, ``module_name``, ``versionsuffix``, ``exts_count``, and
nearest-hierarchy-level dependency resolution. Every eb-stack
dependency record carries an ``external_module`` marker; an EasyBuild
external module pairs only with a dependency marked external, so an
external module resolved to an in-tree build is a mismatch. A dependency
matches on name and version, and on toolchain when both sides name one.

For every targeted recipe the oracle instantiates the recipe's easyblock
and has it write its module file (``make_module_step(fake=True)``), so
each overridden hook runs. From that file it compares the module name
(the file's path), the ``module load`` statements against the toolchain
module and eb-stack's resolved runtime dependencies in order, and the
``EBVERSION*`` value against the recipe version. Environment content the
hooks write (paths, variables) has no eb-stack counterpart and is not
compared. EasyBuild runs with ``EbStackAuditModulesTool``, which loads
no modules, and Tcl module syntax; the flat ``EasyBuildMNS`` needs no
loaded module to place a module file.

Exits 0 on full parity and 1 on any mismatch or error, including an
EasyBuild configuration or easyblock that fails.
"""

from __future__ import annotations

import argparse
import ast
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

TARGETED_EASYBLOCKS = ("ModuleRC", "Toolchain", "Bundle", "Amber")
_TOOLCHAIN_RE = re.compile(
    r"^\s*toolchain\s*=\s*\{\s*['\"]name['\"]\s*:\s*['\"]([^'\"]+)['\"]",
    re.MULTILINE,
)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--easyconfigs",
        type=Path,
        required=True,
        help="Path to the easyconfigs root directory.",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=20261121,
        help="PRNG seed for deterministic stratified sampling (default: 20261121).",
    )
    parser.add_argument(
        "--sample-size",
        type=int,
        default=2000,
        help="Target stratified random sample size (default: 2000).",
    )
    parser.add_argument(
        "--no-targeted",
        action="store_true",
        help="Do not add targeted module-naming easyblock recipes.",
    )
    parser.add_argument(
        "--easybuild-framework",
        type=Path,
        default=None,
        help="Optional easybuild-framework checkout path to prepend to sys.path.",
    )
    parser.add_argument(
        "--easybuild-easyblocks",
        type=Path,
        default=None,
        help="Optional easybuild-easyblocks checkout path to scan and add to sys.path.",
    )
    parser.add_argument(
        "--include-easyblocks",
        type=Path,
        action="append",
        default=[],
        help="Easyblock file to pass to EasyBuild's --include-easyblocks and scan for hooks.",
    )
    parser.add_argument(
        "--pythonpath",
        action="append",
        default=[],
        help="Extra directory to prepend to sys.path when importing easybuild.",
    )
    parser.add_argument(
        "--truth-json",
        type=Path,
        default=None,
        help="Optional JSON oracle file instead of importing EasyBuild.",
    )
    parser.add_argument(
        "--emit-truth",
        type=Path,
        default=None,
        help="Optional path to write the EasyBuild truth JSON.",
    )
    parser.add_argument(
        "--eb-stack",
        type=Path,
        default=None,
        help="Path to the eb-stack binary.",
    )
    parser.add_argument(
        "--eb-stack-json",
        type=Path,
        default=None,
        help="Optional pre-generated JSON from `eb-stack recipe parity-audit`.",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=None,
        help="Optional path to write the full parity report JSON.",
    )
    return parser.parse_args(argv)


_MODULE_NAMING_METHODS = frozenset(
    {
        "make_module_req_guess",
        "det_full_module_name",
        "module_name",
        "make_module_req",
        "make_module_dep",
        "make_module_extend_modpath",
        "make_module_step",
    }
)
_VERSION_TEMPLATE_METHODS = frozenset(
    {
        "generate_template_values",
        "update_config_template_run_step",
        "det_template_values",
    }
)
_TOOLCHAIN_HOOK_METHODS = frozenset(
    {
        "comp_family",
        "mpi_family",
        "toolchain_family",
    }
)
_NAMING_TEMPLATE_ATTRS = frozenset(
    {
        "template_values",
        "modaltsoftname",
        "env_mod_alt_soft_name",
        "versionprefix",
        "version_prefix",
        "altroot",
        "altversion",
    }
)


def _decode_eb_class_name(cls_name: str) -> str:
    raw = cls_name[3:] if cls_name.startswith("EB_") else cls_name
    return (
        raw.replace("_minus_", "-")
        .replace("_plus_", "+")
        .replace("_dot_", ".")
        .replace("_underscore_", "_")
    )


def _class_hook_categories(cls_node: ast.ClassDef) -> set[str]:
    categories: set[str] = set()
    for base in cls_node.bases:
        if (isinstance(base, ast.Name) and base.id == "Toolchain") or (
            isinstance(base, ast.Attribute) and base.attr == "Toolchain"
        ):
            categories.add("toolchain")

    for node in ast.walk(cls_node):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            if node.name in _MODULE_NAMING_METHODS:
                categories.add("module_naming")
            if node.name in _VERSION_TEMPLATE_METHODS:
                categories.add("template")
            if node.name in _TOOLCHAIN_HOOK_METHODS or "toolchain" in node.name.lower():
                categories.add("toolchain")
        elif isinstance(node, ast.Attribute) and node.attr in _NAMING_TEMPLATE_ATTRS:
            categories.add("naming_attr")
        elif isinstance(node, ast.Name) and node.id in _NAMING_TEMPLATE_ATTRS:
            categories.add("naming_attr")
        elif isinstance(node, ast.Constant) and isinstance(node.value, str):
            if node.value in _NAMING_TEMPLATE_ATTRS:
                categories.add("naming_attr")
    return categories


def installed_easyblocks_root() -> Path | None:
    """The importable ``easybuild.easyblocks`` package, if any."""
    try:
        import easybuild.easyblocks as easyblocks
    except ImportError:
        return None
    paths = list(getattr(easyblocks, "__path__", []))
    return Path(paths[0]) if paths else None


def discover_easyblocks_touching_naming(sources: list[Path]) -> dict[str, set[str]]:
    """Hook categories per easyblock name, from directories and files."""
    files: list[Path] = []
    for source in sources:
        if source.is_dir():
            files.extend(sorted(source.rglob("*.py")))
        elif source.is_file():
            files.append(source)
    matched: dict[str, set[str]] = {}
    for path in files:
        try:
            text = path.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        try:
            tree = ast.parse(text, filename=str(path))
        except SyntaxError:
            continue
        for node in ast.walk(tree):
            if not isinstance(node, ast.ClassDef):
                continue
            categories = _class_hook_categories(node)
            if not categories:
                continue
            cls = node.name
            names = {cls}
            if cls.startswith("EB_"):
                names.update({cls[3:], _decode_eb_class_name(cls)})
            for name in names:
                matched.setdefault(name, set()).update(categories)
    return matched


def collect_easyconfig_files(root: Path) -> list[tuple[str, str]]:
    files: list[tuple[str, str]] = []
    for path in sorted(root.rglob("*.eb")):
        rel = path.relative_to(root).as_posix()
        if "/__archive__/" in f"/{rel}":
            continue
        text = path.read_text(encoding="utf-8", errors="ignore")
        files.append((rel, text))
    return files


def classify_recipe(
    rel: str, text: str, extra_easyblocks: dict[str, set[str]]
) -> tuple[str, set[str]]:
    """The sample bucket and the hook categories that target the recipe."""
    easyblock_match = re.search(
        r"^\s*easyblock\s*=\s*['\"]([^'\"]+)['\"]", text, re.MULTILINE
    )
    easyblock = easyblock_match.group(1) if easyblock_match else ""
    name_match = re.search(
        r"^\s*name\s*=\s*['\"]([^'\"]+)['\"]", text, re.MULTILINE
    )
    software_name = name_match.group(1) if name_match else ""
    tc_match = _TOOLCHAIN_RE.search(text)
    tc_name = tc_match.group(1) if tc_match else "system"

    categories: set[str] = set()
    if easyblock in TARGETED_EASYBLOCKS:
        categories.add(f"easyblock:{easyblock}")
    if easyblock:
        categories.update(extra_easyblocks.get(easyblock, set()))
    else:
        categories.update(extra_easyblocks.get(software_name, set()))
    if re.search(r"^\s*version_?prefix\s*=", text, re.MULTILINE):
        categories.add("versionprefix")
    if re.search(r"^\s*(?:modaltsoftname|env_mod_alt_soft_name)\s*=", text, re.MULTILINE):
        categories.add("modaltsoftname")

    if easyblock == "Toolchain" or tc_name in ("foss", "intel", "gompi", "gfbf", "NVHPC"):
        bucket = f"tc:{tc_name}"
    elif "Bundle" in easyblock:
        bucket = "bundle"
    elif re.search(r"^\s*exts_list\s*=", text, re.MULTILINE):
        bucket = "exts_list"
    elif re.search(r"^\s*versionprefix\s*=", text, re.MULTILINE):
        bucket = "versionprefix"
    elif re.search(r"^\s*(dependencies|builddependencies)\s*=", text, re.MULTILINE):
        bucket = "deps"
    else:
        bucket = f"tc:{tc_name}"
    return bucket, categories


def select_sample(
    root: Path,
    seed: int,
    sample_size: int,
    include_targeted: bool,
    extra_easyblocks: dict[str, set[str]],
) -> tuple[list[str], dict[str, str]]:
    files = collect_easyconfig_files(root)
    buckets: dict[str, list[str]] = {}
    targeted: list[str] = []
    reasons: dict[str, str] = {}

    for rel, text in files:
        bucket, categories = classify_recipe(rel, text, extra_easyblocks)
        buckets.setdefault(bucket, []).append(rel)
        if include_targeted and categories:
            targeted.append(rel)
            reasons[rel] = "targeted:" + ",".join(sorted(categories))

    rng = random.Random(seed)
    chosen: list[str] = []
    chosen_set: set[str] = set()

    if sample_size > 0 and files:
        bucket_names = sorted(buckets)
        per_bucket = max(1, sample_size // max(1, len(bucket_names)))
        remainder_pool: list[tuple[str, str]] = []
        for bname in bucket_names:
            items = list(buckets[bname])
            rng.shuffle(items)
            take = items[:per_bucket]
            for rel in take:
                if len(chosen) >= sample_size:
                    break
                if rel not in chosen_set:
                    chosen.append(rel)
                    chosen_set.add(rel)
                    reasons.setdefault(rel, f"sample:{bname}")
            for rel in items[per_bucket:]:
                remainder_pool.append((bname, rel))
        if len(chosen) < sample_size and remainder_pool:
            rng.shuffle(remainder_pool)
            for bname, rel in remainder_pool:
                if len(chosen) >= sample_size:
                    break
                if rel not in chosen_set:
                    chosen.append(rel)
                    chosen_set.add(rel)
                    reasons.setdefault(rel, f"sample:{bname}")

    for rel in sorted(targeted):
        if rel not in chosen_set:
            chosen.append(rel)
            chosen_set.add(rel)

    return sorted(chosen), reasons


def format_eb_dep_module(dep: dict) -> str:
    if dep.get("resolved_module"):
        return str(dep["resolved_module"])
    name = str(dep["name"])
    version = str(dep.get("version") or dep.get("version_req") or "")
    vprefix = str(dep.get("versionprefix") or "")
    vsuffix = str(dep.get("versionsuffix") or "")
    tc_name = str(dep.get("toolchain_name") or "")
    tc_ver = str(dep.get("toolchain_version") or "")
    if not tc_name or tc_name.lower() == "system" or tc_ver.lower() == "system":
        tc_part = ""
    elif tc_ver:
        tc_part = f"-{tc_name}-{tc_ver}"
    else:
        tc_part = f"-{tc_name}"
    return f"{name}/{vprefix}{version}{tc_part}{vsuffix}"


def add_easybuild_to_path(
    framework_dir: Path | None,
    easyblocks_dir: Path | None,
    extra_pythonpath: list[str],
) -> None:
    for entry in reversed(extra_pythonpath):
        if entry and entry not in sys.path:
            sys.path.insert(0, entry)
    for extra_dir in (easyblocks_dir, framework_dir):
        if extra_dir is not None:
            resolved = str(extra_dir.resolve())
            if resolved not in sys.path:
                sys.path.insert(0, resolved)


_MODULE_LOAD_RE = re.compile(r"^\s*module load (\S+)\s*$", re.MULTILINE)
_EBVERSION_RE = re.compile(r'^setenv\s+EBVERSION\S*\s+"([^"]*)"', re.MULTILINE)


def easyblock_module_truth(easyconfig_class, get_easyblock_class, path: Path) -> dict:
    """What the recipe's easyblock writes into its module file.

    The easyblock gets a fresh easyconfig, since its constructor and hooks
    may change it, and the preparation ``eb`` gives it before any step:
    ``post_init`` and the run-step templates such as ``%(installdir)s``.
    The toolchain is handed the recipe's dependencies as ``prepare_step``
    would, without loading their modules.
    """
    ec = easyconfig_class(str(path), validate=False)
    app_class = get_easyblock_class(ec["easyblock"], name=ec["name"])
    app = app_class(ec)
    try:
        app.post_init()
        app.gen_installdir()
        # make_module_dep recognises the top of the flat module tree by
        # comparing existing paths.
        os.makedirs(app.installdir_mod, exist_ok=True)
        app.update_config_template_run_step()
        app.toolchain.dependencies = list(app.cfg.dependencies())
        module_dir = Path(app.make_module_step(fake=True))
        module_file = Path(app.module_generator.get_module_filepath(fake=True))
        text = module_file.read_text(encoding="utf-8")
    finally:
        app.close_log()
    versions = _EBVERSION_RE.findall(text)
    return {
        "easyblock": app_class.__name__,
        "generated_module_name": module_file.relative_to(module_dir).as_posix(),
        "generated_module_loads": _MODULE_LOAD_RE.findall(text),
        "generated_module_version": versions[0] if len(versions) == 1 else None,
    }


def extract_easybuild_truth(
    root: Path,
    rels: list[str],
    reasons: dict[str, str],
    include_easyblocks: list[Path],
) -> dict:
    try:
        from easybuild.base import fancylogger
        from easybuild.framework.easyconfig.easyconfig import EasyConfig, get_easyblock_class
        from easybuild.tools.build_log import EasyBuildError
        from easybuild.tools.module_naming_scheme.easybuild_mns import EasyBuildMNS
        from easybuild.tools.modules import NoModulesTool
        from easybuild.tools.options import set_up_configuration
    except ImportError as exc:
        raise SystemExit(
            "EasyBuild is not importable; pass --truth-json or --easybuild-framework. "
            f"Import error: {exc}"
        ) from exc

    # A ModulesTool subclass is a --modules-tool choice. This one never
    # loads a module, so the truth does not depend on the host's modules.
    class EbStackAuditModulesTool(NoModulesTool):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            self.log = fancylogger.getLogger(self.__class__.__name__, fname=False)
            self.supports_depends_on = False
            self.supports_tcl_getenv = False
            self.supports_tcl_check_group = False
            self.supports_safe_auto_load = False
            self.supports_extensions = False

        def load(self, modules, *args, **kwargs):
            if modules:
                raise EasyBuildError("the parity audit loads no modules: %s", modules)

    workdir = Path(tempfile.mkdtemp(prefix="parity-audit-"))
    # EasyBuild helpers such as det_pylibdir run a bare `python`. Only that
    # name is bound to this interpreter; its own directory may hold eb.
    shim_bin = workdir / "bin"
    shim_bin.mkdir()
    (shim_bin / "python").symlink_to(sys.executable)
    orig_path = os.environ.get("PATH")
    # EasyBuild points tempfile at its own tmpdir under workdir.
    orig_tempdir = tempfile.tempdir
    os.environ["PATH"] = os.pathsep.join(filter(None, [str(shim_bin), orig_path]))
    try:
        return _easybuild_truth(
            root, rels, reasons, include_easyblocks, workdir, EbStackAuditModulesTool
        )
    finally:
        if orig_path is None:
            os.environ.pop("PATH", None)
        else:
            os.environ["PATH"] = orig_path
        tempfile.tempdir = orig_tempdir
        shutil.rmtree(workdir, ignore_errors=True)


def _easybuild_truth(
    root: Path,
    rels: list[str],
    reasons: dict[str, str],
    include_easyblocks: list[Path],
    workdir: Path,
    modules_tool: type,
) -> dict:
    from easybuild.framework.easyconfig.easyconfig import EasyConfig, get_easyblock_class
    from easybuild.tools.module_naming_scheme.easybuild_mns import EasyBuildMNS
    from easybuild.tools.options import set_up_configuration

    args = [
        f"--robot={root}",
        f"--modules-tool={modules_tool.__name__}",
        # EasyBuild requires Lmod for Lua module files.
        "--module-syntax=Tcl",
        f"--prefix={workdir}",
        f"--tmpdir={workdir / 'tmp'}",
        "--ignore-osdeps",
        # What `eb --module-only --force` sets; easyblocks such as
        # EB_GROMACS only write a module for a missing installation then.
        "--module-only",
        "--force",
    ]
    if include_easyblocks:
        args.append(
            "--include-easyblocks=" + ",".join(str(p.resolve()) for p in include_easyblocks)
        )
    # A partial configuration would leave robot, module and environment
    # settings at defaults the truth was never meant to use.
    try:
        set_up_configuration(args=args, silent=True)
    except (Exception, SystemExit) as exc:
        raise SystemExit(f"parity_audit: EasyBuild configuration failed: {exc}") from exc

    mns = EasyBuildMNS()
    results = []
    for rel in rels:
        path = root / rel
        try:
            ec = EasyConfig(str(path), validate=False)
            full_mod_name = mns.det_full_module_name(ec)
            tc = ec.get("toolchain") or {}
            tc_name = str(tc.get("name", "system"))
            tc_ver = str(tc.get("version", "system"))
            all_deps = (
                ec.dependencies()
                if callable(getattr(ec, "dependencies", None))
                else ec.get("dependencies", [])
            )
            deps = [
                dep_to_dict(dep)
                for dep in all_deps
                if not (isinstance(dep, dict) and dep.get("build_only"))
            ]
            bdeps = []
            bdep_raw = (
                ec.builddependencies()
                if callable(getattr(ec, "builddependencies", None))
                else ec.get("builddependencies", [])
            )
            for dep in bdep_raw:
                bdeps.append(dep_to_dict(dep))
            try:
                exts = ec.get("exts_list", resolve=False) or []
            except TypeError:
                exts = ec.get("exts_list") or []
            hooks = {}
            if reasons.get(rel, "").startswith("targeted"):
                try:
                    hooks = easyblock_module_truth(EasyConfig, get_easyblock_class, path)
                except Exception as exc:
                    raise RuntimeError(f"easyblock hooks: {exc}") from exc
            results.append(
                {
                    **hooks,
                    "rel": rel,
                    "bucket": reasons.get(rel, ""),
                    "ok": True,
                    "name": str(ec["name"]),
                    "version": str(ec["version"]),
                    "versionprefix": str(ec.get("versionprefix") or ""),
                    "versionsuffix": str(ec.get("versionsuffix") or ""),
                    "toolchain_name": tc_name,
                    "toolchain_version": tc_ver,
                    "module_name": str(full_mod_name),
                    "dependencies": deps,
                    "builddependencies": bdeps,
                    "exts_list": [
                        {
                            "name": str(e[0]) if isinstance(e, (list, tuple)) else str(e),
                            "version": str(e[1])
                            if isinstance(e, (list, tuple)) and len(e) > 1
                            else "",
                        }
                        for e in exts
                    ],
                }
            )
        except Exception as exc:
            results.append(
                {
                    "rel": rel,
                    "bucket": reasons.get(rel, ""),
                    "ok": False,
                    "error": str(exc),
                }
            )
    return {"results": results}


def dep_to_dict(dep) -> dict:
    if isinstance(dep, dict):
        tc = dep.get("toolchain") or {}
        return {
            "name": str(dep["name"]),
            "version": str(dep.get("version", "")),
            "versionprefix": str(dep.get("versionprefix") or ""),
            "versionsuffix": str(dep.get("versionsuffix") or ""),
            "toolchain_name": str(dep.get("toolchain_name") or tc.get("name") or ""),
            "toolchain_version": str(dep.get("toolchain_version") or tc.get("version") or ""),
            "external_module": bool(dep.get("external_module", False)),
            "hidden": bool(dep.get("hidden", False)),
            "resolved_module": dep.get("resolved_module") or dep.get("full_mod_name"),
        }
    if isinstance(dep, (list, tuple)):
        name = str(dep[0])
        version = str(dep[1]) if len(dep) > 1 else ""
        vsuffix = str(dep[2]) if len(dep) > 2 and dep[2] is not None else ""
        tc = dep[3] if len(dep) > 3 else None
        if isinstance(tc, (list, tuple)) and len(tc) >= 2:
            tc_name, tc_ver = str(tc[0]), str(tc[1])
        elif isinstance(tc, dict):
            tc_name, tc_ver = str(tc.get("name", "")), str(tc.get("version", ""))
        elif tc is True:
            tc_name, tc_ver = "system", "system"
        else:
            tc_name, tc_ver = "", ""
        return {
            "name": name,
            "version": version,
            "versionprefix": "",
            "versionsuffix": vsuffix,
            "toolchain_name": tc_name,
            "toolchain_version": tc_ver,
            "external_module": False,
            "hidden": False,
        }
    return {
        "name": str(dep),
        "version": "",
        "versionprefix": "",
        "versionsuffix": "",
        "toolchain_name": "",
        "toolchain_version": "",
        "external_module": False,
        "hidden": False,
    }


def resolve_eb_stack_bin(explicit: Path | None) -> Path:
    if explicit is not None:
        return explicit
    env_bin = os.environ.get("EB_STACK_BIN")
    if env_bin:
        return Path(env_bin)
    repo_root = Path(__file__).resolve().parents[1]
    for candidate in (
        repo_root / "target" / "release" / "eb-stack",
        repo_root / "target" / "debug" / "eb-stack",
    ):
        if candidate.is_file():
            return candidate
    found = shutil.which("eb-stack")
    if found:
        return Path(found)
    raise SystemExit(
        "Could not locate eb-stack binary. Pass --eb-stack or build with cargo build."
    )


def run_eb_stack_audit(eb_stack_bin: Path, root: Path, rels: list[str]) -> dict:
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False, encoding="utf-8") as fp:
        for rel in rels:
            fp.write(f"{rel}\n")
        recipes_file = Path(fp.name)
    try:
        proc = subprocess.run(
            [
                str(eb_stack_bin),
                "recipe",
                "parity-audit",
                "--easyconfigs",
                str(root),
                "--recipes-file",
                str(recipes_file),
            ],
            capture_output=True,
            text=True,
            check=False,
        )
    finally:
        recipes_file.unlink(missing_ok=True)
    if proc.returncode != 0:
        raise SystemExit(
            f"eb-stack recipe parity-audit failed ({proc.returncode}): {proc.stderr.strip()}"
        )
    return json.loads(proc.stdout)


def _is_external(dep: dict) -> bool:
    return bool(dep.get("external_module"))


def _dep_version(dep: dict) -> str:
    raw = dep.get("version")
    if raw in (None, ""):
        raw = dep.get("version_req") or ""
    text = str(raw)
    if text.startswith("=="):
        return text[2:]
    return text


def _dep_toolchain(dep: dict) -> tuple[str, str] | None:
    name = dep.get("toolchain_name")
    version = dep.get("toolchain_version")
    if not name:
        toolchain = dep.get("toolchain")
        if isinstance(toolchain, dict):
            name = toolchain.get("name") or ""
            version = toolchain.get("version") or ""
        else:
            return None
    name = str(name)
    if not name:
        return None
    return (name, str(version or ""))


def _same_dep(left: dict, right: dict) -> bool:
    """Name and version, plus toolchain when both sides record one."""
    if str(left.get("name") or "") != str(right.get("name") or ""):
        return False
    if _dep_version(left) != _dep_version(right):
        return False
    left_tc = _dep_toolchain(left)
    right_tc = _dep_toolchain(right)
    return not (left_tc and right_tc and left_tc != right_tc)


def expected_module_loads(stack: dict) -> list:
    """The loads EasyBuild writes for eb-stack's answer: the toolchain
    module unless SYSTEM, then each runtime dependency in recipe order."""
    loads: list = []
    toolchain = stack.get("toolchain") or {}
    name = str(toolchain.get("name") or "")
    if name and name.lower() != "system":
        loads.append(f"{name}/{toolchain.get('version') or ''}")
    loads.extend(
        dep.get("resolved_module") for dep in stack.get("deps", []) if dep.get("kind") == "runtime"
    )
    return loads


def compare_generated_module(rel: str, truth: dict, stack: dict) -> list[dict]:
    """Compare the module file a targeted recipe's easyblock wrote."""
    found = []
    for field, easybuild, eb_stack in (
        ("generated_module_name", truth.get("generated_module_name"), stack.get("module_name")),
        ("generated_module_loads", truth.get("generated_module_loads"), expected_module_loads(stack)),
        ("generated_module_version", truth.get("generated_module_version"), stack.get("version")),
    ):
        if easybuild != eb_stack:
            found.append(
                {
                    "rel": rel,
                    "field": field,
                    "easybuild": easybuild,
                    "eb_stack": eb_stack,
                }
            )
    return found


def compare_parity(truth_rows: list[dict], stack_rows: list[dict]) -> dict:
    by_rel_stack = {row["rel"]: row for row in stack_rows}
    mismatches: list[dict] = []
    checked = 0

    for truth in truth_rows:
        rel = truth["rel"]
        checked += 1
        if truth.get("ok") is False:
            mismatches.append(
                {
                    "rel": rel,
                    "field": "easybuild_error",
                    "easybuild": truth.get("error", "error"),
                    "eb_stack": None,
                }
            )
            continue

        stack = by_rel_stack.get(rel)
        if stack is None:
            mismatches.append(
                {
                    "rel": rel,
                    "field": "missing_from_eb_stack",
                    "easybuild": truth.get("module_name"),
                    "eb_stack": None,
                }
            )
            continue

        if not stack.get("ok"):
            mismatches.append(
                {
                    "rel": rel,
                    "field": "eb_stack_error",
                    "easybuild": truth.get("module_name"),
                    "eb_stack": stack.get("error"),
                }
            )
            continue

        if stack.get("dropped_exts"):
            mismatches.append(
                {
                    "rel": rel,
                    "field": "dropped_exts",
                    "easybuild": len(truth.get("exts_list", [])),
                    "eb_stack": stack.get("skipped_statements", []),
                }
            )

        eb_mod = truth.get("module_name", "")
        st_mod = stack.get("module_name", "")
        if eb_mod != st_mod:
            mismatches.append(
                {
                    "rel": rel,
                    "field": "module_name",
                    "easybuild": eb_mod,
                    "eb_stack": st_mod,
                }
            )

        eb_vs = truth.get("versionsuffix", "")
        st_vs = stack.get("versionsuffix", "")
        if eb_vs != st_vs:
            mismatches.append(
                {
                    "rel": rel,
                    "field": "versionsuffix",
                    "easybuild": eb_vs,
                    "eb_stack": st_vs,
                }
            )

        eb_exts = truth.get("exts_count")
        if eb_exts is None:
            eb_exts = len(truth.get("exts_list", []))
        st_exts = stack.get("exts_count", 0)
        if eb_exts != st_exts:
            mismatches.append(
                {
                    "rel": rel,
                    "field": "exts_count",
                    "easybuild": eb_exts,
                    "eb_stack": st_exts,
                }
            )

        if "generated_module_name" in truth:
            mismatches.extend(compare_generated_module(rel, truth, stack))

        # External modules have no toolchain, so they pair only with a stack
        # dependency that says it is external. The marker is required: an
        # identity match alone would hide an external module eb-stack
        # resolved to an ordinary in-tree build.
        stack_deps: list[dict] = []
        stack_external: list[dict] = []
        for dep in stack.get("deps", []):
            kind = str(dep.get("kind") or "")
            if not isinstance(dep.get("external_module"), bool):
                mismatches.append(
                    {
                        "rel": rel,
                        "field": f"{kind}_dep:{dep.get('name')}:missing_external_marker",
                        "easybuild": None,
                        "eb_stack": dep.get("resolved_module"),
                    }
                )
            (stack_external if _is_external(dep) else stack_deps).append(dep)
        external_truth: dict[str, list[dict]] = {"runtime": [], "build": []}
        truth_by_kind: dict[str, list[dict]] = {"runtime": [], "build": []}
        for kind, field in (("runtime", "dependencies"), ("build", "builddependencies")):
            for dep in truth.get(field, []):
                if _is_external(dep):
                    external_truth[kind].append(dep)
                    continue
                truth_by_kind[kind].append(dep)

        for kind, deps in external_truth.items():
            for dep in deps:
                if any(
                    other.get("kind") == kind and _same_dep(dep, other) for other in stack_external
                ):
                    continue
                in_tree = [
                    other.get("resolved_module")
                    for other in stack_deps
                    if other.get("kind") == kind and _same_dep(dep, other)
                ]
                mismatches.append(
                    {
                        "rel": rel,
                        "field": f"{kind}_dep:{dep['name']}:"
                        + ("external_resolved_in_tree" if in_tree else "external_missing"),
                        "easybuild": dep.get("resolved_module") or dep["name"],
                        "eb_stack": in_tree,
                    }
                )

        def _stack_for(kind: str, dep: dict) -> list[dict]:
            return [
                other
                for other in stack_deps
                if other.get("kind") == kind and _same_dep(dep, other)
            ]

        for kind, deps in truth_by_kind.items():
            for dep in deps:
                matched = _stack_for(kind, dep)
                if not dep.get("toolchain_name"):
                    mismatches.append(
                        {
                            "rel": rel,
                            "field": f"{kind}_dep:{dep['name']}:missing_toolchain",
                            "easybuild": f"missing toolchain_name in recipe {rel}",
                            "eb_stack": [other.get("resolved_module") for other in matched],
                        }
                    )
                    continue
                expected_mod = format_eb_dep_module(dep)
                candidates = [other.get("resolved_module") for other in matched]
                if expected_mod not in candidates:
                    mismatches.append(
                        {
                            "rel": rel,
                            "field": f"{kind}_dep:{dep['name']}",
                            "easybuild": expected_mod,
                            "eb_stack": candidates,
                        }
                    )
        for dep in stack.get("deps", []):
            kind = str(dep.get("kind") or "")
            if _is_external(dep):
                if any(_same_dep(dep, other) for other in external_truth.get(kind, [])):
                    continue
            elif any(_same_dep(dep, other) for other in truth_by_kind.get(kind, [])):
                continue
            elif any(_same_dep(dep, other) for other in external_truth.get(kind, [])):
                # Already reported as external_resolved_in_tree.
                continue
            mismatches.append(
                {
                    "rel": rel,
                    "field": f"extra_{kind}_dep:{dep.get('name')}",
                    "easybuild": [],
                    "eb_stack": [dep.get("resolved_module")],
                }
            )

    return {
        "checked": checked,
        "mismatch_count": len(mismatches),
        "mismatches": mismatches,
    }


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    root = args.easyconfigs.resolve()
    if not root.is_dir():
        raise SystemExit(f"easyconfigs root is not a directory: {root}")

    if args.truth_json is not None:
        truth_doc = json.loads(args.truth_json.read_text(encoding="utf-8"))
        truth_rows = truth_doc["results"]
        rels = [row["rel"] for row in truth_rows]
        reasons = {row["rel"]: row.get("bucket", "truth") for row in truth_rows}
    else:
        add_easybuild_to_path(args.easybuild_framework, args.easybuild_easyblocks, args.pythonpath)
        easyblock_sources = [args.easybuild_easyblocks or installed_easyblocks_root()]
        extra_easyblocks = discover_easyblocks_touching_naming(
            [source for source in easyblock_sources if source is not None]
            + args.include_easyblocks
        )
        rels, reasons = select_sample(
            root=root,
            seed=args.seed,
            sample_size=args.sample_size,
            include_targeted=not args.no_targeted,
            extra_easyblocks=extra_easyblocks,
        )
        truth_doc = extract_easybuild_truth(
            root=root,
            rels=rels,
            reasons=reasons,
            include_easyblocks=args.include_easyblocks,
        )
        truth_rows = truth_doc["results"]
        if args.emit_truth is not None:
            args.emit_truth.write_text(json.dumps(truth_doc, indent=2) + "\n", encoding="utf-8")

    if args.eb_stack_json is not None:
        stack_doc = json.loads(args.eb_stack_json.read_text(encoding="utf-8"))
    else:
        eb_stack_bin = resolve_eb_stack_bin(args.eb_stack)
        stack_doc = run_eb_stack_audit(eb_stack_bin, root, rels)

    report = compare_parity(truth_rows, stack_doc.get("recipes", []))
    report["seed"] = args.seed
    report["sample_size"] = args.sample_size
    report["selected_count"] = len(rels)
    report["targeted_count"] = sum(1 for r in reasons.values() if r.startswith("targeted"))

    if args.out is not None:
        args.out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    print(
        f"parity_audit: seed={report['seed']} "
        f"checked={report['checked']} "
        f"targeted={report['targeted_count']} "
        f"mismatches={report['mismatch_count']}"
    )
    for item in report["mismatches"]:
        print(
            f"  MISMATCH {item['rel']} [{item['field']}]: "
            f"easybuild={item['easybuild']!r} eb-stack={item['eb_stack']!r}"
        )
    return 1 if report["mismatch_count"] > 0 else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
