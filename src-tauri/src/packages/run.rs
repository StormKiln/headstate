use super::model::{Ecosystem, EcosystemReport, Outdated, ProjectReport};
use super::{cargo, detect, swift, terraform, tools, version};
use std::path::Path;

/// Whether this project's Yarn is version 1.
///
/// `yarn outdated` was REMOVED in Yarn 2. On a Berry project it is not
/// even a recognised command -- Yarn reports `Couldn't find a script
/// named "outdated"` and **exits 0**, so the existing error handling
/// never fired and an empty result rendered as "you are up to date".
///
/// There is no non-interactive replacement: `yarn npm outdated` does not
/// exist, and `yarn upgrade-interactive` is a full-screen UI.
///
/// `yarn --version` is resolved per project by Corepack from
/// `packageManager`, so this asks in the project directory rather than
/// assuming one global Yarn.
///
/// Unknown counts as NOT version 1: Berry is the default for anything
/// new, and guessing v1 restores the silent-empty-list failure.
fn yarn_is_v1(bin: &Path, repo: &Path) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .current_dir(repo)
        .env("PATH", tools::child_path(bin))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .split('.')
                .next()?
                .parse::<u32>()
                .ok()
        })
        .is_some_and(|major| major == 1)
}

/// Check one repository, one ecosystem.
///
/// Every failure mode produces an `EcosystemReport` with an `error`
/// rather than an empty list. "No updates" and "the check did not run"
/// are opposite answers, and rendering both as nothing reports the second
/// as good news.
pub fn check(repo: &Path, eco: Ecosystem) -> EcosystemReport {
    // Swift has no command that reports outdated packages.
    //
    // `swift package update --dry-run` exists for a `Package.swift`, but
    // Xcode-managed dependencies -- which is what an iOS app actually
    // has -- have nothing: `xcodebuild -resolvePackageDependencies`
    // resolves and does not diff.
    //
    // Saying so is the point. An empty list would read as "up to date",
    // which is the same inversion a missing tool would produce, and this
    // module exists to refuse it.
    // Terraform answers from a FILE plus the registry, never a command.
    //
    // `terraform providers` reports the constraints (`>= 5.0.0`) and
    // does not diff, so there is nothing to spawn and parse. The lock
    // file carries the resolved versions, and `enrich` fills in the
    // latest asynchronously afterwards.
    //
    // A lock file it could not READ is reported through `error` rather
    // than dropped (#954): `error` is documented as "the check could not
    // run" and was hardcoded `None` here because there is no command to
    // fail, which left every file-read failure beneath it unrepresentable.
    if eco == Ecosystem::Terraform {
        return from_files(eco, terraform::scan(repo));
    }

    // Swift answers from a FILE plus the Git host, never a command.
    //
    // No command reports outdated Xcode-managed dependencies, which is
    // what this used to say and stop at. But Swift packages are Git
    // repositories and their versions are TAGS, so `Package.resolved`
    // plus a tag listing answers the question -- see `packages::swift`.
    if eco == Ecosystem::Swift {
        return from_files(eco, swift::scan(repo));
    }

    // Cargo answers from TWO FILES plus the crates.io index, never a
    // command.
    //
    // `cargo` is certainly installed -- there is a `Cargo.toml` -- but
    // it has no `outdated`. `cargo update --dry-run` reports what the
    // resolver would move to WITHIN the existing constraints, which is a
    // different question from "what is the newest published version",
    // and the subcommands that do answer it (`cargo outdated`,
    // `cargo upgrade`) are third-party installs whose absence would
    // silence the ecosystem. `Cargo.toml` says which crates, `Cargo.lock`
    // says which versions, and `enrich` asks the sparse index.
    if eco == Ecosystem::Cargo {
        return from_files(eco, cargo::scan(repo));
    }

    let fallbacks = tools::fallback_dirs();
    let refs: Vec<&str> = fallbacks.iter().map(String::as_str).collect();
    let Some(mut bin) = tools::find(eco.program(), &refs) else {
        return missing_tool(eco);
    };

    // Yarn Berry has no outdated command, so the CHECK runs through npm.
    //
    // `npm outdated` reads package.json and queries the registry; it
    // does not care which resolver installed the tree, and it does not
    // need npm to have installed anything. Verified on a real Yarn 4.9
    // project: 98 packages reported, correct current/latest.
    //
    // Only the check. The version npm reports as latest is a registry
    // fact and true either way, but the constraint Yarn would WRITE is
    // Yarn's business -- the requested-vs-resolved distinction #409
    // phase 1 established.
    if eco == Ecosystem::Yarn && !yarn_is_v1(&bin, repo) {
        let Some(npm) = tools::find("npm", &refs) else {
            return EcosystemReport {
                ecosystem: eco,
                outdated: Vec::new(),
                // A real "cannot check", not an empty list: this
                // ecosystem's own tool cannot answer and the stand-in is
                // absent.
                error: Some(
                    "Yarn 2+ has no command that reports outdated packages, and npm \
                     -- which can read this project -- was not found."
                        .into(),
                ),
            };
        };
        bin = npm;
    }

    let args: &[&str] = match eco {
        Ecosystem::Npm => &["outdated", "--json"],
        // Same flags either way: `yarn outdated --json` (v1) and
        // `npm outdated --json` take the same arguments and produce
        // output `parse_npm` already handles for both.
        Ecosystem::Yarn => &["outdated", "--json"],
        Ecosystem::Poetry => &["show", "--outdated"],
        Ecosystem::Uv => &["pip", "list", "--outdated", "--format", "json"],
        Ecosystem::Dotnet => &["list", "package", "--outdated"],
        Ecosystem::Cocoapods => &["outdated"],
        // Neither reaches here: both return early above, because
        // neither has a command that answers the question.
        Ecosystem::Terraform => &["version"],
        // Swift never reaches here -- `check` returns early for it,
        // because there is no command that answers the question.
        Ecosystem::Swift => &["--version"],
        // Nor does Cargo: it returns early too, because `cargo` has no
        // subcommand that reports outdated dependencies.
        Ecosystem::Cargo => &["--version"],
    };

    let mut command = std::process::Command::new(&bin);
    command
        .args(args)
        // The tool's own directory goes on the child's PATH: `npm` and
        // `yarn` are `#!/usr/bin/env node` scripts, so finding them is
        // not enough -- the child has to find `node` too.
        .env("PATH", tools::child_path(&bin))
        .current_dir(repo);
    // And a locale, for the one tool that cannot run without one. Both
    // variables, because `LANG` alone loses to an inherited `LC_ALL`.
    if let Some(locale) = child_locale(eco) {
        command.env("LC_ALL", locale).env("LANG", locale);
    }

    let out = match command.output() {
        Ok(o) => o,
        Err(e) => {
            return EcosystemReport {
                ecosystem: eco,
                outdated: Vec::new(),
                error: Some(format!("could not run {}: {e}", eco.program())),
            }
        }
    };

    // EXIT CODE IS NOT THE ANSWER for several of these. `npm outdated`
    // exits 1 when updates EXIST -- the normal case -- so treating
    // non-zero as failure would report nothing on every repository that
    // has something to update. The output is what is parsed; the status
    // is only consulted when there is nothing to parse.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed = parse(&stdout, eco, repo);

    if is_real_failure(parsed.is_empty(), out.status.success(), &stdout) {
        // STDERR FIRST, then stdout. Most of these tools put a failure on
        // stderr, but the two whose refusals reach `is_usage_error` --
        // Yarn Berry and CocoaPods -- write theirs to STDOUT, and reading
        // only stderr would show an empty banner for exactly the cases
        // that function was added to catch.
        let stderr = String::from_utf8_lossy(&out.stderr);
        return EcosystemReport {
            ecosystem: eco,
            outdated: Vec::new(),
            error: Some(failure_message(&stderr, &stdout)),
        };
    }

    EcosystemReport {
        ecosystem: eco,
        outdated: parsed,
        error: None,
    }
}

