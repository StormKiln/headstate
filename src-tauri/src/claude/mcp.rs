//! Every MCP server configured on this machine, and which scope defines
//! it (#1216).
//!
//! Before this, a grep over `src-tauri/src` for
//! `mcpServers|\.mcp\.json|mcp_servers|claude\.json` returned three
//! hits, all in [`super::plugins`], all the same filename match setting
//! one boolean: `out.mcp = true`. `~/.claude.json` -- where MCP server
//! configuration actually lives -- was never opened. So #1129 gave the
//! app a page naming every skill, agent and command on the machine, and
//! it could not name a single MCP tool. A plugin contributing 31 tools
//! and one contributing zero were one boolean apart.
//!
//! # Scope is the load-bearing column
//!
//! Not the list. A server added for one project is easy to forget and
//! then confusing everywhere else: it works in one repository and is
//! absent in the next, with nothing on screen explaining why. So every
//! server carries the [`Origin`] that defines it, and
//! [`Inventory::in_force`] answers the per-repository question
//! directly.
//!
//! [`Origin`] is REUSED from [`super::settings`] rather than
//! re-invented. #1216 asked for that explicitly, so the two pages speak
//! one vocabulary -- and it is why `Origin` grew a `Plugin` variant
//! instead of this module declaring a parallel scope enum. That
//! decision and its precedence consequences are argued at `Origin`'s
//! own definition.
//!
//! # `~/.claude.json` is a live state file
//!
//! The class `super`'s module docs warn about: *"one of its files is
//! rewritten by its owner every few seconds."* `~/.claude.json` is
//! Claude Code's running state. It mixes configuration with per-project
//! transcript history, cached experiment flags and server caches, and it
//! is rewritten WHILE Claude Code runs. Measured on the development
//! machine: 160 KB, a top-level `mcpServers` with 4 servers, and a
//! `projects` map with 55 entries of which 3 carry their own
//! `mcpServers`.
//!
//! Three rules follow, and all three are load-bearing.
//!
//! **Read-only.** Nothing here writes to that file. Not once, not to
//! normalise it, not to prune the history it has accumulated. A writer
//! racing Claude Code's own rewrite would corrupt the state of every
//! session on the machine, and the value of doing so is zero -- this is
//! an inventory.
//!
//! **Bounded.** 160 KB is today's figure on one machine and it grows
//! with project history, which is unbounded by construction: every
//! project ever opened adds an entry, and those entries carry
//! `lastCost`, token totals and example-file lists. [`BUDGET_BYTES`]
//! caps the read, in the shape [`super::usage`]'s `BUDGET_BYTES` and
//! [`super::preview`]'s `TAIL_BYTES` already established -- including
//! their honesty about it: when the cap binds, [`Inventory`] SAYS the
//! read stopped early rather than presenting a partial answer as a
//! total.
//!
//! **A parse failure is a refusal, not a zero.** This is the rule the
//! issue names as this codebase's characteristic defect appearing in a
//! new place. A page reporting "0 MCP servers" because a large,
//! concurrently-rewritten state file failed to parse has told the user
//! something false with a credible shape. "Could not read the
//! configuration" and "no servers are configured" are different
//! sentences and this module never collapses them: the first is a
//! [`ScopeRefusal`] on [`Inventory::unreadable`], the second is an empty
//! [`Inventory::servers`] with nothing in `unreadable`.
//!
//! Absent is not zero (#846), in the form this file's particular hazard
//! takes: a concurrent rewrite can hand a reader a TORN file -- a
//! prefix of the new bytes, or a prefix of the old. That parses as
//! malformed JSON, and malformed JSON here means "ask again in a
//! second", not "this machine has no MCP servers". It is reported with
//! serde's own line and column, because that is the only actionable
//! thing available and it also distinguishes a genuine syntax error
//! from a truncation at the cap.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::settings::{Origin, ScopeRefusal};

