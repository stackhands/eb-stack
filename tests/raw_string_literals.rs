use eb_stack::resolve_easyconfig_str_reporting;

#[test]
fn exts_list_with_raw_string_keeps_all_extensions() {
    let src = r#"
easyblock = 'PythonBundle'
name = 'PyStan'
version = '3.10.0'
toolchain = {'name': 'foss', 'version': '2025a'}
dependencies = [('Python', '3.13.1')]
exts_list = [
    ('clikit', '0.6.2', {
        'preinstallopts': r"sed -i 's/\^0.3.0/>=0.3.0/' pyproject.toml && ",
    }),
    ('marshmallow', '3.26.1'),
    ('webargs', '8.7.0'),
    ('httpstan', '4.13.0'),
    ('pysimdjson', '7.0.2'),
    (name, version, {
        'modulename': 'stan',
    }),
]
"#;
    let (ec, skipped) = resolve_easyconfig_str_reporting(src).unwrap();
    assert!(
        skipped.is_empty(),
        "expected no skipped statements, got {skipped:?}"
    );
    assert_eq!(ec.exts_list.len(), 6);
    assert_eq!(ec.exts_list[0].name, "clikit");
    assert_eq!(ec.exts_list[5].name, "PyStan");
    assert_eq!(ec.exts_list[5].version, "3.10.0");
}

#[test]
fn raw_and_rf_strings_preserve_backslashes_and_interpolate() {
    let src = r#"
name = 'dorado'
version = '2.1.2'
toolchain = {'name': 'foss', 'version': '2025a'}
local_tag = 'OpenSSL'
preconfigopts = r"sed -i 's/a\nb/c/' CMakeLists.txt && "
configopts = rf"-DFLAG={local_tag}\path"
description = r"""multi\nline"""
"#;
    let (ec, skipped) = resolve_easyconfig_str_reporting(src).unwrap();
    assert!(
        skipped.is_empty(),
        "expected no skipped statements, got {skipped:?}"
    );
    assert_eq!(ec.configopts.as_deref(), Some(r"-DFLAG=OpenSSL\path"));
}
