use eb_stack::{
    build_order, candidate_matches_dep, check_recipe_deps, module_name, parse_easyconfig_tree,
    resolve_easyconfig_file, resolve_easyconfig_str, resolve_easyconfig_str_for_arch, Choice,
    ModuleKey, Toolchain,
};
use std::path::PathBuf;

#[test]
fn check_recipe_deps_matches_version_plus_versionsuffix_pin() {
    let mcr = resolve_easyconfig_str(
        "name = 'MCR'\n\
         version = 'R2019b'\n\
         local_update = '8'\n\
         versionsuffix = '.%s' % local_update\n\
         toolchain = SYSTEM\n",
    )
    .expect("MCR");
    let freesurfer = resolve_easyconfig_str(
        "name = 'FreeSurfer'\n\
         version = '7.3.2'\n\
         versionsuffix = '-centos7_x86_64'\n\
         toolchain = SYSTEM\n\
         dependencies = [('MCR', 'R2019b.8')]\n",
    )
    .expect("FreeSurfer");

    let mcr_cand = mcr.to_candidate();
    assert!(
        candidate_matches_dep(&mcr_cand, &freesurfer.dependencies[0]),
        "candidate_matches_dep must match MCR R2019b + .8 against R2019b.8"
    );

    let check = check_recipe_deps(&freesurfer, &[mcr_cand]);
    assert!(
        check.ok(),
        "check_recipe_deps must resolve FreeSurfer -> MCR/R2019b.8: {:?}",
        check.missing
    );

    let python_bare = resolve_easyconfig_str(
        "name = 'Python'\n\
         version = '3.12.3'\n\
         versionsuffix = '-bare'\n\
         toolchain = {'name': 'GCCcore', 'version': '13.3.0'}\n",
    )
    .expect("Python-bare");
    let app = resolve_easyconfig_str(
        "name = 'App'\n\
         version = '1.0'\n\
         toolchain = {'name': 'GCCcore', 'version': '13.3.0'}\n\
         dependencies = [('Python', '3.12.3')]\n",
    )
    .expect("App");
    assert!(
        !candidate_matches_dep(&python_bare.to_candidate(), &app.dependencies[0]),
        "plain dependency ('Python', '3.12.3') must still reject -bare"
    );
}

#[test]
fn list_append_accepts_trailing_comma() {
    let src = "\
name = 'Extrae'
version = '4.2.0'
toolchain = {'name': 'gompi', 'version': '2023b'}
builddependencies = [('Automake', '1.16.5')]
dependencies = [('zlib', '1.2.13')]
if ARCH != 'aarch64':
    dependencies.append(
        ('libunwind', '1.6.2'),
    )
builddependencies.append(('CMake', '3.27.6'),)
";
    let parsed = resolve_easyconfig_str_for_arch(src, "x86_64").expect("parse Extrae");
    let dep_names: Vec<&str> = parsed
        .dependencies
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    let bdep_names: Vec<&str> = parsed
        .builddependencies
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(dep_names, vec!["zlib", "libunwind"]);
    assert_eq!(bdep_names, vec!["Automake", "CMake"]);
}
#[test]
fn real_pennylane_conditional_augassign_resolves_cuquantum() {
    let ec_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("parity_pennylane_cond")
        .join("easyconfigs");
    let cands = parse_easyconfig_tree(&ec_root).unwrap().candidates;
    let pennylane_eb = ec_root.join("p/PennyLane/PennyLane-0.37.0-foss-2023a-CUDA-12.1.1.eb");
    let pennylane_resolved = resolve_easyconfig_file(&pennylane_eb).unwrap();
    let pennylane = pennylane_resolved.to_candidate();
    assert_eq!(
        module_name(&ModuleKey::of(&pennylane)),
        "PennyLane/0.37.0-foss-2023a-CUDA-12.1.1"
    );

    let cuquantum_resolved = pennylane_resolved
        .dependencies
        .iter()
        .find(|d| d.name == "cuQuantum")
        .expect("PennyLane must keep conditional += cuQuantum dependency");
    let cuquantum_cand = cands
        .iter()
        .find(|c| module_name(&ModuleKey::of(c)) == "cuQuantum/24.08.0.5-CUDA-12.1.1")
        .expect("cuQuantum candidate in parity_pennylane_cond");
    assert!(
        candidate_matches_dep(cuquantum_cand, cuquantum_resolved),
        "cuQuantum/24.08.0.5-CUDA-12.1.1 must satisfy PennyLane cuQuantum ResolvedDep"
    );

    let cuquantum_dep = pennylane
        .dependencies
        .iter()
        .find(|d| d.name == "cuQuantum")
        .expect("cuQuantum DepReq");
    assert_eq!(
        cuquantum_dep.toolchain,
        Some(Toolchain {
            name: "system".into(),
            version: "system".into(),
        })
    );
    assert_eq!(cuquantum_dep.versionsuffix.as_deref(), Some("-CUDA-12.1.1"));
}

#[test]
fn real_sentieon_system_suffixed_version_req_resolves_jemalloc() {
    let ec_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("parity_sentieon_system_dep")
        .join("easyconfigs");
    let cands = parse_easyconfig_tree(&ec_root).unwrap().candidates;
    let sentieon_eb = ec_root.join("s/sentieon-genomics/sentieon-genomics-202503.02.eb");
    let sentieon = resolve_easyconfig_file(&sentieon_eb).unwrap();
    let sentieon_cand = sentieon.to_candidate();
    assert_eq!(
        module_name(&ModuleKey::of(&sentieon_cand)),
        "sentieon-genomics/202503.02"
    );

    let jemalloc_cand = cands
        .iter()
        .find(|c| c.name == "jemalloc")
        .expect("jemalloc candidate");
    assert!(candidate_matches_dep(
        jemalloc_cand,
        &sentieon.dependencies[0]
    ));
    assert_eq!(
        jemalloc_cand.toolchain,
        Toolchain {
            name: "GCCcore".into(),
            version: "13.3.0".into(),
        }
    );

    let order = build_order(&cands, &["sentieon-genomics".into()], Choice::Newest)
        .expect("build_order for sentieon-genomics");
    let jemalloc_idx = order
        .iter()
        .position(|c| module_name(&ModuleKey::of(c)) == "jemalloc/5.3.0-GCCcore-13.3.0")
        .expect("jemalloc/5.3.0-GCCcore-13.3.0 in build_order");
    let sentieon_idx = order
        .iter()
        .position(|c| module_name(&ModuleKey::of(c)) == "sentieon-genomics/202503.02")
        .expect("sentieon-genomics/202503.02 in build_order");
    assert!(jemalloc_idx < sentieon_idx);
}

#[test]
fn real_sentieon_full_module_pin_passes_recipe_check() {
    let ec_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("parity_sentieon_system_dep")
        .join("easyconfigs");
    let cands = parse_easyconfig_tree(&ec_root).unwrap().candidates;
    let sentieon = resolve_easyconfig_file(
        &ec_root.join("s/sentieon-genomics/sentieon-genomics-202503.02.eb"),
    )
    .unwrap();

    let check = check_recipe_deps(&sentieon, &cands);
    assert!(
        check.missing.is_empty(),
        "('jemalloc', '5.3.0-GCCcore-13.3.0') spells a module build_order resolves: {:?}",
        check.missing
    );
    assert_eq!(
        check.found,
        vec!["runtime:jemalloc-5.3.0-GCCcore-13.3.0 (GCCcore-13.3.0)".to_string()]
    );
}