/// How much of `~/.claude.json` is read.
///
/// 4 MB against a file measured at 160 KB, which is a deliberate 25x
/// headroom rather than a tight fit. The measured size is not the
/// design constraint: the `projects` map grows without bound -- one
/// entry per project ever opened, each carrying cost totals, token
/// counters and an `exampleFiles` list -- so the figure that matters is
/// the one a machine reaches after a year, and nobody has measured
/// that.
///
/// The bound exists so that this cannot become the hang
/// [`super::usage::BUDGET_BYTES`] and [`super::preview::TAIL_BYTES`]
/// exist to prevent, arrived at by a third route. It is generous
/// because binding it is a DEGRADED answer -- a truncated read cannot
/// parse, so it yields a refusal rather than a partial list -- and a
/// refusal on a healthy machine would be a false alarm.
///
/// When it does bind, [`Inventory::truncated`] says so, and the refusal
/// says so in words. A silent cap would be the #846 defect: the reader
/// could not tell a machine with no servers from a file too large to
/// read.
pub const BUDGET_BYTES: u64 = 4 * 1024 * 1024;

/// How a server is reached.
///
/// Rendered from the entry rather than stored as free text, so the page
/// can say "stdio" and the command in two columns. An entry whose shape
/// we do not recognise is [`Transport::Unknown`] rather than omitted:
/// a server we cannot describe is still a server that is configured,
/// and dropping it would understate the inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Transport {
    /// A local process. `command` plus its arguments, joined for
    /// display.
    Stdio { command: String },
    /// An HTTP or SSE endpoint.
    Url { url: String },
    /// The entry parsed as an object but carried neither a `command`
    /// nor a `url`.
    ///
    /// Kept rather than dropped, and named rather than guessed: Claude
    /// Code owns this format and adds to it, so an unrecognised shape
    /// is more likely a newer transport than a broken entry.
    Unknown,
}

/// One configured MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    /// The key it is configured under -- what `/mcp` calls it.
    pub name: String,
    pub transport: Transport,
    /// Which scope defines it. The load-bearing column (#1216).
    pub origin: Origin,
    /// For [`Origin::Project`], the project path whose entry carried it.
    /// For [`Origin::Plugin`], the plugin that ships it. `None` for
    /// [`Origin::User`], which has exactly one home.
    ///
    /// This is what makes "project" actionable: a reader who cannot see
    /// WHICH project has been told the scope and not the answer.
    pub scope_detail: Option<String>,
}

/// Every server found, plus everything that could not be read.
// `PartialEq` but not `Eq`: `ScopeRefusal` carries only `PartialEq`, and
// widening an existing shared type to satisfy a derive here would be
// this module reaching into `settings.rs` for its own convenience.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    /// User-scope and project-scope servers, and plugin-shipped ones.
    pub servers: Vec<Server>,
    /// Scopes that exist and could not be read, with the refusal's own
    /// sentence.
    ///
    /// NON-EMPTY with an empty `servers` is the case the module docs
    /// are about, and the UI must render it as "could not read", never
    /// as "none configured".
    pub unreadable: Vec<ScopeRefusal>,
    /// Whether [`BUDGET_BYTES`] bound the read of `~/.claude.json`.
    ///
    /// When true the file was larger than the budget and only its first
    /// [`BUDGET_BYTES`] were read -- which cannot parse, so this
    /// travels ALONGSIDE a refusal rather than alongside a short list.
    /// It exists so the UI can say WHY the refusal happened: "too large
    /// to read" and "malformed" have different remedies.
    pub truncated: bool,
    /// The size of `~/.claude.json`, when it could be measured.
    ///
    /// Reported so a truncation notice can state the total rather than
    /// just the floor -- the honesty [`super::usage`] applies to its own
    /// budget.
    pub size_bytes: Option<u64>,
}

