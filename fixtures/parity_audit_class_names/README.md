# Parity fixture: encoded easyblock class names

Easyconfig and easyblock for the real EasyBuild parity audit
(`parity_audit_targets_easyblocks_by_their_decoded_class_names`).
`FFTW.MPI` uses `EB_FFTW_period_MPI`, whose class name EasyBuild encodes
with `encode_class_name`, so the audit has to decode it the same way to
target the recipe. eb-stack agrees with the generated module, so the
audit must report no mismatch. Excluded from the published `eb_stack`
crate via `Cargo.toml` `exclude = ["fixtures/parity_*/"]`.

`LICENSE` is the GNU General Public License v2.0 text kept with the
other parity fixtures, since the easyblock subclasses EasyBuild's
`ConfigureMake`
(`sha256: 8177f97513213526df2cf6184d8ff986c675afb514d4e68a404010521b880643`).
