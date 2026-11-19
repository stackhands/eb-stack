# Contributing to eb-stack

Thank you for your interest. eb-stack turns conda-forge, Spack, and EasyBuild
inputs into source-evidenced SBOMs, Resolvo locks, conventional EasyBuild
variants, and persisted build campaigns. Clear, tested, reproducible changes
win.

## Development environment

Rust 1.90 is the minimum supported toolchain; current stable is used for the
main test and lint gates. [pixi](https://pixi.sh) provides docs and optional
dev tasks:

```bash
cargo test --locked --lib
cargo test --locked --test reproduce_real_prs --test bump_emit
prek run -a                   # or: pixi run -e dev prek
pixi run -e dev check         # fmt + clippy + lib tests
pixi run -e docs docbld       # orgmode to rst to Sphinx (Shibuya)
```

Build heavy work on a remote builder when local compiles thrash the machine.

## Code style and gates

- `prek run -a` (trailing whitespace, YAML/TOML, codespell, large files)
- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings` when you touch Rust
- All tests relevant to your change must pass
- New public CLI surface needs a flag in `docs/orgmode/reference/cli.org` and
  either a unit test or an integration test under `tests/`
- Known maintainer-bump regressions live in `tests/reproduce_real_prs.rs` and
  must stay green (library **and** CLI auto-bump)
- Every pull request adds a news fragment, `changelog.d/+<topic>.<type>.md`
  with type `feature`, `bugfix`, `doc`, `removal` or `misc`;
  `towncrier build --draft --version X.Y.Z` shows how it renders

## Documentation

Primary docs live in orgmode under `docs/orgmode/` (Diataxis). Edit there,
then build:

```bash
# once: isolated system-Python environment for the native Rust extension
python3 -m venv .venv-docs
export RUSTFLAGS='-C linker=cc'
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc
.venv-docs/bin/pip install 'sphinx>=9,<10' shibuya sphinx-sitemap \
  sphinx-copybutton sphinx-design 'sphinxcontrib-rust>=1,<2' \
  'sphinx-rustdoc-postprocess>=0.1,<0.2'
cargo install sphinx-rustdocgen   # or use an existing ~/.cargo/bin copy

export EB_STACK_DOCS_PYTHON=$PWD/.venv-docs/bin/python
pixi run -e docs docbld
scripts/check-doc-links.sh docs/build
pixi run -e docs linkcheck
# HTML to docs/build/index.html (includes crates/eb_stack/* Rust API)
```

The Sphinx task treats warnings as errors. The rendered-link check rejects
links to generated RST and incorrect source-edit targets; the Lychee task
checks the rendered site and README links.

`sphinxcontrib-rust` contains a native Rust extension. The explicit linker
environment keeps that extension on the host Rust/C linker rather than a
linker injected by the documentation package environment.

Do not hand-edit generated `docs/source/**/*.rst` or `docs/source/crates/`.

## Packaging campaigns

Follow `skills/annual-bump/SKILL.md` (generation rebuild) and
`skills/new-package/SKILL.md` (conda-forge/Spack: new EasyBuild recipe). Run
`skills/verify-recipe/SKILL.md` over what they emit before a build or a push,
and `skills/tool-repair/SKILL.md` when the emitted file needed a hand
correction: a manual fix that does not return as a regression test is a
defect this tool reproduces on the next run. Keep site hostnames, paths,
credentials, scheduler sizing, and private module names in the site target
layer. The public files contain reusable package intent and target topology.

## Releasing

Version source of truth: `version` in root `Cargo.toml` (currently **0.9.0**).

`cog bump` cuts a release. `cog.toml` runs `tools/release-prepare` before the
bump commit, which builds `CHANGELOG.md` from `changelog.d/` with towncrier,
sets the version in every file that declares or states it, and writes
`llms.txt` and `llms-full.txt` again; the script's header lists the files.
It refuses when one of them no longer names the old version.

1. Branch from `main`: `git switch -c chore/release-X.Y.Z origin/main`.
2. `cog bump --minor` (or `--patch`, `--major`, `--version X.Y.Z`). It runs
   the hook, commits `chore(version): X.Y.Z` and creates the tag on that
   commit. `cog bump --minor --dry-run` prints the version only.
3. Push the branch, open the pull request, and merge it with a merge commit
   once its checks pass, so the tagged commit is on `main`.

`CITATION.cff` gets no `date-released`: `tests/public_metadata.rs` rejects
that line, so the field stays absent until the test is updated for a
published citation.

Then sign the tag cog made and push it:

```bash
git tag -s -f v0.9.0 -m "eb-stack 0.9.0" 'v0.9.0^{}'
git push origin v0.9.0
# .github/workflows/release.yml (cargo-dist) builds the installers
# and publishes the GitHub Release.
# .github/workflows/publish.yml publishes eb_stack to crates.io with Trusted
# Publishing and then registers the MCP server (mcp_registry.yml). It skips a
# version already on crates.io. Until crates.io lists stackhands/eb-stack as a
# Trusted Publisher, publish by hand from the tagged checkout, then run the
# mcp-registry workflow:
cargo publish --locked
```

Verify the binary surface:

```bash
cargo build --release
./target/release/eb-stack --version
# prints eb-stack 0.9.0
```
