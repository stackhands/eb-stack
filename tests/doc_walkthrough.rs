//! The get-started page, the tutorial, how-tos, reference, and explanation
//! pages are commands a checkout can run.
//!
//! Bash blocks must not point at a path this repository does not have.
//! The report commands and the get-started blocks are executed, and the
//! page must quote their output.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn org_files() -> Vec<PathBuf> {
    let mut files = vec![
        root().join("docs/orgmode/get-started.org"),
        root().join("docs/orgmode/tutorial.org"),
    ];
    for dir in ["howto", "reference", "explanation"] {
        let path = root().join("docs/orgmode").join(dir);
        let mut names: Vec<_> = std::fs::read_dir(&path)
            .unwrap_or_else(|_| panic!("{dir}"))
            .map(|entry| entry.expect("entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "org"))
            .collect();
        names.sort();
        files.append(&mut names);
    }
    files
}

fn bash_blocks(text: &str) -> Vec<String> {
    org_blocks(text, "#+begin_src bash\n", "#+end_src")
}

fn example_blocks(text: &str) -> Vec<String> {
    org_blocks(text, "#+begin_example\n", "#+end_example")
}

fn org_blocks(text: &str, open: &str, close: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        rest = &rest[start + open.len()..];
        let end = rest
            .find(close)
            .unwrap_or_else(|| panic!("{close} missing"));
        blocks.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    blocks
}

const FORBIDDEN: &[&str] = &[
    "/path/to",
    "/cvmfs/",
    "/workspace/",
    "site.toml",
    "<sha256",
    "SHA256",
    "vendor/easybuild",
    "CAMPAIGN.json",
    "BASE.toml",
    "SITE.toml",
    "App-1.0",
    "eb-stack/scripts",
    "/shared/",
    "Snel",
];

#[test]
fn howto_bash_blocks_name_files_in_this_checkout() {
    for path in org_files() {
        let text = std::fs::read_to_string(&path).expect("read");
        let rel = path.strip_prefix(root()).unwrap_or(&path);
        for word in ["ships", "landed", "blocked on", "settle"] {
            assert!(!text.contains(word), "{} contains {word:?}", rel.display());
        }
        for block in bash_blocks(&text) {
            for token in FORBIDDEN {
                assert!(
                    !block.contains(token),
                    "{} bash block contains {token:?}:\n{block}",
                    rel.display()
                );
            }
            for word in block.split_whitespace() {
                let word = word
                    .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | ',' | '\\' | '(' | ')'));
                if word.contains(['*', '$', '~', '{']) {
                    continue;
                }
                let Some(rel_path) = word
                    .strip_prefix("fixtures/")
                    .or_else(|| word.strip_prefix("tests/"))
                    .or_else(|| word.strip_prefix("examples/"))
                    .or_else(|| word.strip_prefix("skills/"))
                    .or_else(|| word.strip_prefix("scripts/"))
                else {
                    continue;
                };
                let _ = rel_path;
                let full = root().join(word);
                assert!(full.exists(), "{} names missing path {word}", rel.display());
            }
        }
    }
}

#[test]
fn get_started_blocks_print_what_the_page_quotes() {
    let page = std::fs::read_to_string(root().join("docs/orgmode/get-started.org")).unwrap();
    let commands = bash_blocks(&page);
    let quoted = example_blocks(&page);
    assert_eq!(commands.len(), quoted.len(), "one quoted output per block");
    // The page writes under /tmp; each run gets its own directory instead.
    let tmp = tempfile::tempdir().expect("temp");
    let scratch = format!("{}/", tmp.path().display());
    let bin = Path::new(env!("CARGO_BIN_EXE_eb-stack")).parent().unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    for (command, expected) in commands.iter().zip(&quoted) {
        let output = Command::new("bash")
            .current_dir(root())
            .env("PATH", &path)
            .args(["-euo", "pipefail", "-c"])
            .arg(command.replace("/tmp/", &scratch))
            .output()
            .expect("spawn bash");
        let stdout = String::from_utf8_lossy(&output.stdout).replace(&scratch, "/tmp/");
        assert_eq!(
            &stdout,
            expected,
            "{command}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let status = if command.contains("eb-stack stack admit") {
            78
        } else {
            0
        };
        assert_eq!(output.status.code(), Some(status), "{command}");
    }
    assert!(page.contains("The exit status is 78."));
    let reservations = tmp.path().join("eb-stack-docs/admit-no");
    assert_eq!(std::fs::read_dir(reservations).unwrap().count(), 0);
}