/// The report for an ecosystem that answers from FILES.
///
/// The whole of #954's fix: `FileScan` can say what it could not read, and
/// this is where that reaches the `error` field the UI has always
/// rendered. The rows that DID read are carried through unchanged -- a
/// partial answer labelled partial beats both a silent truncation and an
/// error page, which is the rule `claude::transcript::Scan::is_partial`
/// states and the reason this does not return early on a failure.
fn from_files(eco: Ecosystem, scan: super::model::FileScan) -> EcosystemReport {
    // `None` when nothing failed, so an unaffected repository reports
    // exactly what it reported before.
    let error = scan.error();
    EcosystemReport {
        ecosystem: eco,
        outdated: scan.outdated,
        error,
    }
}

/// The report for a tool that is not installed.
///
/// An ERROR, never an empty list. "No updates" and "the check did not
/// run" are opposite answers, and rendering the second as the first
/// reports a failure as good news -- which is the worst outcome
/// available here, because it is the one nobody investigates.
fn missing_tool(eco: Ecosystem) -> EcosystemReport {
    EcosystemReport {
        ecosystem: eco,
        outdated: Vec::new(),
        error: Some(format!(
            "{} was not found. A desktop app does not inherit your shell's PATH.",
            eco.program()
        )),
    }
}

/// Whether a non-zero exit really means the check failed.
///
/// It usually does not. `npm outdated` EXITS 1 WHEN UPDATES EXIST --
/// the normal case, and the whole reason anyone runs it. Treating
/// non-zero as failure would report "no updates" on every repository
/// that has some, which is the exact inversion this module is built to
/// avoid.
///
/// So a failure requires nothing parsed AND a non-zero status, plus
/// either no output at all or output that is visibly not a result.
///
/// The "visibly not a result" case is real: `yarn outdated` on Yarn 2+
/// exits 1 and writes `Usage Error: Couldn't find a script named
/// "outdated"` to STDOUT. Requiring empty stdout let that through as
/// zero results, so every Berry project reported "no updates" -- the
/// inversion this function exists to prevent, arriving by a route it
/// did not cover.
fn is_real_failure(nothing_parsed: bool, status_ok: bool, stdout: &str) -> bool {
    let out = stdout.trim();
    nothing_parsed && !status_ok && (out.is_empty() || is_usage_error(out))
}

/// Output that is a tool complaining rather than a result.
///
/// Deliberately narrow: it must not match a legitimate empty result, so
/// this looks for the shape of a CLI's own refusal at the very start of
/// the output, never anywhere within it.
///
/// `[!] ` is CocoaPods' own error marker, and it arrives here for the
/// same reason Yarn Berry's does: `pod outdated` writes
/// `[!] No 'Podfile.lock' found in the project directory` to STDOUT and
/// exits 1, so the "empty stdout" half of `is_real_failure` never fired
/// and a project that cannot be checked at all reported "no updates" --
/// the inversion this module exists to refuse, arriving by the third
/// route it did not cover.
fn is_usage_error(stdout: &str) -> bool {
    // First line only. `starts_with` on the whole string would behave
    // the same for every input seen here -- both reject a match on a
    // later line -- but taking the line explicitly says what is meant
    // and does not depend on that coincidence holding.
    let head = stdout.lines().next().unwrap_or_default().trim_start();
    let plain = strip_ansi(head);
    let lowered = plain.to_ascii_lowercase();
    lowered.starts_with("usage error")
        || lowered.starts_with("unknown syntax error")
        || lowered.starts_with("error: unknown command")
        // CocoaPods. Case-sensitive and including the space, so it is
        // the marker rather than any line that opens with a bracket.
        || plain.starts_with("[!] ")
}

/// Text with its ANSI escape sequences removed.
///
/// These tools colour their output EVEN WHEN REDIRECTED -- Yarn writes
/// `\x1b[31m\x1b[1mUsage Error`, CocoaPods writes `\x1b[33mWARNING` --
/// so a pipe is not enough to be rid of them and neither is `--no-color`,
/// which not all of them have.
///
/// Splits on the escape byte and drops the SGR sequence that opens each
/// fragment AFTER one -- `[`, the numeric parameters, then the `m`. That
/// covers colour sequences, which is all any of these tools emit.
///
/// The TEXT BEFORE the first escape is never touched, which is the whole
/// reason this is not a one-line `split_once('m')`: doing that uniformly
/// turns `bash: pod: command not found` into `mand not found`, because
/// the first `m` it finds is the one in "command". Only a fragment that
/// genuinely begins `\x1b[…m` loses anything, and a malformed one is
/// kept whole rather than swallowed.
fn strip_ansi(text: &str) -> String {
    let mut parts = text.split('\u{1b}');
    // Everything before the first escape is literal text.
    let mut out = parts.next().unwrap_or_default().to_string();
    for part in parts {
        let rest = part.strip_prefix('[').and_then(|p| {
            let end = p.find(|c: char| !c.is_ascii_digit() && c != ';')?;
            // Only `m` terminates a colour sequence. Anything else is
            // some other escape this does not claim to understand, so
            // the fragment is left alone rather than half-eaten.
            (p.as_bytes()[end] == b'm').then(|| &p[end + 1..])
        });
        out.push_str(rest.unwrap_or(part));
    }
    out
}

