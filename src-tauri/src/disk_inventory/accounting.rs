use super::model::{Category, Change, Comparison, Observation};
use std::collections::BTreeMap;

pub fn summarize(observation: &mut Observation) {
    let mut groups = BTreeMap::<String, Vec<&super::model::Location>>::new();
    for location in &observation.locations {
        groups
            .entry(location.category.clone())
            .or_default()
            .push(location);
    }
    observation.categories = groups
        .into_iter()
        .map(|(name, locations)| Category {
            name,
            known_allocated: floor_sum(
                locations
                    .iter()
                    .map(|l| l.known_allocated.as_deref().or(l.allocated.as_deref())),
            ),
            allocation_complete: locations
                .iter()
                .all(|l| l.complete && l.allocated.is_some())
                && known_sum(locations.iter().map(|l| l.allocated.as_deref())).is_some(),
            allocated: locations
                .iter()
                .all(|l| l.complete && l.allocated.is_some())
                .then(|| known_sum(locations.iter().map(|l| l.allocated.as_deref())))
                .flatten(),
            logical: known_sum(locations.iter().map(|l| l.logical.as_deref())),
            locations: locations.iter().map(|l| l.id.clone()).collect(),
        })
        .collect();
    observation.known_allocated = floor_sum(
        observation
            .locations
            .iter()
            .map(|c| c.known_allocated.as_deref().or(c.allocated.as_deref())),
    );
    observation.measured = observation.known_allocated.clone();
    observation.allocation_complete = observation.status == "complete"
        && observation
            .locations
            .iter()
            .all(|l| l.complete && l.allocated.is_some())
        && known_sum(observation.locations.iter().map(|l| l.allocated.as_deref())).is_some();
    observation.remainder = match (&observation.volume.used, &observation.measured) {
        (Some(used), Some(measured)) if observation.allocation_complete => used
            .parse::<u64>()
            .ok()
            .zip(measured.parse::<u64>().ok())
            .map(|(u, m)| (i128::from(u) - i128::from(m)).to_string()),
        _ => None,
    };
    let overflow = observation
        .locations
        .iter()
        .filter_map(|l| l.known_allocated.as_deref().or(l.allocated.as_deref()))
        .filter_map(|n| n.parse::<u64>().ok())
        .try_fold(0u64, |sum, n| sum.checked_add(n))
        .is_none();
    observation.accounting_note = if overflow {
        Some("Known allocation exceeds the supported byte-count range; the displayed floor is capped and the total is unknown.".into())
    } else if observation
        .remainder
        .as_deref()
        .is_some_and(|n| n.starts_with('-'))
    {
        Some("Measured allocation exceeds reported usage. Sharing or measurement timing prevents exact reconciliation.".into())
    } else if observation.remainder.is_none() {
        Some("The accounting difference is unavailable because allocation or a matching used-space total could not be measured.".into())
    } else {
        Some("The remainder is an accounting difference, not space known to be disposable.".into())
    };
}
fn floor_sum<'a>(values: impl Iterator<Item = Option<&'a str>>) -> Option<String> {
    let mut total = None;
    for value in values.flatten().filter_map(|s| s.parse::<u64>().ok()) {
        total = Some(total.unwrap_or(0u64).saturating_add(value));
    }
    total.map(|n| n.to_string())
}
fn known_sum<'a>(values: impl Iterator<Item = Option<&'a str>>) -> Option<String> {
    let mut total = 0u64;
    let mut measured = false;
    for value in values.flatten() {
        measured = true;
        total = total.checked_add(value.parse::<u64>().ok()?)?;
    }
    measured.then(|| total.to_string())
}

