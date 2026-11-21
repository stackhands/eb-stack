# Parity fixture: framework dependency resolution

Minimal easyconfigs for the real EasyBuild parity audit
(`parity_audit_resolves_with_easybuild_framework`). One recipe builds
against GCCcore and depends on zlib, so dependency resolution runs.
Another names a dependency whose toolchain is empty, so the audit's
missing-toolchain check runs. Excluded from the published `eb_stack`
crate via `Cargo.toml` `exclude = ["fixtures/parity_*/"]`.

`LICENSE` is the GNU General Public License v2.0 text kept with the
other parity fixtures
(`sha256: 8177f97513213526df2cf6184d8ff986c675afb514d4e68a404010521b880643`).
