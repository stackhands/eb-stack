# Parity fixture: sparse-tree compiler & suffixed subtoolchain hierarchies

Verbatim GPL-2.0-only excerpts vendored from EasyBuild upstream for toolchain
hierarchy parity tests (`tests/toolchain_hierarchy_parity.rs`). Excluded from
the published `eb_stack` crate via `Cargo.toml` `exclude = ["fixtures/parity_*/"]`.

- Upstream `easybuild-easyconfigs` commit `b210e3fdfe940821d74bbbb0d4e0469f16649963`:
  minimal `iimkl-2025b` (no recipe uses `intel-compilers` as its `toolchain`)
  and `rompi-2025a` (`rocm-compilers-19.0.0-ROCm-6.4.1` split version/versionsuffix)
  `get_toolchain_hierarchy` easyconfig closure (27 files)
- Upstream `easybuild-framework` commit `8907031c218b8a331de7c9d411555ead05df0411`:
  `easybuild/toolchains/iimkl.py`, `easybuild/toolchains/compiler/intel_compilers.py`,
  `easybuild/toolchains/rompi.py`, `easybuild/toolchains/compiler/rocm_compilers.py`
- `LICENSE`: verbatim upstream GNU General Public License v2.0 (`sha256: 8177f97513213526df2cf6184d8ff986c675afb514d4e68a404010521b880643`)