#[test]
fn report_howtos_quote_the_fixture_commands() {
    let tmp = tempfile::tempdir().expect("temp");
    let overlap = tmp.path().join("overlap");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "pr-overlap",
            "--pr",
            "tests/pr_overlap_layer/prs.json",
            "--easyconfigs",
            "tests/pr_overlap_layer",
            "--installed",
            "tests/pr_overlap_layer/installed.txt",
            "--installed-label",
            "none",
            "--generated-at",
            "2026-10-24T00:00:00Z",
            "--repo-url",
            "https://github.com/EESSI/software-layer",
            "--eessi-version",
            "2026.06",
            "--merge-order",
            "--out-dir",
        ])
        .arg(&overlap)
        .output()
        .expect("pr-overlap");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let stdout = String::from_utf8(status.stdout).unwrap();
    assert!(
        stdout.contains("prs=2 records=2 shared_modules=91"),
        "{stdout}"
    );
    let order = std::fs::read_to_string(overlap.join("merge-order.toml")).unwrap();
    assert!(order.contains("position = 1\npr = 1585\n"), "{order}");
    assert!(order.contains("pr = 1728\n"), "{order}");
    assert!(order.contains("after = [1585]"), "{order}");
    let page = std::fs::read_to_string(root().join("docs/orgmode/howto/pr-overlap.org")).unwrap();
    assert!(page.contains("prs=2 records=2 shared_modules=91"), "{page}");
    assert!(page.contains("tests/pr_overlap_layer/prs.json"));
    assert!(page.contains("prs=4 records=4 shared_modules=1"));
    assert!(page.contains("tests/pr_overlap_easyconfigs_open/prs.json"));

    let easy = tmp.path().join("easy");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "pr-overlap",
            "--kind",
            "easyconfigs",
            "--pr",
            "tests/pr_overlap_easyconfigs_open/prs.json",
            "--easyconfigs",
            "tests/pr_overlap_easyconfigs_open/tree",
            "--generated-at",
            "2026-10-24T00:00:00Z",
            "--repo-url",
            "https://github.com/easybuilders/easybuild-easyconfigs",
            "--out-dir",
        ])
        .arg(&easy)
        .output()
        .expect("easyconfigs overlap");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let got = std::fs::read_to_string(easy.join("overlap.md")).unwrap();
    let want = std::fs::read_to_string(
        root().join("tests/pr_overlap_easyconfigs_open/expected/overlap.md"),
    )
    .unwrap();
    assert_eq!(got, want);

    let reach = tmp.path().join("reach");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "component-reach",
            "--component",
            "curl",
            "--version",
            ">=8,<9",
            "--generation",
            "2026.1",
            "--easyconfigs",
            "tests/component_reach_recorded",
            "--out-dir",
        ])
        .arg(&reach)
        .output()
        .expect("component-reach");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let stdout = String::from_utf8(status.stdout).unwrap();
    assert!(
        stdout.contains("carriers=2 changed=1 must-rebuild=1 review=0 no-rebuild=1"),
        "{stdout}"
    );
    let page =
        std::fs::read_to_string(root().join("docs/orgmode/howto/component-reach.org")).unwrap();
    assert!(page.contains("carriers=2 changed=1 must-rebuild=1 review=0 no-rebuild=1"));
    assert!(page.contains("tests/component_reach_recorded"));
    assert!(page.contains("981d2123dddccca9ce0b15f9eb2643e872332f5e"));

    let rebuild = tmp.path().join("rebuild");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "rebuild-set",
            "--changed",
            "tests/rebuild_set_fixtures/changed.txt",
            "--easystack",
            "tests/rebuild_set_fixtures/eessi-2026.06-eb-5.4.0-2026.1.yml",
            "--easystack",
            "tests/rebuild_set_fixtures/20261001-eb-5.4.1-zlib.yml",
            "--easyconfigs",
            "tests/rebuild_set_fixtures/release",
            "--easyblocks",
            "tests/rebuild_set_fixtures/easyblocks",
            "--installed",
            "tests/rebuild_set_fixtures/installed.txt",
            "--installed-modulerc",
            "tests/rebuild_set_fixtures/modules/all",
            "--installed-label",
            "x86_64/amd/zen4",
            "--generated-at",
            "2026-10-22T12:00:00Z",
            "--out-dir",
        ])
        .arg(&rebuild)
        .output()
        .expect("rebuild-set");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let stdout = String::from_utf8(status.stdout).unwrap();
    assert!(
        stdout.contains("changed=2 rows=8 must-rebuild=2 review=2 no-rebuild=4 missing=1"),
        "{stdout}"
    );
    let page = std::fs::read_to_string(root().join("docs/orgmode/howto/rebuild-set.org")).unwrap();
    assert!(page.contains("changed=2 rows=8 must-rebuild=2 review=2 no-rebuild=4 missing=1"));

    let list = tmp.path().join("site.list");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .args([
            "stack",
            "solve",
            "--easyconfigs",
            "fixtures/gromacs_2025_to_next/easyconfigs",
            "--toolchain",
            "foss/2025b",
            "--root",
            "GROMACS",
            "--baseline-easystack",
            "fixtures/gromacs_2025_to_next/easystacks/gromacs-2024.4.yml",
            "--build-list-out",
        ])
        .arg(&list)
        .arg("--build-list-excludes-baseline")
        .arg("--stack-diff-out")
        .arg(tmp.path().join("easystack.diff.md"))
        .arg("--lock-out")
        .arg(tmp.path().join("easystack.lock.json"))
        .status()
        .expect("solve");
    assert!(status.success());
    let list = std::fs::read_to_string(&list).unwrap();
    let page = std::fs::read_to_string(root().join("docs/orgmode/howto/emit-reports.org")).unwrap();
    assert!(page.contains(list.trim_end()), "{list}");
    assert!(page.contains("Baseline (`foss-2025b`) to solved ("));
    assert!(!page.contains("): solved ("));
}