/// The one line of stderr worth showing when a check failed.
///
/// Not simply the first line, which is what this used to be, because
/// CocoaPods WARNS BEFORE IT FAILS. With no UTF-8 locale it writes four
/// lines about the terminal encoding and then dies in
/// `unicode_normalize`, so the first line told the user to edit their
/// `~/.profile` while the actual failure sat five lines below it -- and
/// it arrived wearing a raw `\x1b[33m`, which the UI printed as text.
///
/// So: skip blank lines and lines that are a tool's own advisory noise,
/// take the first line that remains, and strip the colour codes off it.
/// If every line is noise the first one is shown anyway -- a warning in
/// the banner beats an empty banner, since the check really did fail.
///
/// Narrow on purpose. Only a line that OPENS with `warning:` or `note:`
/// is skipped, never one that merely contains the word: `[!] No
/// 'Podfile.lock' found` and `bash: pod: command not found` are the
/// messages that matter and neither is touched.
///
/// Both streams are considered, stderr first. Yarn Berry and CocoaPods
/// write their refusals to STDOUT, so stderr alone would leave the
/// banner empty for the two cases most likely to reach here.
fn failure_message(stderr: &str, stdout: &str) -> String {
    let lines: Vec<String> = stderr
        .lines()
        .chain(stdout.lines())
        .map(|l| strip_ansi(l).trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    let is_advisory = |l: &String| {
        let lowered = l.to_ascii_lowercase();
        lowered.starts_with("warning:")
            || lowered.starts_with("warning ")
            || lowered.starts_with("note:")
            // The continuation lines of CocoaPods' own warning. Anchored
            // to the whole phrase, not to "consider", so ordinary advice
            // in a real error message is not thrown away.
            || lowered.starts_with("consider adding the following")
            || lowered.starts_with("export lang=")
    };

    lines
        .iter()
        .find(|l| !is_advisory(l))
        .or(lines.first())
        .cloned()
        .unwrap_or_else(|| "the command failed".to_string())
}

/// The locale to hand this ecosystem's child process, if it needs one.
///
/// COCOAPODS ONLY, and it is not cosmetic. A macOS GUI app inherits no
/// `LANG` or `LC_ALL` -- the same missing-environment class as the PATH
/// problem `tools::child_path` exists for -- and CocoaPods does not
/// merely warn about that: `Pod::Config#installation_root` calls
/// `String#unicode_normalize`, which raises
/// `Encoding::CompatibilityError` on an ASCII-8BIT string, so `pod`
/// exits 1 having produced nothing. Measured on this machine with
/// CocoaPods 1.17 and Ruby 4.0.
///
/// `en_US.UTF-8` is what the warning itself asks for. Scoped to the
/// child process: the app's own environment is not touched, and no other
/// tool here is affected, because none of the others reads the locale to
/// decide whether it can normalise a path.
fn child_locale(eco: Ecosystem) -> Option<&'static str> {
    match eco {
        Ecosystem::Cocoapods => Some("en_US.UTF-8"),
        _ => None,
    }
}

/// Every project in a repository, with its ecosystems checked.
///
/// Per PROJECT, not per repository: a repo with a frontend and a backend
/// is two sets of dependencies in two manifests, and flattening them
/// would produce a list where the same package at two versions is one
/// row and the update command is ambiguous.
pub fn check_repo(repo: &Path) -> Vec<ProjectReport> {
    detect::projects(repo)
        .into_iter()
        .map(|p| {
            let dir = std::path::PathBuf::from(&p.path);
            let reports = p.ecosystems.iter().map(|e| check(&dir, *e)).collect();
            ProjectReport {
                path: p.path,
                label: p.label,
                reports,
            }
        })
        .collect()
}

/// Parse a tool's output into rows.
///
/// Separate from `check` so each format can be tested against captured
/// output without running anything -- three of the five have no stable
/// machine-readable contract, so a fixture is the only honest way to
/// pin them.
pub fn parse(stdout: &str, eco: Ecosystem, repo: &Path) -> Vec<Outdated> {
    match eco {
        Ecosystem::Npm | Ecosystem::Yarn => parse_npm(stdout, eco),
        Ecosystem::Uv => parse_uv(stdout),
        Ecosystem::Poetry => parse_poetry(stdout),
        Ecosystem::Dotnet => parse_dotnet(stdout, repo),
        Ecosystem::Cocoapods => parse_cocoapods(stdout),
        // All three handled before any command runs: none has a
        // command that answers the question.
        Ecosystem::Swift | Ecosystem::Terraform | Ecosystem::Cargo => Vec::new(),
    }
}

/// Whether a parsed row is an update at all, rather than a DOWNGRADE
/// the tool mislabelled `latest`.
///
/// The `latest` column is not always newer than what is installed.
/// `npm outdated --json` reported `jsdom` as `current 30.0.1,
/// latest 29.1.1` in a repository where the registry's own `latest`
/// dist-tag was 30.0.1 -- npm appears to report the newest release
/// satisfying an engine constraint it computed, and still labels the
/// column `latest`. Passed straight through, that offered a downgrade
/// as an available update, and Apply would have run
/// `yarn up jsdom@29.1.1`.
///
/// `version::bump` already refused to CLASSIFY that jump, answering
/// `Unknown`, but nothing ever dropped the row -- so it reached the list
/// looking like a package nobody could compare.
///
/// THE LINE, and it is the whole point of this function: only a
/// CONFIDENTLY backwards jump is dropped. A pair that cannot be compared
/// -- a pre-release suffix, a bare revision, an empty string -- keeps
/// its row and shows as `Bump::Unknown`, because "we cannot tell" is a
/// fact the user should see rather than a row to hide. `is_newer`
/// answers `None` for exactly those, and `Some(false)` only when the
/// numeric comparison is confident, so this reads that distinction
/// rather than reconstructing it.
///
/// Applies to the five parsers that read a tool's output. Terraform and
/// Swift rows are not filtered here: both are built with
/// `latest == current` and are filled in later by `registry::enrich`,
/// whose `newest` already sorts semantically. Swift additionally
/// reports revision pins on purpose, and those must keep showing.
fn keep(name: &str, current: &str, latest: &str, eco: Ecosystem) -> bool {
    if version::is_newer(current, latest) != Some(false) {
        return true;
    }
    // Worth noticing: a tool systematically reporting a wrong `latest`
    // is a bug in that tool, and once the row is gone this line is the
    // only trace of it.
    //
    // A package name and two versions only. No repository path and no
    // manifest -- those identify the checkout, and this is a public
    // repository's log.
    log::warn!(
        "{} reported {name} {current} -> {latest} as an update, which is a downgrade; \
         the row was dropped",
        eco.program()
    );
    false
}

/// `{"pkg": {"current": "1.0.0", "latest": "2.0.0", ...}}`
fn parse_npm(stdout: &str, eco: Ecosystem) -> Vec<Outdated> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(stdout) else {
        return Vec::new();
    };
    let Some(map) = v.as_object() else {
        return Vec::new();
    };
    map.iter()
        .filter_map(|(name, info)| {
            // `current` is absent when a package is not installed at all.
            // Skipping it rather than defaulting keeps a phantom row out
            // of a list the user is about to hand to an agent.
            let current = info.get("current")?.as_str()?;
            let latest = info.get("latest")?.as_str()?;
            if !keep(name, current, latest, eco) {
                return None;
            }
            Some(Outdated {
                name: name.clone(),
                current: current.to_string(),
                latest: latest.to_string(),
                bump: version::bump(current, latest),
                ecosystem: eco,
                manifest: "package.json".into(),
            })
        })
        .collect()
}

