//! Provider-neutral identity. `repo` and `number` retain their existing wire
//! names: a full project path and its local PR number / merge-request IID.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Github,
    Gitlab,
}

pub type Host = String;
pub type ProjectPath = String;
pub type PrNumber = u64;

/// Host is explicit even for GitLab.com. It is an authority (no scheme/path).
/// An absent source on an old payload means GitHub.com. A present source must
/// supply BOTH fields; never default a malformed GitLab source to GitHub.com.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Source {
    pub provider: Provider,
    pub host: Host,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            provider: Provider::Github,
            host: "github.com".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrIdentity {
    #[serde(default)]
    pub source: Source,
    pub repo: ProjectPath,
    pub number: PrNumber,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn identity_distinguishes_provider_host_full_project_and_iid() {
        let identities = [
            (Provider::Github, "github.com", "group/project", 7),
            (Provider::Gitlab, "github.com", "group/project", 7),
            (Provider::Gitlab, "gitlab.com", "group/project", 7),
            (Provider::Gitlab, "gitlab.example", "group/project", 7),
            (Provider::Gitlab, "gitlab.com", "group/subgroup/project", 7),
            (Provider::Gitlab, "gitlab.com", "group/project", 8),
        ]
        .map(|(provider, host, repo, number)| PrIdentity {
            source: Source {
                provider,
                host: host.into(),
            },
            repo: repo.into(),
            number,
        });
        assert_eq!(identities.iter().collect::<HashSet<_>>().len(), 6);
        for identity in identities {
            let json = serde_json::to_value(&identity).unwrap();
            assert_eq!(
                serde_json::from_value::<PrIdentity>(json).unwrap(),
                identity
            );
        }
    }

    #[test]
    fn old_identity_defaults_only_an_absent_source() {
        let old = serde_json::json!({"repo": "group/project", "number": 7});
        let identity: PrIdentity = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(identity.source, Source::default());
        for source in [
            serde_json::json!({"provider": "gitlab"}),
            serde_json::json!({"host": "gitlab.com"}),
            serde_json::json!({"provider": "other", "host": "example.com"}),
            serde_json::Value::Null,
        ] {
            let mut malformed = old.clone();
            malformed["source"] = source;
            assert!(serde_json::from_value::<PrIdentity>(malformed).is_err());
        }
    }
}
