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

/// Called while the owner's immediate publication transaction is held.
/// Reread receipts can include progress published while this caller was on HTTP.
fn reconcile_current(
    key: &str,
    incoming: crate::store::stats::Cached,
    current: crate::store::stats::Cached,
) -> crate::store::stats::Cached {
    let readable = if key.starts_with("series|") {
        serde_json::from_str::<Series>(&current.payload).is_ok()
    } else if key.starts_with("count|") {
        serde_json::from_str::<Outcome>(&current.payload).is_ok()
    } else {
        return incoming;
    };
    if !readable {
        return incoming;
    }
    if current.complete && (!incoming.complete || current.fetched_at > incoming.fetched_at) {
        return current;
    }
    if incoming.complete {
        return incoming;
    }
    if key.starts_with("series|") {
        if let (Ok(mut candidate), Ok(mut saved)) = (
            serde_json::from_str::<Series>(&incoming.payload),
            serde_json::from_str::<Series>(&current.payload),
        ) {
            let (base, other, mut selected, other_age) = if current.fetched_at > incoming.fetched_at
            {
                (&mut saved, &candidate, current.clone(), incoming.fetched_at)
            } else {
                (&mut candidate, &saved, incoming.clone(), current.fetched_at)
            };
            if retain_series(base, other) {
                selected.fetched_at = selected.fetched_at.min(other_age);
                selected.complete = false;
                if let Some(receipt) = &mut base.receipt {
                    receipt.fetched_at = selected.fetched_at;
                    receipt.retained = true;
                    receipt.qualification = Some("Compatible saved measurements are included; retry to refresh missing measurements.".into());
                }
                selected.total = base.points.iter().map(|point| point.merged).sum();
                if let Ok(payload) = serde_json::to_string(base) {
                    selected.payload = payload;
                    return selected;
                }
            } else {
                return selected;
            }
        }
    } else if key.starts_with("count|") {
        if let (Ok(candidate), Ok(saved)) = (
            serde_json::from_str::<Outcome>(&incoming.payload),
            serde_json::from_str::<Outcome>(&current.payload),
        ) {
            if saved.total > candidate.total
                || (saved.total == candidate.total && current.fetched_at > incoming.fetched_at)
            {
                return current;
            }
        }
    }
    incoming
}

/// Keep the current call's actual measurements separate from its pre-HTTP
/// fallback. Only measured values may replace a concurrent measurement;
/// held fallback may fill holes but must not overwrite a measured zero.
fn select_measurements(
    key: &str,
    incoming: crate::store::stats::Cached,
    current: crate::store::stats::Cached,
    fresh: Option<crate::store::stats::Cached>,
) -> crate::store::stats::Cached {
    if !key.starts_with("series|") && !key.starts_with("count|") {
        return incoming;
    }
    let mut published = match fresh {
        Some(fresh) => reconcile_current(key, fresh, current),
        // An errored load measured nothing; its held fallback cannot replace
        // a value published while it was waiting on HTTP.
        None => current,
    };
    if published.complete {
        return published;
    }
    if key.starts_with("series|") {
        if let (Ok(mut measured), Ok(held)) = (
            serde_json::from_str::<Series>(&published.payload),
            serde_json::from_str::<Series>(&incoming.payload),
        ) {
            if retain_series(&mut measured, &held) {
                published.fetched_at = published.fetched_at.min(incoming.fetched_at);
                published.total = measured.points.iter().map(|point| point.merged).sum();
                if let Some(receipt) = &mut measured.receipt {
                    receipt.fetched_at = published.fetched_at;
                    receipt.retained = true;
                    receipt.qualification = Some("Some saved measurements are shown because the retry did not measure everything.".into());
                }
                if let Ok(payload) = serde_json::to_string(&measured) {
                    published.payload = payload;
                }
            }
        }
        published
    } else {
        reconcile_current(key, published, incoming)
    }
}

/// Latest-attempt metadata describes this caller, independently of which
/// compatible measurements win publication. It must not change completeness,
/// acquisition age, or owner identity chosen by arbitration.
pub fn reconcile_publication(
    key: &str,
    incoming: crate::store::stats::Cached,
    current: crate::store::stats::Cached,
    fresh: Option<crate::store::stats::Cached>,
) -> crate::store::stats::Cached {
    if !key.starts_with("series|") && !key.starts_with("count|") {
        return incoming;
    }
    let attempt = serde_json::from_str::<serde_json::Value>(&incoming.payload).ok();
    let mut published = select_measurements(key, incoming, current, fresh);
    if let (Some(attempt), Ok(mut payload)) = (
        attempt,
        serde_json::from_str::<serde_json::Value>(&published.payload),
    ) {
        for field in ["spend", "unmeasured"] {
            if let Some(value) = attempt.get(field) {
                payload[field] = value.clone();
            }
        }
        if let Some(qualification) = attempt.pointer("/receipt/qualification") {
            if let Some(receipt) = payload.get_mut("receipt").and_then(|r| r.as_object_mut()) {
                receipt.insert("qualification".into(), qualification.clone());
            }
        }
        if let Ok(payload) = serde_json::to_string(&payload) {
            published.payload = payload;
        }
    }
    published
}

pub fn fresh_candidate<T: serde::Serialize>(
    value: &T,
    total: u64,
    complete: bool,
    owner: StatsOwner,
    now: DateTime<Utc>,
) -> Option<crate::store::stats::Cached> {
    let mut payload = serde_json::to_value(value).ok()?;
    payload["receipt"] = serde_json::to_value(StatsReceipt::measured(now, owner)).ok()?;
    Some(crate::store::stats::Cached {
        total,
        complete,
        payload: payload.to_string(),
        fetched_at: now,
    })
}
