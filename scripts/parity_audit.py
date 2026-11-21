#!/usr/bin/env python3
"""Reproducible parity audit between EasyBuild and ``eb-stack``.

Selects a deterministic stratified sample across an easyconfigs tree
(seeded via ``--seed``) plus a targeted set of recipes using easyblocks
that touch module naming (``ModuleRC``, ``Toolchain``, ``Bundle``,
``Amber``, ``versionprefix``, ``modaltsoftname``, and any easyblock in
``--easybuild-easyblocks`` overriding ``det_full_module_name`` or using
``modaltsoftname``).

Compares ``eb-stack recipe parity-audit`` against EasyBuild (or a
pre-generated ``--truth-json`` oracle) across parse status, dropped
``exts_list``, ``module_name``, ``versionsuffix``, ``exts_count``, and
nearest-hierarchy-level dependency resolution. Exits 0 on full parity
and 1 on any mismatch or error.
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
_ASSIGN_RE = re.compile(
    r"^\s*(easyblock|versionprefix|modaltsoftname|exts_list|dependencies|builddependencies)\s*=",
    re.MULTILINE,
)
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


def _class_touches_naming_or_hooks(cls_node: ast.ClassDef) -> bool:
    for base in cls_node.bases:
        if isinstance(base, ast.Name) and base.id == "Toolchain":
            return True
        if isinstance(base, ast.Attribute) and base.attr == "Toolchain":
            return True

    for node in ast.walk(cls_node):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            if (
                node.name in _MODULE_NAMING_METHODS
                or node.name in _VERSION_TEMPLATE_METHODS
                or node.name in _TOOLCHAIN_HOOK_METHODS
                or "toolchain" in node.name.lower()
            ):
                return True
        elif isinstance(node, ast.Attribute) and node.attr in _NAMING_TEMPLATE_ATTRS:
            return True
        elif isinstance(node, ast.Name) and node.id in _NAMING_TEMPLATE_ATTRS:
            return True
        elif isinstance(node, ast.Constant) and isinstance(node.value, str):
            if node.value in _NAMING_TEMPLATE_ATTRS:
                return True
    return False


def discover_easyblocks_touching_naming(easyblocks_root: Path | None) -> list[str]:
    if easyblocks_root is None or not easyblocks_root.is_dir():
        return []
    matched: set[str] = set()
    for path in sorted(easyblocks_root.rglob("*.py")):
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
            if _class_touches_naming_or_hooks(node):
                cls = node.name
                matched.add(cls)
                if cls.startswith("EB_"):
                    matched.add(cls[3:])
                    matched.add(_decode_eb_class_name(cls))
    return sorted(matched)


def collect_easyconfig_files(root: Path) -> list[tuple[str, str]]:
    files: list[tuple[str, str]] = []
    for path in sorted(root.rglob("*.eb")):
        rel = path.relative_to(root).as_posix()
        if "/__archive__/" in f"/{rel}":
            continue
        text = path.read_text(encoding="utf-8", errors="ignore")
        files.append((rel, text))
    return files


def classify_recipe(rel: str, text: str, extra_easyblocks: set[str]) -> tuple[str, bool]:
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

    is_targeted = (
        easyblock in TARGETED_EASYBLOCKS
        or easyblock in extra_easyblocks
        or (not easyblock and software_name in extra_easyblocks)
        or bool(re.search(r"^\s*version_?prefix\s*=", text, re.MULTILINE))
        or bool(re.search(r"^\s*(?:modaltsoftname|env_mod_alt_soft_name)\s*=", text, re.MULTILINE))
    )

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
    return bucket, is_targeted


def select_sample(
    root: Path,
    seed: int,
    sample_size: int,
    include_targeted: bool,
    extra_easyblocks: list[str],
) -> tuple[list[str], dict[str, str]]:
    files = collect_easyconfig_files(root)
    extra_set = set(extra_easyblocks)
    buckets: dict[str, list[str]] = {}
    targeted: list[str] = []
    reasons: dict[str, str] = {}

    for rel, text in files:
        bucket, is_targeted = classify_recipe(rel, text, extra_set)
        buckets.setdefault(bucket, []).append(rel)
        if include_targeted and is_targeted:
            targeted.append(rel)
            reasons[rel] = "targeted"

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


def extract_easybuild_truth(
    root: Path,
    rels: list[str],
    reasons: dict[str, str],
    framework_dir: Path | None,
    easyblocks_dir: Path | None,
    extra_pythonpath: list[str],
) -> dict:
    for entry in reversed(extra_pythonpath):
        if entry and entry not in sys.path:
            sys.path.insert(0, entry)
    for extra_dir in (easyblocks_dir, framework_dir):
        if extra_dir is not None:
            resolved = str(extra_dir.resolve())
            if resolved not in sys.path:
                sys.path.insert(0, resolved)

    try:
        from easybuild.framework.easyconfig import easyconfig as eb_easyconfig
        from easybuild.framework.easyconfig.easyconfig import EasyConfig
        from easybuild.tools import modules as eb_modules
        from easybuild.tools.module_naming_scheme.easybuild_mns import EasyBuildMNS
    except ImportError as exc:
        raise SystemExit(
            "EasyBuild is not importable; pass --truth-json or --easybuild-framework. "
            f"Import error: {exc}"
        ) from exc

    try:
        from easybuild.tools.options import set_up_configuration

        set_up_configuration(
            args=[f"--robot={root}"],
            silent=True,
        )
    except Exception:
        pass

    if hasattr(eb_modules, "NoModulesTool") and hasattr(eb_easyconfig, "modules_tool"):
        orig_modules_tool = eb_easyconfig.modules_tool

        def _safe_modules_tool(*args, **kwargs):
            try:
                return orig_modules_tool(*args, **kwargs)
            except Exception:
                return eb_modules.NoModulesTool(testing=True)

        eb_easyconfig.modules_tool = _safe_modules_tool

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
            results.append(
                {
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

        st_deps_by_key: dict[tuple[str, str], list[str | None]] = {}
        for dep in stack.get("deps", []):
            key = (dep["kind"], dep["name"])
            st_deps_by_key.setdefault(key, []).append(dep.get("resolved_module"))

        for kind, field in (("runtime", "dependencies"), ("build", "builddependencies")):
            for dep in truth.get(field, []):
                if dep.get("external_module"):
                    continue
                if not dep.get("toolchain_name") and dep.get("toolchain_name") != "":
                    continue
                expected_mod = format_eb_dep_module(dep)
                candidates = st_deps_by_key.get((kind, dep["name"]), [])
                if expected_mod not in candidates:
                    mismatches.append(
                        {
                            "rel": rel,
                            "field": f"{kind}_dep:{dep['name']}",
                            "easybuild": expected_mod,
                            "eb_stack": candidates,
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

    extra_easyblocks = discover_easyblocks_touching_naming(args.easybuild_easyblocks)

    if args.truth_json is not None:
        truth_doc = json.loads(args.truth_json.read_text(encoding="utf-8"))
        truth_rows = truth_doc["results"]
        rels = [row["rel"] for row in truth_rows]
        reasons = {row["rel"]: row.get("bucket", "truth") for row in truth_rows}
    else:
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
            framework_dir=args.easybuild_framework,
            easyblocks_dir=args.easybuild_easyblocks,
            extra_pythonpath=args.pythonpath,
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
    report["targeted_count"] = sum(1 for r in reasons.values() if r == "targeted")

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
