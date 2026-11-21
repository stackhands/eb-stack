use eb_stack::{hierarchy_for_with_tree, parse_easyconfig_tree, solve_from_easyconfigs, Toolchain};
use std::fs;
use std::path::{Path, PathBuf};

fn write_eb(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn intel_and_lfoss_tree(root: &Path) {
    write_eb(
        root,
        "g/GCCcore/GCCcore-14.3.0.eb",
        "name = 'GCCcore'\nversion = '14.3.0'\ntoolchain = SYSTEM\n",
    );
    write_eb(
        root,
        "z/zlib/zlib-1.3.1-GCCcore-14.3.0.eb",
        "name = 'zlib'\nversion = '1.3.1'\ntoolchain = {'name': 'GCCcore', 'version': '14.3.0'}\n",
    );
    write_eb(
        root,
        "i/intel-compilers/intel-compilers-2025.2.0.eb",
        "easyblock = 'Toolchain'\nname = 'intel-compilers'\nversion = '2025.2.0'\ntoolchain = SYSTEM\ndependencies = [('GCCcore', '14.3.0')]\n",
    );
    write_eb(
        root,
        "i/impi/impi-2021.16.1-intel-compilers-2025.2.0.eb",
        "name = 'impi'\nversion = '2021.16.1'\ntoolchain = {'name': 'intel-compilers', 'version': '2025.2.0'}\n",
    );
    write_eb(
        root,
        "i/imkl/imkl-2025.2.0.eb",
        "name = 'imkl'\nversion = '2025.2.0'\ntoolchain = SYSTEM\n",
    );
    write_eb(
        root,
        "i/iimpi/iimpi-2025b.eb",
        "easyblock = 'Toolchain'\nname = 'iimpi'\nversion = '2025b'\ntoolchain = SYSTEM\ndependencies = [('GCCcore', '14.3.0'), ('intel-compilers', '2025.2.0'), ('impi', '2021.16.1', '', ('intel-compilers', '2025.2.0'))]\n",
    );
    write_eb(
        root,
        "i/iimkl/iimkl-2025b.eb",
        "easyblock = 'Toolchain'\nname = 'iimkl'\nversion = '2025b'\ntoolchain = SYSTEM\ndependencies = [('intel-compilers', '2025.2.0'), ('imkl', '2025.2.0', '', SYSTEM)]\n",
    );
    write_eb(
        root,
        "i/imkl-FFTW/imkl-FFTW-2025.2.0-iimpi-2025b.eb",
        "name = 'imkl-FFTW'\nversion = '2025.2.0'\ntoolchain = {'name': 'iimpi', 'version': '2025b'}\n",
    );
    write_eb(
        root,
        "i/intel/intel-2025b.eb",
        "easyblock = 'Toolchain'\nname = 'intel'\nversion = '2025b'\ntoolchain = SYSTEM\ndependencies = [\n  ('GCCcore', '14.3.0'),\n  ('intel-compilers', '2025.2.0'),\n  ('impi', '2021.16.1', '', ('intel-compilers', '2025.2.0')),\n  ('imkl', '2025.2.0', '', SYSTEM),\n  ('imkl-FFTW', '2025.2.0', '', ('iimpi', '2025b')),\n]\n",
    );
    write_eb(
        root,
        "s/SciPy-bundle/SciPy-bundle-2025.07-iimkl-2025b.eb",
        "name = 'SciPy-bundle'\nversion = '2025.07'\ntoolchain = {'name': 'iimkl', 'version': '2025b'}\n",
    );
    write_eb(
        root,
        "a/ASAP3/ASAP3-3.13.11-intel-2025b.eb",
        "name = 'ASAP3'\nversion = '3.13.11'\ntoolchain = {'name': 'intel', 'version': '2025b'}\ndependencies = [('SciPy-bundle', '2025.07')]\n",
    );

    write_eb(
        root,
        "l/llvm-compilers/llvm-compilers-20.1.8.eb",
        "easyblock = 'Toolchain'\nname = 'llvm-compilers'\nversion = '20.1.8'\ntoolchain = SYSTEM\ndependencies = [('GCCcore', '14.3.0')]\n",
    );
    write_eb(
        root,
        "o/OpenMPI/OpenMPI-5.0.8-llvm-compilers-20.1.8.eb",
        "name = 'OpenMPI'\nversion = '5.0.8'\ntoolchain = {'name': 'llvm-compilers', 'version': '20.1.8'}\n",
    );
    write_eb(
        root,
        "f/FlexiBLAS/FlexiBLAS-3.4.5-llvm-compilers-20.1.8.eb",
        "name = 'FlexiBLAS'\nversion = '3.4.5'\ntoolchain = {'name': 'llvm-compilers', 'version': '20.1.8'}\n",
    );
    write_eb(
        root,
        "f/FFTW/FFTW-3.3.10-llvm-compilers-20.1.8.eb",
        "name = 'FFTW'\nversion = '3.3.10'\ntoolchain = {'name': 'llvm-compilers', 'version': '20.1.8'}\n",
    );
    write_eb(
        root,
        "l/lompi/lompi-2025b.eb",
        "easyblock = 'Toolchain'\nname = 'lompi'\nversion = '2025b'\ntoolchain = SYSTEM\ndependencies = [('llvm-compilers', '20.1.8'), ('OpenMPI', '5.0.8', '', ('llvm-compilers', '20.1.8'))]\n",
    );
    write_eb(
        root,
        "l/lfbf/lfbf-2025b.eb",
        "easyblock = 'Toolchain'\nname = 'lfbf'\nversion = '2025b'\ntoolchain = SYSTEM\ndependencies = [('llvm-compilers', '20.1.8'), ('FlexiBLAS', '3.4.5', '', ('llvm-compilers', '20.1.8')), ('FFTW', '3.3.10', '', ('llvm-compilers', '20.1.8'))]\n",
    );
    write_eb(
        root,
        "l/lfoss/lfoss-2025b.eb",
        "easyblock = 'Toolchain'\nname = 'lfoss'\nversion = '2025b'\ntoolchain = SYSTEM\ndependencies = [\n  ('llvm-compilers', '20.1.8'),\n  ('OpenMPI', '5.0.8', '', ('llvm-compilers', '20.1.8')),\n  ('FlexiBLAS', '3.4.5', '', ('llvm-compilers', '20.1.8')),\n  ('FFTW', '3.3.10', '', ('llvm-compilers', '20.1.8')),\n  ('ScaLAPACK', '2.2.2', '', ('lompi', '2025b')),\n]\n",
    );
    write_eb(
        root,
        "s/ScaLAPACK/ScaLAPACK-2.2.2-lompi-2025b.eb",
        "name = 'ScaLAPACK'\nversion = '2.2.2'\ntoolchain = {'name': 'lompi', 'version': '2025b'}\n",
    );
    write_eb(
        root,
        "n/networkx/networkx-3.5-lfbf-2025b.eb",
        "name = 'networkx'\nversion = '3.5'\ntoolchain = {'name': 'lfbf', 'version': '2025b'}\n",
    );
}

#[test]
fn intel_hierarchy_includes_iimkl_and_solves_iimkl_deps() {
    let tmp = tempfile::tempdir().unwrap();
    let ec_root = tmp.path().join("easyconfigs");
    intel_and_lfoss_tree(&ec_root);
    let cands = parse_easyconfig_tree(&ec_root).unwrap().candidates;

    let intel_h = hierarchy_for_with_tree(
        &Toolchain {
            name: "intel".into(),
            version: "2025b".into(),
        },
        None,
        &cands,
    )
    .unwrap();
    assert_eq!(
        intel_h.member_labels(),
        vec![
            "system",
            "GCCcore-14.3.0",
            "intel-compilers-2025.2.0",
            "iimkl-2025b",
            "iimpi-2025b",
            "intel-2025b",
        ]
    );

    let iimpi_h = hierarchy_for_with_tree(
        &Toolchain {
            name: "iimpi".into(),
            version: "2025b".into(),
        },
        None,
        &cands,
    )
    .unwrap();
    assert_eq!(
        iimpi_h.member_labels(),
        vec![
            "system",
            "GCCcore-14.3.0",
            "intel-compilers-2025.2.0",
            "iimpi-2025b",
        ]
    );

    let policy_path = tmp.path().join("policy.json");
    let lock_path = tmp.path().join("lock.json");
    fs::write(
        &policy_path,
        r#"{"toolchain":{"name":"intel","version":"2025b"},"roots":["ASAP3"]}"#,
    )
    .unwrap();
    let lock = solve_from_easyconfigs(&[&ec_root], &policy_path, None, &lock_path, None).unwrap();
    assert_eq!(lock.package("SciPy-bundle").unwrap().version, "2025.07");
}

#[test]
fn lfoss_hierarchy_includes_lfbf_and_orders_gcccore_first() {
    let tmp = tempfile::tempdir().unwrap();
    let ec_root = tmp.path().join("easyconfigs");
    intel_and_lfoss_tree(&ec_root);
    let cands = parse_easyconfig_tree(&ec_root).unwrap().candidates;

    let lfoss_h = hierarchy_for_with_tree(
        &Toolchain {
            name: "lfoss".into(),
            version: "2025b".into(),
        },
        None,
        &cands,
    )
    .unwrap();
    assert_eq!(
        lfoss_h.member_labels(),
        vec![
            "system",
            "GCCcore-14.3.0",
            "llvm-compilers-20.1.8",
            "lfbf-2025b",
            "lompi-2025b",
            "lfoss-2025b",
        ]
    );
}

#[test]
fn gfbf_hierarchy_excludes_gompi() {
    let tmp = tempfile::tempdir().unwrap();
    let ec_root = tmp.path().join("easyconfigs");
    write_eb(
        &ec_root,
        "g/GCC/GCC-14.3.0.eb",
        "name = 'GCC'\nversion = '14.3.0'\ntoolchain = SYSTEM\ndependencies = [('GCCcore', '14.3.0')]\n",
    );
    write_eb(
        &ec_root,
        "g/gompi/gompi-2025b.eb",
        "easyblock = 'Toolchain'\nname = 'gompi'\nversion = '2025b'\ntoolchain = SYSTEM\ndependencies = [('GCC', '14.3.0')]\n",
    );
    write_eb(
        &ec_root,
        "g/gfbf/gfbf-2025b.eb",
        "easyblock = 'Toolchain'\nname = 'gfbf'\nversion = '2025b'\ntoolchain = SYSTEM\ndependencies = [('GCC', '14.3.0')]\n",
    );
    let cands = parse_easyconfig_tree(&ec_root).unwrap().candidates;

    let gfbf_h = hierarchy_for_with_tree(
        &Toolchain {
            name: "gfbf".into(),
            version: "2025b".into(),
        },
        None,
        &cands,
    )
    .unwrap();
    assert_eq!(
        gfbf_h.member_labels(),
        vec!["system", "GCCcore-14.3.0", "GCC-14.3.0", "gfbf-2025b"]
    );
}

#[test]
fn real_foss_2022a_sibling_order_matches_framework_subtoolchain() {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("parity_foss_subtoolchain");
    let ec_root = fixture_dir.join("easyconfigs");
    let fw_root = fixture_dir.join("framework");

    let foss_py = fs::read_to_string(fw_root.join("easybuild/toolchains/foss.py")).unwrap();
    assert!(foss_py.contains("SUBTOOLCHAIN = [Gompi.NAME, Golf.NAME, Gfbf.NAME]"));

    let cands = parse_easyconfig_tree(&ec_root).unwrap().candidates;
    let h = hierarchy_for_with_tree(
        &Toolchain {
            name: "foss".into(),
            version: "2022a".into(),
        },
        None,
        &cands,
    )
    .unwrap();
    assert_eq!(
        h.bare_tuple_labels(),
        vec![
            "GCCcore-11.3.0",
            "GCC-11.3.0",
            "gfbf-2022a",
            "gompi-2022a",
            "foss-2022a",
        ]
    );
}

#[test]
fn real_sparse_iimkl_and_rompi_hierarchies_keep_compiler_and_gcccore_levels() {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("parity_sparse_compiler_hierarchy");
    let ec_root = fixture_dir.join("easyconfigs");
    let fw_root = fixture_dir.join("framework");

    let iimkl_py = fs::read_to_string(fw_root.join("easybuild/toolchains/iimkl.py")).unwrap();
    let ic_py =
        fs::read_to_string(fw_root.join("easybuild/toolchains/compiler/intel_compilers.py"))
            .unwrap();
    let rompi_py = fs::read_to_string(fw_root.join("easybuild/toolchains/rompi.py")).unwrap();
    let rc_py = fs::read_to_string(fw_root.join("easybuild/toolchains/compiler/rocm_compilers.py"))
        .unwrap();
    assert!(iimkl_py.contains("self.SUBTOOLCHAIN = IntelCompilersToolchain.NAME"));
    assert!(ic_py.contains("COMPILER_MODULE_NAME = ['intel-compilers']"));
    assert!(rompi_py.contains("SUBTOOLCHAIN = ROCmCompilersToolchain.NAME"));
    assert!(rc_py.contains("SUBTOOLCHAIN = SYSTEM_TOOLCHAIN_NAME"));

    let cands = parse_easyconfig_tree(&ec_root).unwrap().candidates;

    let iimkl_h = hierarchy_for_with_tree(
        &Toolchain {
            name: "iimkl".into(),
            version: "2025b".into(),
        },
        None,
        &cands,
    )
    .unwrap();
    assert_eq!(
        iimkl_h.bare_tuple_labels(),
        vec!["GCCcore-14.3.0", "intel-compilers-2025.2.0", "iimkl-2025b",]
    );

    let rompi_h = hierarchy_for_with_tree(
        &Toolchain {
            name: "rompi".into(),
            version: "2025a".into(),
        },
        None,
        &cands,
    )
    .unwrap();
    assert_eq!(
        rompi_h.bare_tuple_labels(),
        vec![
            "GCCcore-14.2.0",
            "rocm-compilers-19.0.0-ROCm-6.4.1",
            "rompi-2025a",
        ]
    );
}
