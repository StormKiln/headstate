//! Measurement age is distinct from the time a revalidation was attempted.
use super::fetch::{Outcome, Series};
use crate::store::stats_owner::StatsOwner;
use chrono::{DateTime, Utc};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsReceipt {
    pub fetched_at: DateTime<Utc>,
    pub reused: bool,
    pub retained: bool,
    pub qualification: Option<String>,
    pub owner: StatsOwner,
}
impl StatsReceipt {
    pub fn measured(now: DateTime<Utc>, owner: StatsOwner) -> Self {
        Self {
            fetched_at: now,
            reused: false,
            retained: false,
            qualification: None,
            owner,
        }
    }
}

/// Counts overlap: keeping the useful older floor is safe; summing is not.
pub fn retain_count(new: &mut Outcome, old: &Outcome) -> bool {
    if !new.is_complete() && old.total > new.total {
        let spend = new.spend.clone();
        *new = old.clone();
        new.spend = spend;
        true
    } else {
        false
    }
}

/// Only fill absent days; a newly measured zero replaces an earlier count.
pub fn retain_series(new: &mut Series, old: &Series) -> bool {
    let mut retained = false;
    for point in &old.points {
        if new.failed_days.contains(&point.date) {
            new.points.push(point.clone());
            new.failed_days.retain(|d| d != &point.date);
            retained = true;
        }
    }
    new.points.sort_by(|a, b| a.date.cmp(&b.date));
    retained
}