#[test]
fn foreign_plan_howtos_use_the_fixture_robots() {
    let tmp = tempfile::tempdir().expect("temp");
    let bs4 = tmp.path().join("bs4");
    let status = run_to(
        &[
            "package",
            "plan",
            "--source",
            "fixtures/foreign_ingest/pypi_bs4/pypi.json",
            "--format",
            "pypi",
            "--toolchain",
            "GCCcore/15.2.0",
            "--easyconfigs",
            "fixtures/foreign_ingest/pypi_bs4/robot",
            "--stack-policy",
            "examples/stacks/eessi-python-gcccore.toml",
            "--out-dir",
        ],
        &bs4,
    );
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let recipe = std::fs::read_to_string(
        bs4.join("easyconfigs/b/beautifulsoup4/beautifulsoup4-4.12.3-GCCcore-15.2.0.eb"),
    )
    .unwrap();
    assert!(recipe.contains("easyblock = 'PythonPackage'"), "{recipe}");
    assert!(!recipe.contains("exts_list"), "{recipe}");
    assert!(
        recipe.contains("dependencies = [('Python', '3.14.2'), ('Python-bundle-PyPI', '2026.04')]"),
        "{recipe}"
    );
    let page = std::fs::read_to_string(root().join("docs/orgmode/howto/pypi-extend.org")).unwrap();
    assert!(page.contains("fixtures/foreign_ingest/pypi_bs4/robot"));
    assert!(page.contains("easyblock = 'PythonPackage'") || page.contains("~PythonPackage~"));
    // EESSI installs pure-Python packages on GCCcore, so the page plans bs4 there.
    for command in [
        "eb-stack package inspect \\\n  --source fixtures/foreign_ingest/pypi_bs4/pypi.json \\\n  \
         --format pypi \\\n  --toolchain GCCcore/15.2.0 \\\n",
        "eb-stack package plan \\\n  --source fixtures/foreign_ingest/pypi_bs4/pypi.json \\\n  \
         --format pypi \\\n  --toolchain GCCcore/15.2.0 \\\n  \
         --easyconfigs fixtures/foreign_ingest/pypi_bs4/robot \\\n  \
         --stack-policy examples/stacks/eessi-python-gcccore.toml \\\n",
    ] {
        assert!(page.contains(command), "pypi-extend.org lacks:\n{command}");
    }
    assert!(page.contains("~Python 3.14.2~") && page.contains("~Python-bundle-PyPI 2026.04~"));
    let skill = std::fs::read_to_string(root().join("skills/package-plan/SKILL.md")).unwrap();
    for line in [
        "  --toolchain GCCcore/15.2.0 \\\n",
        "  --stack-policy examples/stacks/eessi-python-gcccore.toml \\\n",
        "beautifulsoup4/beautifulsoup4-4.12.3-GCCcore-15.2.0.eb\n",
    ] {
        assert!(skill.contains(line), "package-plan skill lacks {line:?}");
    }

    let cran = tmp.path().join("cran");
    let status = run_to(
        &[
            "package",
            "plan",
            "--source",
            "fixtures/foreign_ingest/cran_jsonlite/cran.json",
            "--format",
            "cran",
            "--toolchain",
            "foss/2026.1",
            "--easyconfigs",
            "fixtures/foreign_ingest/cran_jsonlite/robot",
            "--stack-policy",
            "examples/stacks/eessi-r-extras.toml",
            "--source-checksum",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "--out-dir",
        ],
        &cran,
    );
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let recipe =
        std::fs::read_to_string(cran.join("easyconfigs/j/jsonlite/jsonlite-1.8.8-foss-2026.1.eb"))
            .unwrap();
    assert!(recipe.contains("easyblock = 'Bundle'"), "{recipe}");
    assert!(
        recipe.contains("exts_defaultclass = 'RPackage'"),
        "{recipe}"
    );

    let missing = run_to(
        &[
            "package",
            "plan",
            "--source",
            "fixtures/foreign_ingest/cran_unpinned/cran.json",
            "--format",
            "cran",
            "--toolchain",
            "foss/2026.1",
            "--easyconfigs",
            "fixtures/foreign_ingest/cran_bundle/robot",
            "--stack-policy",
            "examples/stacks/eessi-r-extras.toml",
            "--source-checksum",
            "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "--out-dir",
        ],
        &tmp.path().join("missing"),
    );
    assert!(!missing.status.success());
    let err = String::from_utf8_lossy(&missing.stderr);
    assert!(err.contains("processx is not in the robot"), "{err}");
    let page = std::fs::read_to_string(root().join("docs/orgmode/howto/cran-extend.org")).unwrap();
    assert!(page.contains("processx is not in the robot"));
    assert!(
        page.contains("easyblock = RPackage")
            || page.contains("~easyblock = RPackage~")
            || page.contains("~RPackage~")
    );
}

