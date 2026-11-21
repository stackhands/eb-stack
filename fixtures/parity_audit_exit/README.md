# Parity fixture: easyblock exit

An easyconfig and easyblock for the real EasyBuild parity audit
(`parity_audit_fails_when_an_easyblock_exits`). `HookExit` uses an
easyblock whose `make_module_step` raises `SystemExit(0)`. The audit
must record that as an EasyBuild error and exit non-zero, not stop
with status 0 and no report. Excluded from the published `eb_stack`
crate via `Cargo.toml` `exclude = ["fixtures/parity_*/"]`.

`LICENSE` is the GNU General Public License v2.0 text kept with the
other parity fixtures, since the easyblock subclasses EasyBuild's
`ConfigureMake`
(`sha256: 8177f97513213526df2cf6184d8ff986c675afb514d4e68a404010521b880643`).
