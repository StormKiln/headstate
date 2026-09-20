//! What Claude Code actually reads, across the scopes it merges (#1130).
//!
//! Claude Code merges settings from a user file, a project file and a
//! local file. Headstate read exactly ONE of them, for exactly one
//! purpose: `install.rs` opens `~/.claude/settings.json` to find and
//! manipulate the `hooks` key, and `hooks_mut` is its only accessor.
//! `permissions` appears in this repository only as two test fixtures.
//!
//! So a user debugging "why did that tool get denied" had to open three
//! files and merge them mentally -- while `events.rs` was already
//! recording every `PermissionDenied` and tallying it by tool. The
//! effect was visible and the rule that caused it was not.
//!
//! # An unparsed scope is Unknown, not absent
//!
//! The load-bearing rule here. If one of the three files cannot be
//! parsed, the merge RESULT is unknown -- not "that scope contributed
//! nothing". Collapsing those is #1042's shape: a scope we could not
//! read and a scope that is genuinely empty produce the same merged
//! value by different routes, and only one of them is an answer.
//!
//! So a refusal travels per scope, and a key whose winner sits under an
//! unreadable scope is reported as undecidable rather than resolved.
//!
//! # Read-only
//!
//! Nothing here writes. Authoring a permission rule needs an answer to
//! "how do we recognise what we wrote", and a rule has no fingerprint --
//! `"Bash(git status:*)"` written by Headstate is byte-identical to one
//! the user typed. That is deferred with its own design.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

use super::install::{read_settings, Refusal};

/// Which file a value came from, in Claude Code's precedence order.
///
/// Later beats earlier: a project file overrides the user's, and a local
/// file overrides both.
///
/// # Why `Plugin` lives in this enum (#1216)
///
/// The MCP inventory needed a fourth scope: a server can be shipped by
/// an installed plugin's `.mcp.json`, which is not any of the three
/// settings files. The choice was to EXTEND this enum or to wrap it in
/// a second one carrying `Origin` plus a plugin case.
///
/// Extending, because wrapping would give the MCP page a vocabulary the
/// settings page does not speak -- two enums for one question ("which
/// scope defines this"), which is exactly what #1216 asked not to
/// happen. One enum means `ORIGIN_LABEL` on the frontend is one table
/// and a reader learns the word "project" once.
///
/// `Plugin` is declared FIRST, so it is the LOWEST precedence in the
/// derived `Ord`. That is load-bearing twice over. It is correct --
/// Claude Code lets a user or project `mcpServers` entry shadow a
/// plugin's server of the same name -- and it is what keeps
/// [`effective_in`] below untouched: that function compares origins
/// with `>` to decide whether an unreadable scope could have overridden
/// a winner, and a variant beneath every settings scope can never
/// change one of those answers. No settings file ever yields `Plugin`,
/// and [`Origin::ORDER`] deliberately still lists only the three that
/// name a settings file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    /// An installed plugin's `.mcp.json`. Not a settings file, and not
    /// produced by [`effective_in`] -- see the enum's docs for why it
    /// sits here anyway, and why it sorts lowest.
    Plugin,
    /// `~/.claude/settings.json`
    User,
    /// `<repo>/.claude/settings.json` -- committed, shared with the team.
    Project,
    /// `<repo>/.claude/settings.local.json` -- not committed.
    Local,
}

impl Origin {
    /// Every SETTINGS scope, lowest precedence first.
    ///
    /// Three, not four: `Plugin` names no settings file, so adding it
    /// here would make [`effective_in`] try to read
    /// `<repo>/.claude/settings.json`-shaped path that does not exist
    /// and report a refusal for a scope that was never in question.
    pub const ORDER: [Origin; 3] = [Origin::User, Origin::Project, Origin::Local];

    fn path(self, home: &Path, repo: &Path) -> PathBuf {
        match self {
            Origin::User => home.join(".claude").join("settings.json"),
            Origin::Project => repo.join(".claude").join("settings.json"),
            Origin::Local => repo.join(".claude").join("settings.local.json"),
            // Unreachable through `ORDER`, which is the only caller.
            // Returning the user path rather than panicking: a panic
            // here would be a crash in a read-only inventory.
            Origin::Plugin => home.join(".claude").join("settings.json"),
        }
    }
}