/// `[{"name": "x", "version": "1.0", "latest_version": "2.0"}]`
fn parse_uv(stdout: &str) -> Vec<Outdated> {
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(stdout) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|r| {
            let name = r.get("name")?.as_str()?;
            let current = r.get("version")?.as_str()?;
            let latest = r.get("latest_version")?.as_str()?;
            if !keep(name, current, latest, Ecosystem::Uv) {
                return None;
            }
            Some(Outdated {
                name: name.to_string(),
                current: current.to_string(),
                latest: latest.to_string(),
                bump: version::bump(current, latest),
                ecosystem: Ecosystem::Uv,
                manifest: "pyproject.toml".into(),
            })
        })
        .collect()
}

/// `name  current  latest  description`, whitespace-aligned.
///
/// Poetry has no JSON output for this, so the columns are the contract --
/// and they are not one Poetry promises. A line that does not have at
/// least three fields is skipped rather than half-parsed.
fn parse_poetry(stdout: &str) -> Vec<Outdated> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let name = cols.next()?;
            let current = cols.next()?;
            let latest = cols.next()?;
            // Both version columns must LOOK like versions, or this is a
            // header, a warning, or wrapped description text.
            if !current.starts_with(|c: char| c.is_ascii_digit())
                || !latest.starts_with(|c: char| c.is_ascii_digit())
            {
                return None;
            }
            if !keep(name, current, latest, Ecosystem::Poetry) {
                return None;
            }
            Some(Outdated {
                name: name.to_string(),
                current: current.to_string(),
                latest: latest.to_string(),
                bump: version::bump(current, latest),
                ecosystem: Ecosystem::Poetry,
                manifest: "pyproject.toml".into(),
            })
        })
        .collect()
}

/// `- Alamofire 5.6.1 -> 5.8.0 (latest version 5.8.0)`
///
/// `pod outdated` prints a bulleted list. Lines that do not carry two
/// versions are headers or advice, and are skipped rather than
/// half-parsed.
fn parse_cocoapods(stdout: &str) -> Vec<Outdated> {
    stdout
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("- ")?;
            let mut parts = rest.split_whitespace();
            let name = parts.next()?;
            let current = parts.next()?;
            // The arrow, then the target version.
            let latest = parts.find(|p| p.starts_with(|c: char| c.is_ascii_digit()))?;
            if !current.starts_with(|c: char| c.is_ascii_digit()) {
                return None;
            }
            if !keep(name, current, latest, Ecosystem::Cocoapods) {
                return None;
            }
            Some(Outdated {
                name: name.to_string(),
                current: current.to_string(),
                latest: latest.to_string(),
                bump: version::bump(current, latest),
                ecosystem: Ecosystem::Cocoapods,
                manifest: "Podfile".into(),
            })
        })
        .collect()
}

