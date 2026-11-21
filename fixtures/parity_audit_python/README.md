# Parity fixture: PythonPackage module generation

A minimal easyconfig for the real EasyBuild parity audit
(`parity_audit_runs_python_easyblocks_with_its_own_interpreter`).
`PythonPackage.make_module_extra` asks a bare `python` for its library
directory (`det_pylibdir`), so writing this recipe's module runs
whichever `python` comes first on `PATH`. The audit must bind that name
to its own interpreter. Excluded from the published `eb_stack` crate
via `Cargo.toml` `exclude = ["fixtures/parity_*/"]`.

`LICENSE` is the GNU General Public License v2.0 text kept with the
other parity fixtures
(`sha256: 8177f97513213526df2cf6184d8ff986c675afb514d4e68a404010521b880643`).
