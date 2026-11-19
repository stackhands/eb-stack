//! Name the toolchains of a build list that have no `buildenv` module.
//!
//! A program compiled by hand against a stack links correctly only through a
//! build environment of its toolchain: `buildenv/default-<toolchain>` sets the
//! compiler variables and puts the RPATH wrappers first on `PATH`. A stack
//! gets one for the toolchains somebody thought of. `stack buildenv` reads the
//! build lists, collects every toolchain a line builds with, and reports the
//! ones that neither a line of the lists nor an installed-module listing
//! covers, with the line that would add each.

use crate::cli_parse::usage;
use crate::easystack::filename_toolchain;
use crate::jenkins::parse_build_list;
use crate::shard::parse_lines;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// The EasyBuild name of the build environment recipes.
const BUILDENV: &str = "buildenv";
/// The version every `buildenv` recipe of the community repository carries.
const BUILDENV_VERSION: &str = "default";

/// What `stack buildenv` reads.
#[derive(Debug, Clone, Default)]
pub struct BuildenvRequest {
    /// Build lists, one easyconfig per line with optional `eb` flags.
    pub build_lists: Vec<PathBuf>,
    /// Installed-module listings, `Name/Version` per line.
    pub installed: Vec<PathBuf>,
    /// Print JSON.
    pub json: bool,
    /// Report a missing build environment as a failure.
    pub strict: bool,
}

/// Where the build environment of a toolchain comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// An installed-module listing has the module.
    Installed,
    /// A line of the build lists builds it.
    Listed,
    /// Neither.
    Missing,
}

/// One toolchain of the build lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolchainRow {
    /// Toolchain label, `foss-2026.1`.
    pub toolchain: String,
    /// Lines of the lists that build with it, the build environments excluded.
    pub lines: usize,
    /// The first such line, as `<list>:<line number>`.
    pub first: String,
    /// The module of its build environment, `buildenv/default-foss-2026.1`.
    pub module: String,
    /// The easyconfig that builds the module.
    pub easyconfig: String,
    /// Where the build environment comes from.
    pub source: Source,
}

/// The report of one run.
#[derive(Debug, Clone, Serialize)]
pub struct BuildenvReport {
    /// One row per toolchain, by name.
    pub toolchains: Vec<ToolchainRow>,
    /// How many rows are `missing`.
    pub missing: usize,
    /// The printed form.
    #[serde(skip)]
    pub stdout: String,
    #[serde(skip)]
    strict: bool,
}

impl BuildenvReport {
    /// The message of a failed run: a toolchain lacks its build environment
    /// and `--strict` was given.
    pub fn failure(&self) -> Option<String> {
        (self.strict && self.missing > 0).then(|| {
            format!(
                "{} of {} toolchains have no buildenv module",
                self.missing,
                self.toolchains.len()
            )
        })
    }
}

/// The module names of one installed-module listing: the first word of each
/// line, without a trailing `.lua`, comments and blank lines left out.
fn installed_modules(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| line.split('#').next()?.split_whitespace().next())
        .map(|word| word.strip_suffix(".lua").unwrap_or(word).to_string())
        .collect()
}

/// Run `stack buildenv`.
pub fn run_buildenv(args: &BuildenvRequest) -> Result<BuildenvReport> {
    if args.build_lists.is_empty() {
        return Err(usage("stack buildenv needs --build-list"));
    }
    let mut installed = BTreeSet::new();
    for path in &args.installed {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read installed-module listing {}", path.display()))?;
        installed.extend(installed_modules(&text));
    }
    // toolchain -> (lines, first line)
    let mut used: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut listed = BTreeSet::new();
    for path in &args.build_lists {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read build list {}", path.display()))?;
        for shard_line in parse_lines(&text) {
            let Some(line) = parse_build_list(&shard_line.text).into_iter().next() else {
                continue;
            };
            let Some(found) = filename_toolchain(&line.file) else {
                continue;
            };
            if found.name == BUILDENV {
                listed.insert(found.toolchain);
                continue;
            }
            let at = format!("{}:{}", path.display(), shard_line.number);
            used.entry(found.toolchain)
                .and_modify(|(count, _)| *count += 1)
                .or_insert((1, at));
        }
    }
    let toolchains: Vec<ToolchainRow> = used
        .into_iter()
        .map(|(toolchain, (lines, first))| {
            let module = format!("{BUILDENV}/{BUILDENV_VERSION}-{toolchain}");
            let source = if installed.contains(&module) {
                Source::Installed
            } else if listed.contains(&toolchain) {
                Source::Listed
            } else {
                Source::Missing
            };
            ToolchainRow {
                easyconfig: format!("{BUILDENV}-{BUILDENV_VERSION}-{toolchain}.eb"),
                toolchain,
                lines,
                first,
                module,
                source,
            }
        })
        .collect();
    let missing = toolchains
        .iter()
        .filter(|row| row.source == Source::Missing)
        .count();
    let mut report = BuildenvReport {
        toolchains,
        missing,
        stdout: String::new(),
        strict: args.strict,
    };
    report.stdout = if args.json {
        let mut text = serde_json::to_string_pretty(&report)?;
        text.push('\n');
        text
    } else {
        render_text(&report)
    };
    Ok(report)
}