/// `   > PackageName   1.0.0   1.0.0   2.0.0`
///
/// `dotnet list package --outdated` prints a tree with `>`-prefixed
/// package lines: requested, resolved, then latest.
fn parse_dotnet(stdout: &str, repo: &Path) -> Vec<Outdated> {
    let manifest = std::fs::read_dir(repo)
        .ok()
        .and_then(|entries| {
            entries.flatten().find_map(|e| {
                let p = e.path();
                let ext = p.extension()?.to_str()?.to_ascii_lowercase();
                ["csproj", "fsproj", "vbproj"]
                    .contains(&ext.as_str())
                    .then(|| e.file_name().to_string_lossy().to_string())
            })
        })
        .unwrap_or_else(|| "the project file".to_string());

    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("> ")?;
            let cols: Vec<&str> = rest.split_whitespace().collect();
            // name, requested, resolved, latest.
            if cols.len() < 4 {
                return None;
            }
            let (name, current, latest) = (cols[0], cols[2], cols[3]);
            if !keep(name, current, latest, Ecosystem::Dotnet) {
                return None;
            }
            Some(Outdated {
                name: name.to_string(),
                current: current.to_string(),
                latest: latest.to_string(),
                bump: version::bump(current, latest),
                ecosystem: Ecosystem::Dotnet,
                manifest: manifest.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::model::Bump;

    fn nowhere() -> &'static Path {
        Path::new("/nonexistent")
    }

    /// npm's real shape, including a package with NO `current` -- which
    /// happens when a dependency is declared but not installed. A
    /// phantom row here would be handed straight to an agent.
    #[test]
    fn parses_npm_json_and_skips_uninstalled_packages() {
        let out = r#"{
          "react": {"current": "18.2.0", "wanted": "18.3.0", "latest": "19.0.0"},
          "vite":  {"current": "5.0.1",  "wanted": "5.0.2",  "latest": "5.0.2"},
          "ghost": {"wanted": "1.0.0", "latest": "2.0.0"}
        }"#;
        let rows = parse(out, Ecosystem::Npm, nowhere());
        assert_eq!(
            rows.len(),
            2,
            "the uninstalled package is skipped: {rows:?}"
        );
        let react = rows.iter().find(|r| r.name == "react").unwrap();
        assert_eq!(react.bump, Bump::Major);
        assert_eq!(react.manifest, "package.json");
        let vite = rows.iter().find(|r| r.name == "vite").unwrap();
        assert_eq!(vite.bump, Bump::Patch);
    }

    /// npm prints `{}` when everything is current.
    #[test]
    fn an_empty_npm_result_is_no_rows_not_an_error() {
        assert!(parse("{}", Ecosystem::Npm, nowhere()).is_empty());
    }

    #[test]
    fn parses_uv_json() {
        let out = r#"[
          {"name": "requests", "version": "2.28.0", "latest_version": "2.31.0"},
          {"name": "urllib3",  "version": "1.26.0", "latest_version": "2.0.0"}
        ]"#;
        let rows = parse(out, Ecosystem::Uv, nowhere());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].bump, Bump::Minor);
        assert_eq!(rows[1].bump, Bump::Major);
    }

    /// Poetry has no JSON output, so the COLUMNS are the contract -- and
    /// they are not a contract Poetry promises. Header lines, warnings,
    /// and wrapped descriptions all have to be rejected.
    #[test]
    fn parses_poetry_columns_and_rejects_everything_else() {
        let out = "\
Warning: something happened
requests 2.28.0 2.31.0 Python HTTP for Humans
urllib3  1.26.0 2.0.0  HTTP library
  continued description text
";
        let rows = parse(out, Ecosystem::Poetry, nowhere());
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].name, "requests");
        assert_eq!(rows[1].bump, Bump::Major);
    }

    /// A version column that is not a version means the line is not a
    /// package row, whatever else it looks like.
    #[test]
    fn a_poetry_line_without_versions_is_not_a_package() {
        let out = "Package Version Latest Description\n";
        assert!(parse(out, Ecosystem::Poetry, nowhere()).is_empty());
    }

    #[test]
    fn parses_dotnet_tree_output() {
        let out = "\
Project `Api` has the following updates
   [net8.0]:
   Top-level Package      Requested   Resolved   Latest
   > Newtonsoft.Json      13.0.1      13.0.1     13.0.3
   > Serilog              2.12.0      2.12.0     3.1.1
";
        let rows = parse(out, Ecosystem::Dotnet, nowhere());
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].name, "Newtonsoft.Json");
        assert_eq!(rows[0].bump, Bump::Patch);
        assert_eq!(rows[1].bump, Bump::Major);
    }

    /// Unparseable output yields no rows rather than garbage ones. The
    /// caller turns "no rows and a failed status" into an error.
    #[test]
    fn garbage_output_produces_no_rows() {
        for eco in [
            Ecosystem::Npm,
            Ecosystem::Uv,
            Ecosystem::Poetry,
            Ecosystem::Dotnet,
        ] {
            assert!(
                parse("not json, not columns", eco, nowhere()).is_empty(),
                "{eco:?}"
            );
        }
    }

    /// The whole reason `error` exists on the report: a check that could
    /// not run must NOT render as "you are up to date".
    ///
    /// Driven through `check` against an empty directory, where every
    /// tool has something to complain about. What the message SAYS
    /// depends on whether the tool is installed on this machine -- that
    /// varies by environment and is not the property under test. What
    /// must hold everywhere is that a failure never comes back as an
    /// empty success.
    #[test]
    fn a_failed_check_never_looks_like_success() {
        let t = tempfile::TempDir::new().unwrap();
        let r = check(t.path(), Ecosystem::Dotnet);
        assert!(
            r.error.is_some() || r.outdated.is_empty(),
            "a report with rows and no error would be claiming a real result"
        );
        if let Some(msg) = &r.error {
            assert!(!msg.trim().is_empty(), "an error must say something");
        }
    }

    /// `npm outdated` EXITS 1 WHEN UPDATES EXIST. Treating non-zero as
    /// failure reports "no updates" on every repository that has some --
    /// the exact inversion this module exists to prevent.
    #[test]
    fn a_non_zero_exit_with_output_is_not_a_failure() {
        assert!(
            !is_real_failure(false, false, r#"{"react":{}}"#),
            "npm exits 1 when it finds updates"
        );
        assert!(
            !is_real_failure(true, false, "{}"),
            "output that parsed to nothing is a format question, not a failed run"
        );
    }

    #[test]
    fn a_non_zero_exit_with_no_output_at_all_is_a_failure() {
        assert!(is_real_failure(true, false, "   "));
    }

    #[test]
    fn a_clean_exit_is_never_a_failure() {
        assert!(!is_real_failure(true, true, ""));
    }

    /// Swift IS checked now, from `Package.resolved` plus the Git host's
    /// tags -- #434.
    ///
    /// The old assertion here was that it reported "cannot check". That
    /// was accurate about COMMANDS (nothing diffs Xcode-managed
    /// dependencies) and wrong about the question: Swift versions are
    /// git tags, and the resolved file names both the pin and its source
    /// URL.
    ///
    /// A repository with no resolved file still reports NOTHING rather
    /// than an error: there is genuinely nothing pinned, which is a real
    /// empty rather than a failed check.
    #[test]
    fn swift_reports_its_pins_rather_than_refusing() {
        let t = tempfile::TempDir::new().unwrap();
        let r = check(t.path(), Ecosystem::Swift);
        assert!(r.outdated.is_empty(), "no resolved file, nothing pinned");
        assert!(r.error.is_none(), "an absent file is not a failure");

        // With one, the pin is reported -- and left UNCOMPARED until
        // enrichment, never claimed to be current.
        std::fs::write(
            t.path().join("Package.resolved"),
            r#"{"pins":[{"identity":"x",
               "location":"https://github.com/octocat/example.git",
               "state":{"revision":"abc","version":"1.2.3"}}],"version":3}"#,
        )
        .unwrap();
        let r = check(t.path(), Ecosystem::Swift);
        assert_eq!(r.outdated.len(), 1);
        assert_eq!(r.outdated[0].current, "1.2.3");
        assert_eq!(r.outdated[0].bump, crate::packages::model::Bump::Unknown);
    }

    #[test]
    fn parses_cocoapods_output() {
        let out = "\
The following pod updates are available:
- Alamofire 5.6.1 -> 5.8.0 (latest version 5.8.0)
- SwiftyJSON 4.0.0 -> 5.0.0 (latest version 5.0.0)
";
        let rows = parse(out, Ecosystem::Cocoapods, nowhere());
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].name, "Alamofire");
        assert_eq!(rows[0].bump, Bump::Minor);
        assert_eq!(rows[1].bump, Bump::Major);
    }

    /// The not-installed path, without depending on what is installed.
    #[test]
    fn a_tool_that_cannot_be_found_is_reported_as_missing() {
        assert!(tools::find("headstate-definitely-not-a-real-tool", &[]).is_none());

        // And that None becomes an ERROR, not an empty success.
        let r = missing_tool(Ecosystem::Npm);
        assert!(r.outdated.is_empty());
        let msg = r
            .error
            .expect("a missing tool must never report as up to date");
        assert!(msg.contains("npm"), "and must name the tool: {msg}");
        assert!(
            msg.contains("PATH"),
            "and say why, since that is actionable"
        );
    }

    // ---- downgrades ---------------------------------------------------
    //
    // A tool's `latest` column is not always newer than what is
    // installed. `npm outdated --json` reported `jsdom` as
    // `current 30.0.1, latest 29.1.1` while the registry's own `latest`
    // dist-tag was 30.0.1. Passed through, the Packages page offers a
    // DOWNGRADE as an update, and Apply runs `yarn up jsdom@29.1.1`.
    //
    // The two tests that matter are the two SIDES of the line: a
    // confidently backwards jump disappears, and a pair that cannot be
    // compared still shows as `Unknown`. Collapsing those would either
    // reintroduce the bug or hide rows the design deliberately surfaces.

    /// The real jsdom shape, from `npm outdated --json` in this
    /// repository. No row: applying it would be a downgrade.
    #[test]
    fn an_npm_downgrade_produces_no_row() {
        let out = r#"{"jsdom": {"current": "30.0.1", "wanted": "30.0.1", "latest": "29.1.1"}}"#;
        let rows = parse(out, Ecosystem::Npm, nowhere());
        assert!(
            rows.is_empty(),
            "29.1.1 is older than 30.0.1, so it is not an update: {rows:?}"
        );
    }

    /// And the row beside it survives. Dropping the downgrade must not
    /// take the rest of the report with it.
    #[test]
    fn a_downgrade_does_not_take_the_real_updates_with_it() {
        let out = r#"{
          "jsdom": {"current": "30.0.1", "wanted": "30.0.1", "latest": "29.1.1"},
          "react": {"current": "18.2.0", "wanted": "18.3.0", "latest": "19.0.0"}
        }"#;
        let rows = parse(out, Ecosystem::Npm, nowhere());
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].name, "react");
        assert_eq!(rows[0].bump, Bump::Major);
    }

    /// THE OTHER SIDE OF THE LINE. A pair that cannot be compared is
    /// NOT dropped: it is reported as `Unknown`, because "we cannot
    /// tell" is the honest answer this design keeps rather than a row to
    /// hide. Only a CONFIDENTLY backwards jump disappears.
    #[test]
    fn an_uncomparable_npm_pair_is_still_reported_as_unknown() {
        let out = r#"{
          "alpha": {"current": "1.0rc1", "latest": "1.0"},
          "beta":  {"current": "main",   "latest": "2.0.0"},
          "gamma": {"current": "1.2.3",  "latest": "not-a-version"}
        }"#;
        let rows = parse(out, Ecosystem::Npm, nowhere());
        assert_eq!(rows.len(), 3, "none of these may be dropped: {rows:?}");
        for r in &rows {
            assert_eq!(r.bump, Bump::Unknown, "{}", r.name);
        }
    }

    /// The same rule in every parser that builds a row, not just npm's.
    /// Each fixture is that tool's real output shape with the `latest`
    /// column made older than the installed version.
    #[test]
    fn every_parser_drops_a_backwards_row() {
        let uv = r#"[{"name": "requests", "version": "2.31.0", "latest_version": "2.28.0"}]"#;
        assert!(
            parse(uv, Ecosystem::Uv, nowhere()).is_empty(),
            "uv: 2.28.0 is older than 2.31.0"
        );

        let poetry = "requests 2.31.0 2.28.0 Python HTTP for Humans\n";
        assert!(
            parse(poetry, Ecosystem::Poetry, nowhere()).is_empty(),
            "poetry: 2.28.0 is older than 2.31.0"
        );

        let dotnet = "   > Newtonsoft.Json      13.0.3      13.0.3     13.0.1\n";
        assert!(
            parse(dotnet, Ecosystem::Dotnet, nowhere()).is_empty(),
            "dotnet: 13.0.1 is older than 13.0.3"
        );

        let pods = "- Alamofire 5.8.0 -> 5.6.1 (latest version 5.6.1)\n";
        assert!(
            parse(pods, Ecosystem::Cocoapods, nowhere()).is_empty(),
            "cocoapods: 5.6.1 is older than 5.8.0"
        );
    }

    /// And the other side of the line in every parser too. An
    /// uncomparable pair keeps its row, in all four.
    #[test]
    fn every_parser_still_reports_an_uncomparable_row() {
        let uv = r#"[{"name": "requests", "version": "1.0rc1", "latest_version": "1.0"}]"#;
        let rows = parse(uv, Ecosystem::Uv, nowhere());
        assert_eq!(rows.len(), 1, "uv: {rows:?}");
        assert_eq!(rows[0].bump, Bump::Unknown);

        // Poetry and CocoaPods both require their version columns to
        // START with a digit, so a pre-release suffix is the shape of
        // uncomparable pair those parsers can actually see.
        let poetry = "requests 1.0rc1 1.0 Python HTTP for Humans\n";
        let rows = parse(poetry, Ecosystem::Poetry, nowhere());
        assert_eq!(rows.len(), 1, "poetry: {rows:?}");
        assert_eq!(rows[0].bump, Bump::Unknown);

        let dotnet = "   > Newtonsoft.Json      13.0.1      13.0.1     not-a-version\n";
        let rows = parse(dotnet, Ecosystem::Dotnet, nowhere());
        assert_eq!(rows.len(), 1, "dotnet: {rows:?}");
        assert_eq!(rows[0].bump, Bump::Unknown);

        let pods = "- Alamofire 5.6.1 -> 5.6.1rc1 (latest version 5.6.1rc1)\n";
        let rows = parse(pods, Ecosystem::Cocoapods, nowhere());
        assert_eq!(rows.len(), 1, "cocoapods: {rows:?}");
        assert_eq!(rows[0].bump, Bump::Unknown);
    }

    /// A package already at the newest version is not a downgrade, and
    /// the parsers must keep behaving as they did: npm lists such a
    /// package when `wanted` differs from `current`, and that row is a
    /// real one.
    #[test]
    fn an_equal_pair_is_not_treated_as_a_downgrade() {
        let out = r#"{"vite": {"current": "5.0.2", "wanted": "5.0.2", "latest": "5.0.2"}}"#;
        let rows = parse(out, Ecosystem::Npm, nowhere());
        assert_eq!(rows.len(), 1, "an equal pair is not backwards: {rows:?}");
        assert_eq!(rows[0].bump, Bump::Unknown);
    }
}