impl Inventory {
    /// The servers in force for one repository.
    ///
    /// User-scope and plugin-scope servers apply everywhere; a
    /// project-scope server applies only to the project it was
    /// configured under. This is the per-repository question #1216
    /// asks.
    ///
    /// The frontend's `mcpInForce` applies the SAME rule, because the
    /// page marks rows per server rather than requesting a filtered
    /// list. Two implementations of one rule is a real cost and it is
    /// taken deliberately: the alternative is a second command round
    /// trip per repository selection, over a transport the phone also
    /// uses. Both sides are tested against the same cases -- including
    /// the trailing separator -- and this comment is the pointer
    /// between them, so a change to one is a change to two.
    ///
    /// Paths are compared after normalising a trailing separator, which
    /// is the only difference observed between what a user types and
    /// what Claude Code stores. No symlink resolution: that would touch
    /// the filesystem from what is otherwise a pure function over
    /// already-read data, and a wrong answer from a resolved symlink
    /// would be indistinguishable from a right one.
    pub fn in_force(&self, repo: &Path) -> Vec<&Server> {
        let want = normalise(&repo.to_string_lossy());
        self.servers
            .iter()
            .filter(|s| match s.origin {
                Origin::Project => s
                    .scope_detail
                    .as_deref()
                    .is_some_and(|p| normalise(p) == want),
                _ => true,
            })
            .collect()
    }
}

/// Trim a trailing path separator so `/a/b` and `/a/b/` compare equal.
fn normalise(p: &str) -> String {
    p.trim_end_matches('/').to_string()
}

/// `~/.claude.json`.
pub fn state_path_in(home: &Path) -> PathBuf {
    home.join(".claude.json")
}

/// Read at most [`BUDGET_BYTES`] of a file, and say whether that bound
/// bound.
///
/// `take` rather than a whole-file read: the point is that the process
/// never HOLDS more than the budget, so a file that grew to a gigabyte
/// between the metadata call and the read still costs 4 MB. Reading
/// whole and then truncating the string would have already paid the
/// cost the budget exists to avoid.
///
/// Returns `(bytes, truncated, size)`. `size` is the file's length as
/// metadata reported it, which may disagree with what was read -- the
/// file is being rewritten -- and is used only for the notice.
fn read_bounded(path: &Path) -> std::io::Result<(Vec<u8>, bool, Option<u64>)> {
    let file = std::fs::File::open(path)?;
    let size = file.metadata().ok().map(|m| m.len());
    let (buf, truncated) = bounded_from(file)?;
    Ok((buf, truncated, size))
}

/// The bound itself, over any reader.
///
/// Split out from [`read_bounded`] so a test can hand it a reader that
/// COUNTS the bytes pulled from it. That distinction is not cosmetic:
/// reading the whole file and then truncating the buffer produces a
/// byte-identical result, and is exactly the flaw this bound exists to
/// prevent -- the process would still hold the whole file. A test
/// asserting only on the returned `Vec` cannot tell the two apart, so
/// it would pass against the broken implementation. Sabotage-proven:
/// see `the_bound_limits_what_is_pulled_from_the_reader`.
fn bounded_from<R: Read>(reader: R) -> std::io::Result<(Vec<u8>, bool)> {
    let mut buf = Vec::new();
    // BUDGET_BYTES + 1, so reading exactly the budget is distinguishable
    // from reading a file that is exactly the budget long. Without the
    // extra byte a file of precisely BUDGET_BYTES would be reported as
    // truncated when it was read whole.
    let mut handle = reader.take(BUDGET_BYTES + 1);
    handle.read_to_end(&mut buf)?;
    let truncated = buf.len() as u64 > BUDGET_BYTES;
    if truncated {
        buf.truncate(BUDGET_BYTES as usize);
    }
    Ok((buf, truncated))
}

/// Pull the `mcpServers` object out of a JSON object, as servers.
fn servers_from(obj: &Value, origin: Origin, detail: Option<&str>) -> Vec<Server> {
    let Some(map) = obj.get("mcpServers").and_then(Value::as_object) else {
        return Vec::new();
    };
    map.iter()
        .map(|(name, entry)| Server {
            name: name.clone(),
            transport: transport_of(entry),
            origin,
            scope_detail: detail.map(str::to_string),
        })
        .collect()
}