#[test]
fn reference_pages_do_not_embed_a_checkout_path() {
    for dir in ["docs/orgmode/reference", "docs/orgmode/explanation"] {
        let mut names: Vec<_> = std::fs::read_dir(root().join(dir))
            .unwrap_or_else(|_| panic!("{dir}"))
            .map(|entry| entry.expect("entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "org"))
            .collect();
        names.sort();
        for path in names {
            let text = std::fs::read_to_string(&path).expect("read");
            let rel = path.strip_prefix(root()).unwrap_or(&path);
            for token in ["/workspace", "/tmp/ec-fixture-prs.json"] {
                assert!(!text.contains(token), "{} contains {token}", rel.display());
            }
        }
    }
}

#[test]
fn easyblock_reach_howto_uses_the_pinned_trees() {
    let page =
        std::fs::read_to_string(root().join("docs/orgmode/howto/easyblock-reach.org")).unwrap();
    assert!(page.contains("scripts/fetch-upstream-trees.sh"));
    assert!(page.contains("EB_EASYBLOCKS"));
    assert!(page.contains("EB_EASYBUILD_FRAMEWORK"));
    assert!(!page.contains("~/src"));
    assert!(!page.contains("-b develop"));
    assert!(!page.contains("git clone"));
    assert!(page.contains("easyconfigs=2352 installed=0 classes=39 reverse=5058"));
    assert!(page.contains("easyconfigs=0 installed=0 classes=0 reverse=0"));
}

/// `sphinx-rustdocgen` joins a `#[path = "..."]` value onto the parent file,
/// then opens that path. A module file has to be `name.rs` or `name/mod.rs`
/// next to its parent.
#[test]
fn rust_modules_use_files_the_api_generator_can_open() {
    fn walk(dir: &Path) {
        for entry in std::fs::read_dir(dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path);
                continue;
            }
            if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).expect("read");
                assert!(
                    !text.contains("#[path"),
                    "{} uses #[path]; sphinx-rustdocgen cannot open that module",
                    path.display()
                );
            }
        }
    }
    walk(&root().join("src"));
}