/// End-to-end against a REAL project under a GUI-like PATH.
///
/// Ignored by default: it needs npm and a project on this machine. The
/// unit tests cover the mechanism; this proves the whole path, which is
/// what the bug report was about.
///
/// `HEADSTATE_E2E_REPO=/path/to/project cargo test -- --ignored gui_like`
#[cfg(test)]
mod e2e {
    use super::*;

    #[test]
    #[ignore = "needs npm and a real project"]
    fn a_gui_like_path_still_resolves_the_interpreter() {
        let Ok(repo) = std::env::var("HEADSTATE_E2E_REPO") else {
            eprintln!("set HEADSTATE_E2E_REPO to run this");
            return;
        };
        // The PATH a GUI-launched .app actually gets: no version
        // manager, no Homebrew.
        temp_env::with_var("PATH", Some("/usr/bin:/bin:/usr/sbin:/sbin"), || {
            let report = check(std::path::Path::new(&repo), Ecosystem::Npm);
            eprintln!("error: {:?}", report.error);
            eprintln!("outdated: {}", report.outdated.len());
            if let Some(e) = &report.error {
                assert!(
                    !e.contains("No such file or directory"),
                    "the interpreter must resolve: {e}"
                );
            }
        });
    }
}

#[cfg(test)]
mod berry {
    use super::*;

    /// The REAL bytes Yarn 4 writes, colour codes and all, captured from
    /// `yarn outdated --json` on a Yarn 4.9 project. Yarn colours its
    /// output even when redirected, so a naive prefix check misses it.
    const YARN_BERRY_USAGE: &str =
        "\u{1b}[31m\u{1b}[1mUsage Error\u{1b}[22m\u{1b}[39m: Couldn't find a script named \"outdated\".\n";

    #[test]
    fn a_yarn_berry_usage_error_is_a_failure_not_zero_updates() {
        assert!(
            is_usage_error(YARN_BERRY_USAGE),
            "the real Yarn Berry output must be recognised"
        );
        // Exits 1 and writes to STDOUT, so the old rule -- which
        // required empty stdout -- let this through as "no updates".
        assert!(is_real_failure(true, false, YARN_BERRY_USAGE));
    }

