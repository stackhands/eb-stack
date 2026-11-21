# Parity fixture: easyblock hooks

Easyconfigs and easyblocks for the real EasyBuild parity audit
(`parity_audit_compares_modules_generated_by_targeted_easyblocks`).
`HookLoad` uses an easyblock whose `make_module_dep` adds a module load
the recipe does not declare. `HookVersion` uses an easyblock whose
`make_module_step` changes the version written to the module. eb-stack
reads neither hook, so the audit must report both. Excluded from the
published `eb_stack` crate via `Cargo.toml`
`exclude = ["fixtures/parity_*/"]`.

`LICENSE` is the GNU General Public License v2.0 text kept with the
other parity fixtures, since the easyblocks subclass EasyBuild's
`ConfigureMake`
(`sha256: 8177f97513213526df2cf6184d8ff986c675afb514d4e68a404010521b880643`).