/// Describe how one entry is reached.
fn transport_of(entry: &Value) -> Transport {
    if let Some(url) = entry.get("url").and_then(Value::as_str) {
        return Transport::Url {
            url: url.to_string(),
        };
    }
    if let Some(cmd) = entry.get("command").and_then(Value::as_str) {
        // Arguments joined onto the command: `poetry` alone does not
        // identify a server, and `poetry run --directory .../enclave-mcp`
        // does. This is for DISPLAY -- it is never executed, so no
        // quoting rule is being implied.
        let args = entry
            .get("args")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        return Transport::Stdio {
            command: if args.is_empty() {
                cmd.to_string()
            } else {
                format!("{cmd} {args}")
            },
        };
    }
    Transport::Unknown
}

/// Every MCP server configured under `home`, with the plugins under
/// `plugins_root` that ship one.
///
/// `home` and `plugins_root` are both PARAMETERS so this is testable
/// without touching `$HOME` -- process-global state that would race
/// every other test in the binary, the reason
/// [`super::settings::effective_in`] and
/// [`super::definitions::scan_in`] give for the same choice.
pub fn inventory_in(home: &Path, plugins_root: &Path) -> Inventory {
    let mut out = Inventory::default();
    let path = state_path_in(home);

    match read_bounded(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Genuinely absent, which is a real answer: a machine where
            // Claude Code has never run has no MCP configuration, and
            // that is an empty inventory rather than a refusal. This is
            // the ONE case where empty is honest, and it is separated
            // from every other error for exactly that reason.
        }
        Err(e) => {
            out.unreadable.push(ScopeRefusal {
                origin: Origin::User,
                path: path.display().to_string(),
                detail: format!("{} could not be read: {e}", path.display()),
            });
        }
        Ok((bytes, truncated, size)) => {
            out.truncated = truncated;
            out.size_bytes = size;
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(v) => collect(&mut out, &v),
                Err(_) if truncated => {
                    // The cap bound, so the parse failure is EXPLAINED
                    // rather than mysterious, and the message says so.
                    // Reporting serde's position on a deliberately cut
                    // buffer would point the user at a line that is
                    // fine in the real file.
                    let total = size
                        .map(|s| format!("{s} bytes"))
                        .unwrap_or_else(|| "an unknown size".to_string());
                    out.unreadable.push(ScopeRefusal {
                        origin: Origin::User,
                        path: path.display().to_string(),
                        detail: format!(
                            "{} is {total}, larger than the {BUDGET_BYTES}-byte read budget, \
                             so only its first {BUDGET_BYTES} bytes were read and they do not \
                             parse as JSON. No MCP servers could be listed from it -- this is \
                             not a machine with none configured.",
                            path.display()
                        ),
                    });
                }
                Err(e) => {
                    // Malformed, or torn by a concurrent rewrite. serde's
                    // own line and column verbatim: it is the only
                    // actionable detail, and "invalid state file" is not.
                    out.unreadable.push(ScopeRefusal {
                        origin: Origin::User,
                        path: path.display().to_string(),
                        detail: format!(
                            "{} did not parse as JSON: {e}. Claude Code rewrites this file \
                             while it runs, so a partial read is possible -- no MCP servers \
                             could be listed from it, which is not the same as none being \
                             configured.",
                            path.display()
                        ),
                    });
                }
            }
        }
    }

    collect_plugins(&mut out, plugins_root);
    out
}

/// Pull user-scope and project-scope servers out of a parsed state file.
fn collect(out: &mut Inventory, v: &Value) {
    out.servers.extend(servers_from(v, Origin::User, None));

    let Some(projects) = v.get("projects").and_then(Value::as_object) else {
        return;
    };
    for (path, entry) in projects {
        out.servers
            .extend(servers_from(entry, Origin::Project, Some(path)));
    }
}