    /// The inversion this must never reintroduce: `npm outdated` exits 1
    /// precisely WHEN there are updates.
    #[test]
    fn a_non_zero_exit_with_real_output_is_not_a_failure() {
        let real = r#"{"lodash":{"current":"4.17.20","latest":"4.17.21"}}"#;
        assert!(!is_real_failure(false, false, real));
    }

    /// A genuinely empty result stays a non-failure.
    #[test]
    fn an_honestly_empty_result_is_not_a_failure() {
        assert!(!is_usage_error("{}"));
        assert!(!is_real_failure(true, true, "{}"));
    }

    /// Anchored to the FIRST line, so a later line that happens to begin
    /// with those words is not mistaken for the tool refusing.
    ///
    /// Pretty-printed JSON is the realistic case: `npm outdated --json`
    /// emits one key per line, and a package or field could legitimately
    /// start with "usage error".
    #[test]
    fn the_words_on_a_later_line_are_not_a_usage_error() {
        let pretty = "{\n  \"pkg\": {\n\"usage error handling\": 1\n  }\n}";
        assert!(
            !is_usage_error(pretty),
            "only the first line may declare a refusal"
        );
        // And a real result that merely mentions them stays a result.
        assert!(!is_real_failure(false, false, pretty));

        // The case that makes the anchoring load-bearing rather than
        // decorative: output whose FIRST line is blank, with the words
        // further down. Matching the whole string would call this a
        // refusal; matching the first line does not.
        let later = "\n{\"a\":1}\nusage error: not a refusal, just text\n";
        assert!(!is_usage_error(later));
    }
}

/// CocoaPods, which fails in a way none of the other tools do.
#[cfg(test)]
mod cocoapods {
    use super::*;

    /// The REAL bytes CocoaPods 1.17 writes to stderr when the child
    /// process has no UTF-8 locale, captured from `pod outdated` run with
    /// `LANG`, `LC_ALL` and `LC_CTYPE` unset -- which is exactly a macOS
    /// GUI app's environment.
    ///
    /// Two facts about it, and both matter. It is INDENTED four spaces
    /// and coloured, so it does not look like the first line of an error
    /// message. And it is a WARNING followed by three more lines of
    /// advice, after which CocoaPods crashes for real with an
    /// `Encoding::CompatibilityError` -- so the useful line is the fifth,
    /// not the first.
    const POD_UTF8_WARNING: &str = concat!(
        "    \u{1b}[33mWARNING: CocoaPods requires your terminal to be using UTF-8 encoding.\n",
        "    Consider adding the following to ~/.profile:\n\n",
        "    export LANG=en_US.UTF-8\n",
        "    \u{1b}[0m\n",
        "/opt/homebrew/Cellar/ruby/4.0.5/lib/ruby/4.0.0/unicode_normalize/normalize.rb:153:in ",
        "'UnicodeNormalize.normalize': Unicode Normalization not appropriate for ASCII-8BIT ",
        "(Encoding::CompatibilityError)\n",
    );

    /// No raw escape sequence ever reaches the UI.
    ///
    /// `[33m` rendered as literal text in the error banner. Yarn's
    /// output is already stripped on the `is_usage_error` path, so the
    /// stripper existed -- it was just not on the path that builds the
    /// message a user reads.
    #[test]
    fn the_error_message_carries_no_ansi_escape() {
        let msg = failure_message(POD_UTF8_WARNING, "");
        assert!(
            !msg.contains('\u{1b}') && !msg.contains("[33m") && !msg.contains("[0m"),
            "an escape code leaked into the UI: {msg:?}"
        );
    }

    /// The message names what actually went wrong.
    ///
    /// The first stderr line is a WARNING about the terminal, and
    /// reporting it as the error told the user to fix their `~/.profile`
    /// when the real failure was four lines further down. Taking the
    /// first line is right for every other tool here and wrong for this
    /// one, because this one warns before it fails.
    #[test]
    fn the_error_message_is_the_failure_not_the_warning_above_it() {
        let msg = failure_message(POD_UTF8_WARNING, "");
        assert!(
            !msg.contains("WARNING"),
            "a warning was reported as the error: {msg:?}"
        );
        assert!(
            msg.contains("Encoding::CompatibilityError"),
            "the real failure must be named: {msg:?}"
        );
    }

    /// A tool whose first stderr line IS the error keeps reporting it.
    ///
    /// The warning skip must be narrow enough that it does not eat a
    /// legitimate first line, and blank and indented lines are skipped
    /// only in service of finding one.
    #[test]
    fn an_ordinary_first_line_is_still_the_message() {
        assert_eq!(
            failure_message("bash: pod: command not found\n", ""),
            "bash: pod: command not found"
        );
        assert_eq!(
            failure_message(
                "",
                "[!] No `Podfile.lock' found in the project directory.\n"
            ),
            "[!] No `Podfile.lock' found in the project directory."
        );
    }

    /// Nothing on stderr at all still says something.
    #[test]
    fn empty_stderr_falls_back_to_a_sentence() {
        assert_eq!(failure_message("", ""), "the command failed");
        assert_eq!(failure_message("   \n\n", ""), "the command failed");
        // Warnings and nothing else: there is no better line to show, so
        // the warning is better than an empty banner.
        assert_eq!(
            failure_message("    \u{1b}[33mWARNING: something\n", ""),
            "WARNING: something"
        );
    }

    /// CocoaPods' own refusal, on STDOUT with a non-zero exit, is a
    /// failure -- not zero updates.
    ///
    /// The exact shape Yarn Berry's was: `pod outdated` in a directory
    /// with a `Podfile` but no `Podfile.lock` prints
    /// `[!] No 'Podfile.lock' found in the project directory` to stdout
    /// and exits 1. `is_real_failure` required stdout to be EMPTY or a
    /// recognised usage error, and this was neither -- so a project that
    /// could not be checked at all rendered as a clean, empty list.
    ///
    /// Measured on this repository: `src-mobile/gen/apple` has a Podfile
    /// and no installed Pods, and reported "no updates" with no error.
    #[test]
    fn a_cocoapods_refusal_on_stdout_is_a_failure_not_zero_updates() {
        const NO_LOCK: &str = "[!] No `Podfile.lock' found in the project directory, \
                               run `pod install'.\n";
        assert!(is_usage_error(NO_LOCK));
        assert!(is_real_failure(true, false, NO_LOCK));
        // And the banner names it, reading the stream it was written to.
        assert_eq!(
            failure_message("", NO_LOCK),
            "[!] No `Podfile.lock' found in the project directory, run `pod install'."
        );
    }

