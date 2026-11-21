# Parity fixture: easyblock preparation

Easyconfigs and easyblocks for the real EasyBuild parity audit
(`parity_audit_prepares_targeted_easyblocks_like_easybuild`). Each
easyblock writes its module only after the preparation `eb` does before
`make_module_step`: `HookTemplate` resolves `%(installdir)s`, which
`update_config_template_run_step` defines; `HookParallel` reads the
parallelism `post_init` sets; `HookInstalled` refuses a missing
installation unless `--module-only --force` is set, as `EB_GROMACS` does.
eb-stack agrees with all three modules, so the audit must report no
mismatch. Excluded from the published `eb_stack` crate via `Cargo.toml`
`exclude = ["fixtures/parity_*/"]`.

`LICENSE` is the GNU General Public License v2.0 text kept with the
other parity fixtures, since the easyblocks subclass EasyBuild's
`ConfigureMake`
(`sha256: 8177f97513213526df2cf6184d8ff986c675afb514d4e68a404010521b880643`).