/// Every installed plugin's `.mcp.json`, as servers.
///
/// A directory that cannot be listed is a refusal rather than silence,
/// the rule [`super::definitions`] states: a permission wall hides an
/// unknown number of servers, and omitting them would understate the
/// inventory without saying so.
fn collect_plugins(out: &mut Inventory, root: &Path) {
    if !root.exists() {
        // No plugins directory at all is an honest zero: nothing is
        // installed. Distinct from one that exists and will not open.
        return;
    }
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(e) => {
            out.unreadable.push(ScopeRefusal {
                origin: Origin::Plugin,
                path: root.display().to_string(),
                detail: format!(
                    "{} could not be listed: {e}. Any MCP servers shipped by installed \
                     plugins are therefore unlisted rather than absent.",
                    root.display()
                ),
            });
            return;
        }
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    // Sorted so the inventory is stable between calls: `read_dir` order
    // is the filesystem's and a list that reorders itself on refresh
    // reads as having changed when it has not.
    dirs.sort();
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        let plugin = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // Both spellings, matching `plugins::read_contribution`, which
        // observed each in the real cache.
        for name in [".mcp.json", "mcp.json"] {
            let file = dir.join(name);
            if !file.exists() {
                continue;
            }
            match read_bounded(&file) {
                Ok((bytes, false, _)) => match serde_json::from_slice::<Value>(&bytes) {
                    Ok(v) => out
                        .servers
                        .extend(servers_from(&v, Origin::Plugin, Some(&plugin))),
                    Err(e) => out.unreadable.push(ScopeRefusal {
                        origin: Origin::Plugin,
                        path: file.display().to_string(),
                        detail: format!("{} did not parse as JSON: {e}", file.display()),
                    }),
                },
                Ok((_, true, size)) => {
                    let total = size
                        .map(|s| format!("{s} bytes"))
                        .unwrap_or_else(|| "an unknown size".to_string());
                    out.unreadable.push(ScopeRefusal {
                        origin: Origin::Plugin,
                        path: file.display().to_string(),
                        detail: format!(
                            "{} is {total}, larger than the {BUDGET_BYTES}-byte read budget, \
                             so the servers it ships could not be listed.",
                            file.display()
                        ),
                    });
                }
                Err(e) => out.unreadable.push(ScopeRefusal {
                    origin: Origin::Plugin,
                    path: file.display().to_string(),
                    detail: format!("{} could not be read: {e}", file.display()),
                }),
            }
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// `(tempdir, home, plugins_root)`. The plugins root is created so
    /// the "exists but empty" path is the default, rather than the
    /// absent-directory shortcut.
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let plugins = home.join(".claude").join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        (tmp, home, plugins)
    }

    #[test]
    fn a_user_scope_server_is_listed_with_its_transport() {
        let (_t, home, plugins) = fixture();
        write(
            &state_path_in(&home),
            r#"{"mcpServers": {"codegraph": {"type": "stdio", "command": "npx",
               "args": ["-y", "codegraph-mcp"]}}}"#,
        );

        let inv = inventory_in(&home, &plugins);
        assert!(inv.unreadable.is_empty(), "{:?}", inv.unreadable);
        assert_eq!(inv.servers.len(), 1);
        assert_eq!(inv.servers[0].name, "codegraph");
        assert_eq!(inv.servers[0].origin, Origin::User);
        assert_eq!(
            inv.servers[0].transport,
            Transport::Stdio {
                command: "npx -y codegraph-mcp".to_string()
            }
        );
        // No project, so no detail to give. The field is not a
        // placeholder for "user".
        assert_eq!(inv.servers[0].scope_detail, None);
    }

    /// #1216's second mandatory test. A server configured under one
    /// project must be attributed to THAT project, not to the user
    /// scope -- the confusion the whole feature exists to remove.
    #[test]
    fn a_project_scope_server_is_attributed_to_its_project() {
        let (_t, home, plugins) = fixture();
        write(
            &state_path_in(&home),
            r#"{
              "mcpServers": {"global": {"command": "g"}},
              "projects": {
                "/Users/x/code/one": {"mcpServers": {"atlassian": {"url": "https://a"}}},
                "/Users/x/code/two": {"lastCost": 1.5}
              }
            }"#,
        );

        let inv = inventory_in(&home, &plugins);
        assert!(inv.unreadable.is_empty(), "{:?}", inv.unreadable);

        let atl = inv.servers.iter().find(|s| s.name == "atlassian").unwrap();
        assert_eq!(atl.origin, Origin::Project);
        assert_eq!(atl.scope_detail.as_deref(), Some("/Users/x/code/one"));
        assert_eq!(
            atl.transport,
            Transport::Url {
                url: "https://a".to_string()
            }
        );

        // And it is NOT claimed by the user scope.
        let global = inv.servers.iter().find(|s| s.name == "global").unwrap();
        assert_eq!(global.origin, Origin::User);

        // A project with no `mcpServers` contributes nothing and is not
        // an error.
        assert_eq!(inv.servers.len(), 2);

        // In force in its own project, and not in the other one.
        let one: Vec<_> = inv
            .in_force(Path::new("/Users/x/code/one"))
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(one, vec!["global", "atlassian"]);
        let two: Vec<_> = inv
            .in_force(Path::new("/Users/x/code/two"))
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(two, vec!["global"]);
    }

    /// #1216's first mandatory test, and the module's whole point. A
    /// state file that does not parse must produce a REFUSAL carrying
    /// the parse position -- never an empty list, which would render as
    /// "no MCP servers are configured".
    #[test]
    fn a_malformed_state_file_is_a_refusal_not_zero_servers() {
        let (_t, home, plugins) = fixture();
        // Torn exactly as a concurrent rewrite tears it: a prefix.
        write(
            &state_path_in(&home),
            r#"{"mcpServers": {"codegraph": {"command": "npx"#,
        );

        let inv = inventory_in(&home, &plugins);
        assert!(inv.servers.is_empty());
        assert_eq!(inv.unreadable.len(), 1, "{:?}", inv.unreadable);
        let r = &inv.unreadable[0];
        assert_eq!(r.origin, Origin::User);
        // The parse LOCATION, so the user can act. Its absence is what
        // makes a refusal indistinguishable from a shrug.
        assert!(
            r.detail.contains("line") && r.detail.contains("column"),
            "refusal must carry the parse position: {}",
            r.detail
        );
        // And it must not have been reported as a size problem: the
        // file is tiny and the cap never bound.
        assert!(!inv.truncated);
        assert!(
            !r.detail.contains("read budget"),
            "a small malformed file must not be blamed on the budget: {}",
            r.detail
        );
    }

    /// #1216's third mandatory test, and the one a previous ticket got
    /// wrong by building a fixture small enough to be read whole.
    ///
    /// The fixture here is deliberately LARGER than [`BUDGET_BYTES`],
    /// so the bound genuinely binds, and the assertion is that the
    /// truncation is REPORTED -- both as the flag and in words.
    #[test]
    fn a_file_over_the_budget_binds_the_bound_and_says_so() {
        let (_t, home, plugins) = fixture();

        // Valid JSON whose `projects` map is padded past the budget, so
        // this is not merely a big blob: it is the real shape, grown to
        // the size the module docs say `projects` grows to.
        let mut body = String::from(r#"{"mcpServers": {"a": {"command": "x"}}, "projects": {"#);
        let mut i = 0usize;
        while body.len() as u64 <= BUDGET_BYTES + 1024 {
            if i > 0 {
                body.push(',');
            }
            body.push_str(&format!(
                r#""/Users/x/code/p{i}": {{"lastCost": 1.0, "exampleFiles": ["{}"]}}"#,
                "f".repeat(512)
            ));
            i += 1;
        }
        body.push_str("}}");
        let path = state_path_in(&home);
        write(&path, &body);

        let real = std::fs::metadata(&path).unwrap().len();
        assert!(
            real > BUDGET_BYTES,
            "fixture must exceed the budget or it proves nothing: {real} <= {BUDGET_BYTES}"
        );

        let inv = inventory_in(&home, &plugins);

        // The bound bound, and that fact is on the struct.
        assert!(inv.truncated, "the read was not reported as truncated");
        assert_eq!(inv.size_bytes, Some(real));

        // And it is REPORTED, in words, as a refusal -- not as an empty
        // list. A reader must be able to tell "too large to read" from
        // "nothing configured".
        assert!(inv.servers.is_empty());
        assert_eq!(inv.unreadable.len(), 1, "{:?}", inv.unreadable);
        let d = &inv.unreadable[0].detail;
        assert!(d.contains("read budget"), "{d}");
        assert!(d.contains(&real.to_string()), "must state the total: {d}");
        assert!(
            d.contains("not a machine with none configured"),
            "must distinguish refusal from zero: {d}"
        );
    }

    /// The bound must bound the PROCESS, not just the output. A read
    /// that slurped the whole file and then truncated the string would
    /// pass the test above while paying exactly the cost the budget
    /// exists to avoid.
    #[test]
    fn the_read_never_holds_more_than_the_budget() {
        let (_t, home, _p) = fixture();
        let path = state_path_in(&home);
        let mut body = String::from("{");
        body.push_str(&"x".repeat((BUDGET_BYTES + 4096) as usize));
        write(&path, &body);

        let (bytes, truncated, size) = read_bounded(&path).unwrap();
        assert!(truncated);
        assert_eq!(bytes.len() as u64, BUDGET_BYTES);
        assert_eq!(size, Some(body.len() as u64));
    }

    /// The bound must limit what is PULLED, not merely what is
    /// returned.
    ///
    /// This is the test that distinguishes the real bound from the
    /// flaw: `take(u64::MAX)` followed by truncating the buffer returns
    /// byte-identical output, so every assertion on the returned `Vec`
    /// passes against it while the process holds the entire file. A
    /// counting reader observes the difference directly.
    #[test]
    fn the_bound_limits_what_is_pulled_from_the_reader() {
        /// A reader with more bytes than the budget, which counts how
        /// many are actually taken from it.
        struct Counting {
            pulled: std::rc::Rc<std::cell::Cell<u64>>,
            left: u64,
        }
        impl Read for Counting {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                let n = out.len().min(self.left as usize);
                out[..n].fill(b'z');
                self.left -= n as u64;
                self.pulled.set(self.pulled.get() + n as u64);
                Ok(n)
            }
        }

        let pulled = std::rc::Rc::new(std::cell::Cell::new(0u64));
        let reader = Counting {
            pulled: std::rc::Rc::clone(&pulled),
            // Far more than the budget, so an unbounded read is obvious.
            left: BUDGET_BYTES * 4,
        };

        let (buf, truncated) = bounded_from(reader).unwrap();
        assert!(truncated);
        assert_eq!(buf.len() as u64, BUDGET_BYTES);
        // The load-bearing assertion. At most one byte beyond the
        // budget is pulled -- the probe byte that detects truncation --
        // and never the whole 16 MB.
        assert!(
            pulled.get() <= BUDGET_BYTES + 1,
            "the bound did not limit the read: {} bytes were pulled for a {BUDGET_BYTES}-byte budget",
            pulled.get()
        );
    }

    /// A file exactly the budget long is read WHOLE, not reported as
    /// truncated. The off-by-one the `+ 1` in `read_bounded` exists for.
    #[test]
    fn a_file_exactly_the_budget_is_not_truncated() {
        let (_t, home, _p) = fixture();
        let path = state_path_in(&home);
        write(&path, &"y".repeat(BUDGET_BYTES as usize));

        let (bytes, truncated, _) = read_bounded(&path).unwrap();
        assert!(!truncated);
        assert_eq!(bytes.len() as u64, BUDGET_BYTES);
    }

    /// An absent state file is the one honest empty: Claude Code has
    /// never run here. It must NOT be a refusal, or every fresh machine
    /// would show an error.
    #[test]
    fn an_absent_state_file_is_empty_not_a_refusal() {
        let (_t, home, plugins) = fixture();
        let inv = inventory_in(&home, &plugins);
        assert!(inv.servers.is_empty());
        assert!(inv.unreadable.is_empty());
        assert!(!inv.truncated);
    }

    /// A parsing file with no `mcpServers` key is a genuine zero, and
    /// must be distinguishable from the refusal above.
    #[test]
    fn a_state_file_with_no_servers_is_a_real_zero() {
        let (_t, home, plugins) = fixture();
        write(
            &state_path_in(&home),
            r#"{"projects": {}, "autoUpdates": true}"#,
        );

        let inv = inventory_in(&home, &plugins);
        assert!(inv.servers.is_empty());
        assert!(
            inv.unreadable.is_empty(),
            "a readable file with no servers is not a refusal: {:?}",
            inv.unreadable
        );
    }

    #[test]
    fn a_plugin_shipped_server_carries_the_plugin_origin() {
        let (_t, home, plugins) = fixture();
        write(&state_path_in(&home), r#"{"mcpServers": {}}"#);
        write(
            &plugins.join("github").join(".mcp.json"),
            r#"{"mcpServers": {"github": {"command": "gh-mcp"}}}"#,
        );

        let inv = inventory_in(&home, &plugins);
        assert!(inv.unreadable.is_empty(), "{:?}", inv.unreadable);
        assert_eq!(inv.servers.len(), 1);
        assert_eq!(inv.servers[0].origin, Origin::Plugin);
        assert_eq!(inv.servers[0].scope_detail.as_deref(), Some("github"));
        // Plugin servers apply everywhere, so they are in force in any
        // repository.
        assert_eq!(inv.in_force(Path::new("/anywhere")).len(), 1);
    }

    /// A plugin's unparseable `.mcp.json` is its own scope's refusal and
    /// must not take the user scope's servers down with it.
    #[test]
    fn a_broken_plugin_manifest_refuses_only_its_own_scope() {
        let (_t, home, plugins) = fixture();
        write(
            &state_path_in(&home),
            r#"{"mcpServers": {"ok": {"command": "c"}}}"#,
        );
        write(&plugins.join("broken").join(".mcp.json"), "{not json");

        let inv = inventory_in(&home, &plugins);
        // The user scope still answered.
        assert_eq!(inv.servers.len(), 1);
        assert_eq!(inv.servers[0].name, "ok");
        // And the plugin scope said it could not.
        assert_eq!(inv.unreadable.len(), 1);
        assert_eq!(inv.unreadable[0].origin, Origin::Plugin);
    }

    /// An entry with neither `command` nor `url` is LISTED as unknown
    /// rather than dropped: a server we cannot describe is still
    /// configured, and omitting it would understate the inventory.
    #[test]
    fn an_unrecognised_entry_is_listed_as_unknown_not_dropped() {
        let (_t, home, plugins) = fixture();
        write(
            &state_path_in(&home),
            r#"{"mcpServers": {"future": {"transport": "something-new"}}}"#,
        );

        let inv = inventory_in(&home, &plugins);
        assert_eq!(inv.servers.len(), 1);
        assert_eq!(inv.servers[0].transport, Transport::Unknown);
    }

    /// `Origin::Plugin` must sort BELOW every settings scope, which is
    /// what keeps `settings::effective_in`'s `>` comparisons correct
    /// after the enum grew. Pinned here because the consequence of
    /// getting it wrong is silent and one view away.
    #[test]
    fn the_plugin_origin_sorts_below_every_settings_scope() {
        assert!(Origin::Plugin < Origin::User);
        assert!(Origin::Plugin < Origin::Project);
        assert!(Origin::Plugin < Origin::Local);
        // And the settings order is still exactly the three files.
        assert_eq!(
            Origin::ORDER,
            [Origin::User, Origin::Project, Origin::Local]
        );
    }

    /// A trailing separator must not hide a project's servers.
    #[test]
    fn a_trailing_separator_does_not_change_what_is_in_force() {
        let (_t, home, plugins) = fixture();
        write(
            &state_path_in(&home),
            r#"{"projects": {"/Users/x/code/one/": {"mcpServers": {"a": {"command": "c"}}}}}"#,
        );

        let inv = inventory_in(&home, &plugins);
        assert_eq!(inv.in_force(Path::new("/Users/x/code/one")).len(), 1);
    }
}
