# Parity fixture: foss-2022a SUBTOOLCHAIN sibling order

Verbatim GPL-2.0-only excerpts vendored from EasyBuild upstream for toolchain
hierarchy parity tests (`tests/toolchain_hierarchy_parity.rs`). Excluded from
the published `eb_stack` crate via `Cargo.toml` `exclude = ["fixtures/parity_*/"]`.

- Upstream `easybuild-easyconfigs` commit `b210e3fdfe940821d74bbbb0d4e0469f16649963`:
  minimal `foss-2022a` `get_toolchain_hierarchy` easyconfig closure (32 files)
- Upstream `easybuild-framework` commit `8907031c218b8a331de7c9d411555ead05df0411`:
  `easybuild/toolchains/foss.py` (`SUBTOOLCHAIN = [Gompi.NAME, Golf.NAME, Gfbf.NAME]`)
- `LICENSE`: verbatim upstream GNU General Public License v2.0 (`sha256: 8177f97513213526df2cf6184d8ff986c675afb514d4e68a404010521b880643`)