/// One scope's contribution to a key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contribution {
    pub origin: Origin,
    /// The value that scope carried, rendered as JSON.
    pub value: Value,
}

/// A key, its winner, and everyone who also had an opinion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedKey {
    pub key: String,
    /// The value that wins, or `None` when it cannot be decided.
    ///
    /// `None` means a scope with HIGHER precedence than the best
    /// readable one could not be parsed, so what it carried -- possibly
    /// nothing, possibly an override -- is unknown. Rendering the
    /// lower scope's value as the winner would be a confident wrong
    /// answer.
    pub winner: Option<Contribution>,
    /// Every scope that carried this key, in precedence order.
    pub contributions: Vec<Contribution>,
    /// Whether an unreadable scope could have changed the answer.
    pub undecidable: bool,
}

/// The merged view, plus what could not be read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Effective {
    pub keys: Vec<ResolvedKey>,
    /// Scopes that exist and could not be parsed, with the refusal's own
    /// sentence.
    pub unreadable: Vec<ScopeRefusal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeRefusal {
    pub origin: Origin,
    pub path: String,
    /// The `Refusal`'s own message. It names the file and the parse
    /// error with its line and column, which is the only actionable
    /// thing we have.
    pub detail: String,
}

/// The keys worth resolving.
///
/// A short list rather than every key in the file. Re-implementing
/// Claude Code's whole merge algorithm would be a second source of truth
/// for someone else's behaviour, and wrong the moment it changes. These
/// four are the ones a user debugging a denial or a surprising model
/// actually asks about, and each is a plain last-wins scalar or object.
pub const KEYS: &[&str] = &["permissions", "model", "env", "outputStyle"];

/// Resolve the tracked keys across the three scopes.
///
/// `home` and `repo` are both PARAMETERS so this is testable without
/// touching `$HOME` -- process-global state that would race every other
/// test in the binary.
pub fn effective_in(home: &Path, repo: &Path) -> Effective {
    let mut out = Effective::default();

    // Read every scope FIRST, so a refusal is known before any key is
    // resolved. Resolving as we go would let an early key be decided
    // against a scope we later discover we could not read.
    let mut parsed: Vec<(Origin, Option<Value>)> = Vec::new();
    for origin in Origin::ORDER {
        let path = origin.path(home, repo);
        match read_settings(&path) {
            Ok(v) => parsed.push((origin, v)),
            Err(r) => {
                out.unreadable.push(ScopeRefusal {
                    origin,
                    path: path.display().to_string(),
                    detail: refusal_text(&r),
                });
                // Recorded as unreadable and NOT pushed: it contributes
                // nothing we can name, and the `undecidable` flag below
                // is what carries the fact that it might have.
            }
        }
    }

    for key in KEYS {
        let mut contributions = Vec::new();
        for (origin, value) in &parsed {
            let Some(obj) = value.as_ref().and_then(Value::as_object) else {
                continue;
            };
            if let Some(v) = obj.get(*key) {
                contributions.push(Contribution {
                    origin: *origin,
                    value: v.clone(),
                });
            }
        }

        // The winner is the highest-precedence scope that carried it.
        let winner = contributions.last().cloned();

        // Could an unreadable scope have overridden that winner?
        //
        // Only one with HIGHER precedence. An unreadable user file
        // cannot change an answer a local file already decided, and
        // saying otherwise would attach a warning to a key that is not
        // in doubt -- which is how a qualification becomes noise.
        let floor = winner.as_ref().map(|c| c.origin);
        let undecidable = out.unreadable.iter().any(|r| match floor {
            Some(w) => r.origin > w,
            // No readable scope carried it at all, so any unreadable
            // scope could have.
            None => true,
        });

        if contributions.is_empty() && !undecidable {
            continue;
        }
        out.keys.push(ResolvedKey {
            key: (*key).to_string(),
            winner: if undecidable { None } else { winner },
            contributions,
            undecidable,
        });
    }

    out
}

