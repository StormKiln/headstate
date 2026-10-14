//! Suggested locations only: finding a cache never grants deletion authority.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
#[derive(Clone)]
pub(super) struct Provider {
    pub path: PathBuf,
    pub category: &'static str,
    pub evidence: String,
}
pub(super) fn current() -> Vec<Provider> {
    crate::auth::home_dir()
        .map(|home| {
            known_locations(
                &home,
                std::env::consts::OS,
                &std::env::vars_os()
                    .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                    .collect(),
            )
        })
        .unwrap_or_default()
}
fn no_links(path: &Path) -> bool {
    path.ancestors()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .all(|p| std::fs::symlink_metadata(p).is_ok_and(|m| !m.file_type().is_symlink()))
}
/// Read simple literal user cache paths only. No expansion, scripts or tool invocation.
fn configured_path(home: &Path, file: &str, key: &str) -> Option<PathBuf> {
    use std::io::Read;
    let path = home.join(file);
    if !no_links(&path) {
        return None;
    }
    let mut raw = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(64 * 1024 + 1)
        .read_to_string(&mut raw)
        .ok()?;
    if raw.len() > 64 * 1024 {
        return None;
    }
    let value = raw
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (k, v) = if file == ".npmrc" {
                line.split_once('=')?
            } else if file.ends_with(".yml") {
                line.split_once(':')?
            } else {
                line.split_once(char::is_whitespace)?
            };
            (k.trim() == key).then_some(v.trim().trim_matches('"').trim_matches('\''))
        })
        .next_back()?;
    if value.contains('$') || value.contains('`') {
        return None;
    }
    let path = value
        .strip_prefix("~/")
        .map(|v| home.join(v))
        .unwrap_or_else(|| PathBuf::from(value));
    path.is_absolute().then_some(path)
}
fn known_locations(home: &Path, os: &str, env: &BTreeMap<String, String>) -> Vec<Provider> {
    let absolute = |name: &str| env.get(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let cache = absolute("XDG_CACHE_HOME").unwrap_or_else(|| {
        if os == "macos" {
            home.join("Library/Caches")
        } else if os == "windows" {
            absolute("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData/Local"))
        } else {
            home.join(".cache")
        }
    });
    let cargo = absolute("CARGO_HOME").unwrap_or_else(|| home.join(".cargo"));
    let rustup = absolute("RUSTUP_HOME").unwrap_or_else(|| home.join(".rustup"));
    let mut candidates = vec![
        (
            absolute("YARN_CACHE_FOLDER")
                .or_else(|| configured_path(home, ".yarnrc", "cache-folder"))
                .unwrap_or_else(|| cache.join(if os == "macos" { "Yarn" } else { "yarn" })),
            "Package caches",
            "Yarn cache",
        ),
        (
            absolute("npm_config_cache")
                .or_else(|| absolute("NPM_CONFIG_CACHE"))
                .or_else(|| configured_path(home, ".npmrc", "cache"))
                .unwrap_or_else(|| {
                    if os == "windows" {
                        cache.join("npm-cache")
                    } else {
                        home.join(".npm")
                    }
                }),
            "Package caches",
            "npm cache",
        ),
        (
            absolute("UV_CACHE_DIR")
                .unwrap_or_else(|| cache.join(if os == "windows" { "uv/cache" } else { "uv" })),
            "Package caches",
            "uv cache",
        ),
        (
            cargo.join("registry"),
            "Package caches",
            "Cargo registry cache",
        ),
        (cargo.join("git"), "Package caches", "Cargo Git cache"),
        (
            rustup.join("toolchains"),
            "Toolchains",
            "Rust toolchains; installed tools, not build output",
        ),
        (
            rustup.join("downloads"),
            "Package caches",
            "Rust toolchain downloads",
        ),
        (
            cache.join("org.swift.swiftpm"),
            "Package caches",
            "Swift package cache",
        ),
    ];
    candidates.extend([
        (home.join(".cache/uv"), "Package caches", "uv XDG cache"),
        (home.join(".cache/yarn"), "Package caches", "Yarn XDG cache"),
        (
            home.join(".yarn/berry/cache"),
            "Package caches",
            "Yarn Berry cache",
        ),
    ]);
    if let Some(global) = configured_path(home, ".yarnrc.yml", "globalFolder") {
        candidates.push((
            global.join("cache"),
            "Package caches",
            "Yarn configured global cache",
        ));
    }
    if let Some(cache) = configured_path(home, ".yarnrc.yml", "cacheFolder") {
        candidates.push((cache, "Package caches", "Yarn configured cache"));
    }
    if os == "macos" {
        candidates.extend([
            (
                home.join("Library/Developer/Xcode/DerivedData"),
                "Build caches",
                "Xcode DerivedData",
            ),
            (
                home.join("Library/Developer/Xcode/Archives"),
                "Build archives",
                "Xcode archives; may contain signing and debugging evidence",
            ),
            (
                home.join("Library/Developer/Xcode/iOS DeviceSupport"),
                "Toolchains",
                "Xcode device support",
            ),
            (
                home.join("Library/Developer/CoreSimulator/Devices"),
                "Simulator user data",
                "Simulator devices; may contain application and user data",
            ),
        ]);
    }
    let mut seen = BTreeSet::new();
    candidates.into_iter().filter_map(|(path, category, label)| {
        if !no_links(&path) || !path.is_dir() { return None; }
        let path = path.canonicalize().ok()?;
        if !seen.insert(path.clone()) { return None; }
        Some(Provider { path, category, evidence: format!("{label}: known tool location, user tool configuration, or current application environment override. Project attribution is unknown; historical tool environment and CLI overrides are not inferred.") })
    }).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn injected_standard_override_and_absent_paths_are_deduplicated() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(home.join("Library/Caches/Yarn")).unwrap();
        std::fs::create_dir_all(home.join("custom-uv")).unwrap();
        let env = BTreeMap::from([
            (
                "UV_CACHE_DIR".into(),
                home.join("custom-uv").display().to_string(),
            ),
            (
                "YARN_CACHE_FOLDER".into(),
                home.join("Library/Caches/Yarn").display().to_string(),
            ),
        ]);
        let found = known_locations(&home, "macos", &env);
        assert_eq!(
            found
                .iter()
                .filter(|p| p.path.ends_with("Library/Caches/Yarn"))
                .count(),
            1
        );
        assert!(found.iter().any(|p| p.path.ends_with("custom-uv")));
        assert!(!found.iter().any(|p| p.path.ends_with(".npm")));
    }
    #[test]
    fn literal_tool_config_and_platform_defaults_are_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(home.join("configured-npm")).unwrap();
        std::fs::create_dir_all(home.join(".cache/uv")).unwrap();
        std::fs::write(home.join(".npmrc"), "cache=~/configured-npm\n").unwrap();
        let found = known_locations(&home, "linux", &BTreeMap::new());
        assert!(found.iter().any(|p| p.path.ends_with("configured-npm")));
        assert!(found.iter().any(|p| p.path.ends_with(".cache/uv")));
        std::fs::create_dir_all(home.join("AppData/Local/npm-cache")).unwrap();
        std::fs::remove_file(home.join(".npmrc")).unwrap();
        assert!(known_locations(&home, "windows", &BTreeMap::new())
            .iter()
            .any(|p| p.path.ends_with("AppData/Local/npm-cache")));
    }
    #[test]
    fn macos_includes_berry_and_xdg_uv_alongside_library_caches() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        for path in [
            ".yarn/berry/cache",
            ".cache/uv",
            "Library/Caches/uv",
            "Library/Caches/Yarn",
            "custom-berry/cache",
        ] {
            std::fs::create_dir_all(home.join(path)).unwrap();
        }
        std::fs::write(home.join(".yarnrc.yml"), "globalFolder: '~/custom-berry'\n").unwrap();
        let found = known_locations(&home, "macos", &BTreeMap::new());
        for suffix in [
            ".yarn/berry/cache",
            ".cache/uv",
            "Library/Caches/uv",
            "Library/Caches/Yarn",
            "custom-berry/cache",
        ] {
            assert!(
                found.iter().any(|p| p.path.ends_with(suffix)),
                "missing {suffix}"
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn provider_does_not_follow_a_symlinked_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("uv")).unwrap();
        std::os::unix::fs::symlink(outside.path(), home.join(".cache")).unwrap();
        assert!(known_locations(&home, "linux", &BTreeMap::new()).is_empty());
    }
}
