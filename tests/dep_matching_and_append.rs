use eb_stack::{
    candidate_matches_dep, check_recipe_deps, resolve_easyconfig_str,
    resolve_easyconfig_str_for_arch,
};

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
