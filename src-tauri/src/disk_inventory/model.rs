use serde::{Deserialize, Serialize};

pub const METHOD: &str = "allocated-identity-v1";
pub const MAX_ROOTS: usize = 32;
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    pub external_roots: Vec<String>,
    pub additional_locations: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Coverage {
    pub affects_completeness: bool,
    pub path: String,
    pub reason: String,
}
/// Byte counts are decimal strings: a Rust u64 need not fit a JS Number.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Volume {
    pub id: String,
    #[serde(skip)]
    pub device: String,
    pub identity_stable: bool,
    pub mount: String,
    pub label: String,
    pub scope: String,
    pub capacity: Option<String>,
    pub used: Option<String>,
    pub available: Option<String>,
    pub method: String,
    pub limitation: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Location {
    pub id: String,
    pub identity_stable: bool,
    pub path: String,
    pub aliases: Vec<String>,
    pub owners: Vec<String>,
    pub category: String,
    pub evidence: Vec<String>,
    pub logical: Option<String>,
    pub allocated: Option<String>,
    pub reclaimable: Option<String>,
    pub complete: bool,
    pub active: bool,
    pub review_only: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Category {
    pub name: String,
    pub allocated: Option<String>,
    pub logical: Option<String>,
    pub locations: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Change {
    pub location_id: String,
    pub path: String,
    pub bytes: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Comparison {
    pub baseline_at: Option<i64>,
    pub reason: Option<String>,
    pub changes: Vec<Change>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Observation {
    pub id: String,
    pub volume: Volume,
    pub method: String,
    pub configuration: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub status: String,
    pub visited: u64,
    pub locations: Vec<Location>,
    pub categories: Vec<Category>,
    pub coverage: Vec<Coverage>,
    pub measured: Option<String>,
    pub remainder: Option<String>,
    pub accounting_note: Option<String>,
    pub approximate: bool,
    pub comparison: Comparison,
    pub history_error: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Status {
    pub run_id: Option<String>,
    pub running: bool,
    pub visited: u64,
    pub current_path: Option<String>,
    pub observations: Vec<Observation>,
    pub error: Option<String>,
}

/// Keep error-heavy walks bounded without turning omitted failures into success.
pub trait CoverageNotes {
    fn note(&mut self, item: Coverage);
}
impl CoverageNotes for Vec<Coverage> {
    fn note(&mut self, item: Coverage) {
        if self.len() < 999 {
            self.push(item);
        } else if self.len() == 999 {
            self.push(Coverage { affects_completeness: true, path: String::new(), reason: "Additional coverage details exceeded the 1,000-note limit; some locations remain unknown.".into() });
        }
    }
}