/// `repair-campaign.org` quotes the doctor error the binary prints.
#[test]
fn the_repair_howto_quotes_the_doctor_error_it_prints() {
    let tmp = tempfile::tempdir().expect("temp");
    let out = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .args([
            "target",
            "doctor",
            "--config",
            "examples/targets/missing-easybuild.toml",
            "--target",
            "missing-eb",
        ])
        .env("TMPDIR", tmp.path())
        .output()
        .expect("run eb-stack");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let page =
        std::fs::read_to_string(root().join("docs/orgmode/howto/repair-campaign.org")).unwrap();
    let quoted = page
        .split("#+begin_example\n")
        .nth(1)
        .and_then(|rest| rest.split("#+end_example").next())
        .unwrap()
        .trim()
        .to_string();
    assert!(stderr.contains(&quoted), "page: {quoted}\nstderr: {stderr}");
}

/// Serves the Kaleido x86_64 wheel and nothing else, so aarch64 and riscv64
/// are `arch-missing`, the shape of software-layer #1670.
fn kaleido_server() -> (String, std::thread::JoinHandle<()>, std::net::SocketAddr) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let worker = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buffer = [0u8; 4096];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]);
            let mut first = request.lines().next().unwrap_or("").split_whitespace();
            let (method, path) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
            if method == "STOP" {
                break;
            }
            let status = match (method, path) {
                ("HEAD", "/kaleido-0.2.1-manylinux1-x86_64.whl") => "200 OK",
                ("GET", "/kaleido-0.2.1-manylinux1-x86_64.whl") => "206 Partial Content",
                _ => "404 Not Found",
            };
            let _ = stream.write_all(
                format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            );
        }
    });
    (format!("http://{addr}"), worker, addr)
}

/// `arch-unsupported.org` quotes the source-probe output with the server
/// address written as the fixture's `__BASE_URL__`: first with no `--arch`,
/// then with `--arch aarch64`, then with `--arch x86_64 --arch aarch64`.
#[test]
fn the_source_probe_howto_quotes_the_output_it_prints() {
    use std::io::Write;
    let (base_url, worker, addr) = kaleido_server();
    let tmp = tempfile::tempdir().expect("temp");
    let recipe_rel = "k/Kaleido/Kaleido-0.2.1-GCCcore-15.2.0.eb";
    let fixture = root().join("tests/source_probe_fixtures/easyconfigs");
    let tree = tmp.path().join("easyconfigs");
    std::fs::create_dir_all(tree.join("k/Kaleido")).unwrap();
    let recipe = std::fs::read_to_string(fixture.join(recipe_rel)).unwrap();
    std::fs::write(
        tree.join(recipe_rel),
        recipe.replace("__BASE_URL__", &base_url),
    )
    .unwrap();
    let probe = |arches: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
        command.current_dir(root()).args(["stack", "arch"]);
        for arch in arches {
            command.args(["--arch", arch]);
        }
        let out = command
            .args([
                "--easystack",
                "tests/source_probe_fixtures/easystacks/kaleido.yml",
                "--easyconfigs",
            ])
            .arg(&tree)
            .arg("--probe-sources")
            .output()
            .expect("run eb-stack");
        assert_eq!(
            out.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .replace(&base_url, "__BASE_URL__")
    };
    let every_arch = probe(&[]);
    let aarch64 = probe(&["aarch64"]);
    let two = probe(&["x86_64", "aarch64"]);
    if let Ok(mut stop) = std::net::TcpStream::connect(addr) {
        let _ = stop.write_all(b"STOP / HTTP/1.1\r\n\r\n");
    }
    let _ = worker.join();
    let page =
        std::fs::read_to_string(root().join("docs/orgmode/howto/arch-unsupported.org")).unwrap();
    let section = page
        .split("* Per-architecture source probe\n")
        .nth(1)
        .expect("source probe section");
    assert!(
        section.contains("--easyconfigs tests/source_probe_fixtures/easyconfigs"),
        "{section}"
    );
    assert!(section.contains("  --arch aarch64 \\\n"), "{section}");
    assert!(section.contains("  --arch x86_64 \\\n"), "{section}");
    let quoted: Vec<&str> = section
        .split("#+begin_example\n")
        .skip(1)
        .filter_map(|rest| rest.split("#+end_example").next())
        .collect();
    assert_eq!(quoted.len(), 3, "{section}");
    assert_eq!(
        quoted[0], every_arch,
        "page:\n{}\nstdout:\n{every_arch}",
        quoted[0]
    );
    assert_eq!(
        quoted[1], aarch64,
        "page:\n{}\nstdout:\n{aarch64}",
        quoted[1]
    );
    assert_eq!(quoted[2], two, "page:\n{}\nstdout:\n{two}", quoted[2]);
    assert!(quoted.iter().all(|text| !text.contains("127.0.0.1")));
}