    /// The marker must not swallow a real result.
    ///
    /// It is anchored to the first line, requires the trailing space, and
    /// a zero exit is never a failure whatever was printed -- so an
    /// ordinary `pod outdated` listing stays a listing.
    #[test]
    fn a_real_cocoapods_listing_is_not_mistaken_for_a_refusal() {
        let listing = "The following pod updates are available:\n\
                       - Alamofire 5.8.0 -> 5.8.0 (latest version 5.9.1)\n";
        assert!(!is_usage_error(listing));
        assert!(!is_real_failure(false, false, listing));
        // A bracket that is not the marker.
        assert!(!is_usage_error("[info] nothing to do"));
        assert!(!is_usage_error("[!]nospace"));
        // Anchored to the FIRST line, like every other rule here.
        assert!(!is_usage_error("a real first line\n[!] later\n"));
        // Exit 0 is never a failure, whatever it printed.
        assert!(!is_real_failure(true, true, "[!] something"));
    }

    /// Text with no escapes at all comes back unchanged.
    ///
    /// The bug this pins was in the stripper the Yarn path already had:
    /// it split every fragment at its first `m`, INCLUDING the text
    /// before any escape, so `bash: pod: command not found` came out as
    /// `mand not found`. It was invisible there because that path only
    /// ever asked whether the result STARTS WITH "usage error", and a
    /// mangled line answers no just as a clean one does. Putting the
    /// same helper on the path that builds a message a user reads is
    /// what made it visible.
    #[test]
    fn stripping_leaves_plain_text_alone() {
        assert_eq!(
            strip_ansi("bash: pod: command not found"),
            "bash: pod: command not found"
        );
        assert_eq!(
            strip_ansi("no escapes, many m's here"),
            "no escapes, many m's here"
        );
        // And it still does its job on the real coloured bytes.
        assert_eq!(
            strip_ansi("\u{1b}[31m\u{1b}[1mUsage Error\u{1b}[22m\u{1b}[39m: nope"),
            "Usage Error: nope"
        );
        // A sequence that is not a colour is left whole rather than
        // half-eaten.
        assert_eq!(strip_ansi("a\u{1b}[2Kb"), "a[2Kb");
    }

    /// Non-ASCII text does not panic and is not mangled.
    ///
    /// `find` returns a BYTE index and the byte at it is then compared
    /// against `m`, so a multi-byte character right after the escape is
    /// the case where those two could disagree. It lands on a character
    /// boundary either way -- so the read is in range, the lead byte is
    /// never `m`, and the fragment is correctly left whole. Worth a test
    /// rather than a comment: a package name or a path in a tool's error
    /// message is not guaranteed to be ASCII.
    #[test]
    fn stripping_is_safe_on_non_ascii() {
        assert_eq!(strip_ansi("é\u{1b}[31mrouge\u{1b}[0m"), "érouge");
        assert_eq!(strip_ansi("\u{1b}[é"), "[é");
        assert_eq!(strip_ansi("\u{1b}[31mné"), "né");
        assert_eq!(strip_ansi("路径: not found"), "路径: not found");
    }

    /// The child process is given a UTF-8 locale.
    ///
    /// A GUI app inherits neither `LANG` nor `LC_ALL` -- the same class
    /// of missing-environment bug as the PATH one `tools::child_path`
    /// exists for -- and without one CocoaPods does not merely warn, it
    /// dies in `unicode_normalize`. Measured on this machine: with the
    /// locale unset `pod outdated` exits 1 having printed nothing usable;
    /// with `LC_ALL=en_US.UTF-8` it runs and reports its real answer.
    ///
    /// Setting it is what the warning itself asks for, and it is scoped
    /// to the child: nothing about the app's own environment changes.
    #[test]
    fn cocoapods_gets_a_utf8_locale_and_other_tools_do_not() {
        assert_eq!(child_locale(Ecosystem::Cocoapods), Some("en_US.UTF-8"));
        for eco in [
            Ecosystem::Npm,
            Ecosystem::Yarn,
            Ecosystem::Poetry,
            Ecosystem::Uv,
            Ecosystem::Dotnet,
        ] {
            assert_eq!(child_locale(eco), None, "{eco:?}");
        }
    }
}

/// End-to-end against a REAL Yarn Berry project. Ignored by default: it
/// needs npm, a project on this machine, and the network.
///
/// `HEADSTATE_YARN_REPO=/path cargo test -- --ignored yarn_e2e`
#[cfg(test)]
mod yarn_e2e {
    use super::*;

    #[test]
    #[ignore = "needs a real Yarn Berry project and network access"]
    fn a_yarn_berry_project_reports_updates() {
        let Ok(repo) = std::env::var("HEADSTATE_YARN_REPO") else {
            eprintln!("set HEADSTATE_YARN_REPO to run this");
            return;
        };
        let report = check(Path::new(&repo), Ecosystem::Yarn);
        eprintln!("error: {:?}", report.error);
        eprintln!("outdated: {}", report.outdated.len());
        assert!(report.error.is_none(), "{:?}", report.error);
        assert!(
            !report.outdated.is_empty(),
            "a Berry project with outdated packages must report them"
        );
    }
}

/// End-to-end against the REAL `pod` binary, in the environment a GUI
/// app actually has. Ignored by default: it needs CocoaPods installed.
///
/// `HEADSTATE_PODFILE_DIR=/path cargo test -- --ignored pod_e2e --nocapture`
#[cfg(test)]
mod pod_e2e {
    use super::*;

    /// The locale fix, measured rather than asserted from a fixture.
    ///
    /// The test CLEARS `LANG`, `LC_ALL` and `LC_CTYPE` from its own
    /// process first, because that -- not a terminal -- is the
    /// environment a launched `.app` runs in, and it is the only way to
    /// reproduce the failure at all. With the variables cleared and no
    /// fix, `pod outdated` exits 1 having written a coloured warning and
    /// a Ruby backtrace; with the fix it runs and reports whatever it
    /// really has to say.
    ///
    /// `temp_env` restores the variables afterwards, so this does not
    /// leak into the rest of the suite.
    #[test]
    #[ignore = "needs CocoaPods installed and a directory holding a Podfile"]
    fn a_gui_environment_no_longer_breaks_cocoapods() {
        let Ok(dir) = std::env::var("HEADSTATE_PODFILE_DIR") else {
            eprintln!("set HEADSTATE_PODFILE_DIR to run this");
            return;
        };
        temp_env::with_vars(
            [
                ("LANG", None::<&str>),
                ("LC_ALL", None::<&str>),
                ("LC_CTYPE", None::<&str>),
            ],
            || {
                let report = check(Path::new(&dir), Ecosystem::Cocoapods);
                eprintln!("error: {:?}", report.error);
                eprintln!("outdated: {}", report.outdated.len());
                if let Some(e) = &report.error {
                    assert!(!e.contains('\u{1b}'), "a raw escape reached the UI: {e:?}");
                    assert!(
                        !e.contains("UTF-8 encoding"),
                        "the encoding warning is still reported as the error: {e:?}"
                    );
                }
            },
        );
    }
}
