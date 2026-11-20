use eb_stack::{module_name, resolve_easyconfig_str, solve_from_easyconfigs, ModuleKey};
use std::fs;
use std::path::Path;

fn write_eb(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

#[test]
fn toolchain_version_expands_cudaver_from_dependencies() {
    let src = r#"
name = 'ELPA'
version = '2025.06.002'
toolchain = {'name': 'NVHPC', 'version': '25.3-CUDA-%(cudaver)s'}
dependencies = [('CUDA', '12.8.0', '', SYSTEM)]
"#;
    let ec = resolve_easyconfig_str(src).unwrap();
    assert_eq!(ec.toolchain.name, "NVHPC");
    assert_eq!(ec.toolchain.version, "25.3-CUDA-12.8.0");
    assert_eq!(
        module_name(&ModuleKey::of(&ec.to_candidate())),
        "ELPA/2025.06.002-NVHPC-25.3-CUDA-12.8.0"
    );
}

#[test]
fn cudaver_from_toolchain_version_resolves_dep_versionsuffix_and_solves() {
    let src = r#"
name = 'OpenMPI'
version = '5.0.3'
toolchain = {'name': 'NVHPC', 'version': '24.9-CUDA-12.6.0'}
dependencies = [('UCX-CUDA', '1.16.0', '-CUDA-%(cudaver)s')]
"#;
    let ec = resolve_easyconfig_str(src).unwrap();
    assert_eq!(ec.dependencies.len(), 1);
    assert_eq!(
        ec.dependencies[0].versionsuffix.as_deref(),
        Some("-CUDA-12.6.0")
    );

    let tmp = tempfile::tempdir().unwrap();
    let ec_root = tmp.path().join("easyconfigs");
    write_eb(
        &ec_root,
        "g/GCCcore/GCCcore-13.3.0.eb",
        "name = 'GCCcore'\nversion = '13.3.0'\ntoolchain = SYSTEM\n",
    );
    write_eb(
        &ec_root,
        "c/CUDA/CUDA-12.6.0.eb",
        "name = 'CUDA'\nversion = '12.6.0'\ntoolchain = SYSTEM\n",
    );
    write_eb(
        &ec_root,
        "n/nvidia-compilers/nvidia-compilers-24.9-CUDA-12.6.0.eb",
        "name = 'nvidia-compilers'\nversion = '24.9'\nversionsuffix = '-CUDA-%(cudaver)s'\ntoolchain = SYSTEM\ndependencies = [('GCCcore', '13.3.0'), ('CUDA', '12.6.0')]\n",
    );
    write_eb(
        &ec_root,
        "n/NVHPC/NVHPC-24.9-CUDA-12.6.0.eb",
        "easyblock = 'Toolchain'\nname = 'NVHPC'\nversion = '24.9'\nversionsuffix = '-CUDA-%(cudaver)s'\ntoolchain = SYSTEM\ndependencies = [('CUDA', '12.6.0'), ('nvidia-compilers', version, versionsuffix)]\n",
    );
    write_eb(
        &ec_root,
        "u/UCX-CUDA/UCX-CUDA-1.16.0-GCCcore-13.3.0-CUDA-12.6.0.eb",
        "name = 'UCX-CUDA'\nversion = '1.16.0'\nversionsuffix = '-CUDA-%(cudaver)s'\ntoolchain = {'name': 'GCCcore', 'version': '13.3.0'}\ndependencies = [('CUDA', '12.6.0', '', SYSTEM)]\n",
    );
    write_eb(
        &ec_root,
        "o/OpenMPI/OpenMPI-5.0.3-NVHPC-24.9-CUDA-12.6.0.eb",
        src,
    );

    let policy_path = tmp.path().join("policy.json");
    let lock_path = tmp.path().join("lock.json");
    fs::write(
        &policy_path,
        r#"{"toolchain":{"name":"NVHPC","version":"24.9-CUDA-12.6.0"},"roots":["OpenMPI"]}"#,
    )
    .unwrap();
    let lock = solve_from_easyconfigs(&[&ec_root], &policy_path, None, &lock_path, None).unwrap();
    let ucx = lock.package("UCX-CUDA").unwrap();
    assert_eq!(ucx.versionsuffix.as_deref(), Some("-CUDA-12.6.0"));
}