fn shell_blocks(text: &str, fence: bool) -> Vec<String> {
    if fence {
        let mut blocks = Vec::new();
        for lang in ["sh", "bash", "shell"] {
            blocks.extend(org_blocks(text, &format!("```{lang}\n"), "```"));
        }
        return blocks;
    }
    let mut blocks = Vec::new();
    for lang in ["bash", "sh", "shell"] {
        blocks.extend(org_blocks(
            text,
            &format!("#+begin_src {lang}\n"),
            "#+end_src",
        ));
    }
    blocks
}

/// A line continuation is one backslash. Two of them end the command at the
/// first line when a reader pastes the block.
#[test]
fn shell_blocks_continue_a_line_with_one_backslash() {
    let mut pages = vec![(root().join("README.md"), true)];
    for path in org_files() {
        pages.push((path, false));
    }
    pages.push((root().join("docs/orgmode/index.org"), false));
    pages.push((root().join("docs/orgmode/install.org"), false));
    for (path, fence) in pages {
        let text = std::fs::read_to_string(&path).expect("read");
        let rel = path.strip_prefix(root()).unwrap_or(&path);
        for block in shell_blocks(&text, fence) {
            for line in block.lines() {
                assert!(
                    !line.trim_end().ends_with("\\\\"),
                    "{} has a shell line that ends in two backslashes:\n{line}",
                    rel.display()
                );
            }
        }
    }
}

/// The capacity page names the release that first carried both commands, and
/// its two commands run in a checkout.
#[test]
fn capacity_reference_commands_run_in_a_checkout() {
    let page = std::fs::read_to_string(root().join("docs/orgmode/reference/planning-capacity.org"))
        .unwrap();
    let changelog = std::fs::read_to_string(root().join("CHANGELOG.md")).unwrap();
    let added = changelog
        .split("\n## ")
        .find(|section| section.contains("- `stack capacity` reads"))
        .and_then(|section| section.split_whitespace().next())
        .expect("the changelog section that adds stack capacity");
    assert!(
        page.contains(&format!("release {added} and")),
        "the page does not name release {added}, which added stack capacity"
    );
    assert!(!page.contains("release binary\ndoes not provide"));

    let tmp = tempfile::tempdir().expect("temp");
    let scratch = format!("{}/", tmp.path().display());
    let bin = Path::new(env!("CARGO_BIN_EXE_eb-stack")).parent().unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let blocks = shell_blocks(&page, false);
    assert_eq!(blocks.len(), 2, "capacity and shard commands");
    let mut stdout = Vec::new();
    for command in &blocks {
        let output = Command::new("bash")
            .current_dir(root())
            .env("PATH", &path)
            .args(["-euo", "pipefail", "-c"])
            .arg(command.replace("/tmp/", &scratch))
            .output()
            .expect("spawn bash");
        assert!(
            output.status.success(),
            "{command}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        stdout.push(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let capacity: serde_json::Value = serde_json::from_str(&stdout[0]).expect("capacity JSON");
    assert_eq!(capacity["queue_fitting_nodes"], 1);
    assert_eq!(capacity["free_slots"], 1);
    assert!(tmp.path().join("capacity-demo").is_dir());
}

fn run_to(args: &[&str], out: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .args(args)
        .arg(out)
        .output()
        .expect("spawn")
}