pub fn compare(current: &Observation, previous: Option<&Observation>) -> Comparison {
    let refusal = |reason: &str| Comparison {
        baseline_at: previous.map(|p| p.finished_at),
        reason: Some(reason.into()),
        changes: vec![],
    };
    let Some(previous) = previous else {
        return refusal("No baseline");
    };
    if !current.volume.identity_stable || !previous.volume.identity_stable {
        return refusal("Stable storage identity is unavailable");
    }
    if current.volume.id != previous.volume.id || current.volume.scope != previous.volume.scope {
        return refusal("Storage scope changed");
    }
    if current.method != previous.method || current.volume.method != previous.volume.method {
        return refusal("Measurement method changed");
    }
    if current.configuration != previous.configuration {
        return refusal("Scan locations or exclusions changed");
    }
    let old: BTreeMap<_, _> = previous.locations.iter().map(|l| (&l.id, l)).collect();
    let new: BTreeMap<_, _> = current.locations.iter().map(|l| (&l.id, l)).collect();
    let changed_paths: Vec<_> = previous
        .locations
        .iter()
        .filter(|l| !new.contains_key(&l.id))
        .chain(
            current
                .locations
                .iter()
                .filter(|l| !old.contains_key(&l.id)),
        )
        .map(|l| std::path::Path::new(&l.path))
        .collect();
    let overlaps = |path: &str| {
        changed_paths.iter().any(|p| {
            *p != std::path::Path::new(path)
                && (p.starts_with(path) || std::path::Path::new(path).starts_with(p))
        })
    };
    let mut changes = vec![];
    for location in &current.locations {
        let prior = old.get(&location.id).copied();
        let mut reason = None;
        let mut kind = if prior.is_some() {
            "persistent"
        } else {
            "added"
        };
        let bytes =
            if !location.identity_stable || !location.complete || location.allocated.is_none() {
                reason = Some("Current allocation or identity is incomplete".into());
                None
            } else if exclusions(current, &location.path) != exclusions(previous, &location.path) {
                reason = Some("Intentional exclusions changed".into());
                None
            } else if overlaps(&location.path) {
                reason = Some("Overlapping location attribution changed".into());
                None
            } else if let Some(prior) = prior {
                if !prior.identity_stable || !prior.complete || prior.allocated.is_none() {
                    reason = Some("Baseline allocation or identity is incomplete".into());
                    None
                } else if prior.path != location.path
                    || prior.category != location.category
                    || prior.owners != location.owners
                {
                    reason = Some("Location moved or attribution changed".into());
                    None
                } else {
                    location
                        .allocated
                        .as_deref()
                        .and_then(|s| s.parse::<u64>().ok())
                        .zip(
                            prior
                                .allocated
                                .as_deref()
                                .and_then(|s| s.parse::<u64>().ok()),
                        )
                        .map(|(a, b)| (i128::from(a) - i128::from(b)).to_string())
                }
            } else {
                // A new identity is not evidence of a zero-byte baseline (it could
                // be newly discovered or a replacement at the same path).
                reason = Some("Newly observed location; no measured baseline".into());
                None
            };
        if prior.is_some() && bytes.is_none() {
            kind = "incomparable";
        }
        changes.push(Change {
            location_id: location.id.clone(),
            path: location.path.clone(),
            bytes,
            kind: kind.into(),
            reason,
        });
    }
    for location in previous
        .locations
        .iter()
        .filter(|l| !new.contains_key(&l.id))
    {
        let absence_verified = current.status == "complete"
            && location.complete
            && location.identity_stable
            && !overlaps(&location.path)
            && !current.locations.iter().any(|l| l.path == location.path)
            && std::fs::symlink_metadata(&location.path)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
        changes.push(Change {
            location_id: location.id.clone(),
            path: location.path.clone(),
            bytes: if absence_verified {
                location
                    .allocated
                    .as_deref()
                    .and_then(|n| n.parse::<u64>().ok())
                    .map(|n| (-i128::from(n)).to_string())
            } else {
                None
            },
            kind: if absence_verified {
                "removed"
            } else {
                "incomparable"
            }
            .into(),
            reason: Some(
                if absence_verified {
                    "Previously measured location is now absent"
                } else {
                    "Location was not observed; removal is unverified"
                }
                .into(),
            ),
        });
    }
    changes.sort_by_key(|c| {
        std::cmp::Reverse(
            c.bytes
                .as_deref()
                .and_then(|n| n.parse::<i128>().ok())
                .unwrap_or_default()
                .abs(),
        )
    });
    Comparison {
        baseline_at: Some(previous.finished_at),
        reason: (current.status != "complete"
            || previous.status != "complete"
            || changes.iter().any(|c| c.bytes.is_none()))
        .then(|| {
            "Only individually comparable locations have deltas; this is not full-volume growth"
                .into()
        }),
        changes,
    }
}

fn exclusions<'a>(observation: &'a Observation, path: &str) -> Vec<(&'a str, &'a str)> {
    let mut exclusions: Vec<_> = observation
        .coverage
        .iter()
        .filter(|c| {
            !c.affects_completeness
                && c.reason.contains("link")
                && std::path::Path::new(&c.path).starts_with(path)
        })
        .map(|c| (c.path.as_str(), c.reason.as_str()))
        .collect();
    exclusions.sort();
    exclusions.dedup();
    exclusions
}