/// A refusal as its own sentence.
fn refusal_text(r: &Refusal) -> String {
    r.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        (tmp, home, repo)
    }

    #[test]
    fn a_local_file_overrides_the_user_file() {
        let (_t, home, repo) = fixture();
        write(
            &home.join(".claude").join("settings.json"),
            r#"{"model": "sonnet"}"#,
        );
        write(
            &repo.join(".claude").join("settings.local.json"),
            r#"{"model": "opus"}"#,
        );

        let eff = effective_in(&home, &repo);
        let model = eff.keys.iter().find(|k| k.key == "model").unwrap();
        assert_eq!(model.winner.as_ref().unwrap().origin, Origin::Local);
        assert_eq!(model.winner.as_ref().unwrap().value, "opus");
        // Both are still listed: the question "where did this come
        // from" needs the losers as much as the winner.
        assert_eq!(model.contributions.len(), 2);
    }

    /// #1042's shape. A scope that could not be parsed is not a scope
    /// that contributed nothing -- and if it outranks the winner, the
    /// merged value is UNKNOWN rather than the lower scope's.
    #[test]
    fn an_unparsed_higher_scope_makes_the_key_undecidable() {
        let (_t, home, repo) = fixture();
        write(
            &home.join(".claude").join("settings.json"),
            r#"{"model": "sonnet"}"#,
        );
        write(
            &repo.join(".claude").join("settings.local.json"),
            "{ not json",
        );

        let eff = effective_in(&home, &repo);
        assert_eq!(eff.unreadable.len(), 1);
        let model = eff.keys.iter().find(|k| k.key == "model").unwrap();
        assert!(
            model.undecidable,
            "a higher scope we could not read may have overridden it"
        );
        assert!(
            model.winner.is_none(),
            "so no winner may be claimed -- the lower value is not the answer"
        );
    }

    /// The other direction, which is what stops the qualification
    /// becoming noise: an unreadable LOWER scope cannot change an answer
    /// a higher one already decided.
    #[test]
    fn an_unparsed_lower_scope_does_not_make_a_decided_key_undecidable() {
        let (_t, home, repo) = fixture();
        write(&home.join(".claude").join("settings.json"), "{ not json");
        write(
            &repo.join(".claude").join("settings.local.json"),
            r#"{"model": "opus"}"#,
        );

        let eff = effective_in(&home, &repo);
        assert_eq!(eff.unreadable.len(), 1, "the user file is still reported");
        let model = eff.keys.iter().find(|k| k.key == "model").unwrap();
        assert!(
            !model.undecidable,
            "local already wins; user cannot outrank it"
        );
        assert_eq!(model.winner.as_ref().unwrap().value, "opus");
    }

    /// An ABSENT file is not an unreadable one. Most repositories have
    /// no `.claude/settings.json`, and reporting that would make the
    /// honest signal worthless.
    #[test]
    fn absent_scopes_are_not_reported_as_unreadable() {
        let (_t, home, repo) = fixture();
        write(
            &home.join(".claude").join("settings.json"),
            r#"{"model": "sonnet"}"#,
        );

        let eff = effective_in(&home, &repo);
        assert!(eff.unreadable.is_empty());
        assert!(!eff.keys.iter().any(|k| k.undecidable));
    }

    /// A key nobody set is simply absent from the result, rather than
    /// listed with a null winner -- which would read as "set to
    /// nothing".
    #[test]
    fn a_key_nobody_set_is_not_listed() {
        let (_t, home, repo) = fixture();
        write(
            &home.join(".claude").join("settings.json"),
            r#"{"model": "sonnet"}"#,
        );

        let eff = effective_in(&home, &repo);
        assert!(!eff.keys.iter().any(|k| k.key == "outputStyle"));
    }

    /// Permissions are the key this feature exists for: `events.rs`
    /// records every denial and could not show the rule behind it.
    #[test]
    fn permissions_resolve_across_scopes() {
        let (_t, home, repo) = fixture();
        write(
            &home.join(".claude").join("settings.json"),
            r#"{"permissions": {"deny": ["Bash(rm:*)"]}}"#,
        );
        write(
            &repo.join(".claude").join("settings.json"),
            r#"{"permissions": {"allow": ["Bash(git status:*)"]}}"#,
        );

        let eff = effective_in(&home, &repo);
        let perms = eff.keys.iter().find(|k| k.key == "permissions").unwrap();
        assert_eq!(perms.contributions.len(), 2);
        assert_eq!(perms.winner.as_ref().unwrap().origin, Origin::Project);
    }
}