fn render_text(report: &BuildenvReport) -> String {
    let width = report
        .toolchains
        .iter()
        .map(|row| row.toolchain.len())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for row in &report.toolchains {
        let state = match row.source {
            Source::Installed => format!("installed  {}", row.module),
            Source::Listed => format!("listed     {}", row.easyconfig),
            Source::Missing => format!("missing    add {}", row.easyconfig),
        };
        out.push_str(&format!(
            "{:<width$}  {:>4} {}  {}\n",
            row.toolchain,
            row.lines,
            if row.lines == 1 { "line " } else { "lines" },
            state
        ));
    }
    out.push_str(&format!(
        "{} of {} toolchains have no buildenv module\n",
        report.missing,
        report.toolchains.len()
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(list: &str, installed: Option<&str>, json: bool, strict: bool) -> BuildenvReport {
        let dir = tempfile::tempdir().unwrap();
        let list_path = dir.path().join("site.list");
        std::fs::write(&list_path, list).unwrap();
        let mut request = BuildenvRequest {
            build_lists: vec![list_path],
            json,
            strict,
            ..BuildenvRequest::default()
        };
        if let Some(text) = installed {
            let path = dir.path().join("installed.txt");
            std::fs::write(&path, text).unwrap();
            request.installed.push(path);
        }
        run_buildenv(&request).unwrap()
    }

    const LIST: &str = "\
# a comment
FFTW-3.3.10-GCC-15.2.0.eb
HDF5-1.14.6-gompi-2026.1.eb --include-systems=snel-zen4
HDF5-1.14.6-iimpi-2026.1.eb
VASP6-6.6.1-NVHPC-26.9-CUDA-13.3.0-ACC.eb
QuantumESPRESSO-7.5-foss-2026.1.eb
ELPA-2025.06.001-foss-2026.1.eb
buildenv-default-GCC-15.2.0.eb --from-commit=0123456789abcdef
EasyBuild-5.4.2.eb
";

    fn sources(report: &BuildenvReport) -> Vec<(&str, Source, usize)> {
        report
            .toolchains
            .iter()
            .map(|row| (row.toolchain.as_str(), row.source, row.lines))
            .collect()
    }

    #[test]
    fn each_toolchain_is_installed_listed_or_missing() {
        let report = run(
            LIST,
            Some("buildenv/default-foss-2026.1\nGCC/15.2.0\n"),
            false,
            false,
        );
        assert_eq!(
            sources(&report),
            [
                ("GCC-15.2.0", Source::Listed, 1),
                ("NVHPC-26.9", Source::Missing, 1),
                ("foss-2026.1", Source::Installed, 2),
                ("gompi-2026.1", Source::Missing, 1),
                ("iimpi-2026.1", Source::Missing, 1),
            ]
        );
        assert_eq!(report.missing, 3);
        assert!(report
            .stdout
            .contains("missing    add buildenv-default-NVHPC-26.9.eb"));
        assert!(report
            .stdout
            .ends_with("3 of 5 toolchains have no buildenv module\n"));
        assert!(report.failure().is_none());
    }

    #[test]
    fn a_system_recipe_and_a_build_environment_are_not_users_of_a_toolchain() {
        let report = run(
            "EasyBuild-5.4.2.eb\nbuildenv-default-foss-2026.1.eb\n",
            None,
            false,
            true,
        );
        assert!(report.toolchains.is_empty());
        assert!(report.failure().is_none());
    }

    #[test]
    fn strict_fails_only_while_a_build_environment_is_missing() {
        let missing = run(LIST, None, false, true);
        assert_eq!(
            missing.failure().as_deref(),
            Some("4 of 5 toolchains have no buildenv module")
        );
        let covered = run(
            "FFTW-3.3.10-GCC-15.2.0.eb\n",
            Some("buildenv/default-GCC-15.2.0.lua  # from a directory listing\n"),
            false,
            true,
        );
        assert_eq!(sources(&covered), [("GCC-15.2.0", Source::Installed, 1)]);
        assert!(covered.failure().is_none());
    }

    #[test]
    fn json_carries_the_line_to_add_and_where_the_toolchain_is_first_used() {
        let report = run(LIST, None, true, false);
        let value: serde_json::Value = serde_json::from_str(&report.stdout).unwrap();
        let rows = value["toolchains"].as_array().unwrap();
        let nvhpc = rows
            .iter()
            .find(|row| row["toolchain"] == "NVHPC-26.9")
            .unwrap();
        assert_eq!(nvhpc["source"], "missing");
        assert_eq!(nvhpc["easyconfig"], "buildenv-default-NVHPC-26.9.eb");
        assert_eq!(nvhpc["module"], "buildenv/default-NVHPC-26.9");
        assert!(nvhpc["first"].as_str().unwrap().ends_with("site.list:5"));
        assert_eq!(value["missing"], 4);
    }

    #[test]
    fn no_build_list_is_a_usage_error() {
        let error = run_buildenv(&BuildenvRequest::default()).unwrap_err();
        assert!(error.to_string().contains("--build-list"));
    }
}
