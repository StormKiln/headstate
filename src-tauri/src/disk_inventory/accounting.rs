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
            allocated: known_sum(locations.iter().map(|l| l.allocated.as_deref())),
            logical: known_sum(locations.iter().map(|l| l.logical.as_deref())),
            locations: locations.iter().map(|l| l.id.clone()).collect(),
        })
        .collect();
    observation.measured = known_sum(observation.locations.iter().map(|c| c.allocated.as_deref()));
    observation.remainder = match (&observation.volume.used, &observation.measured) {
        (Some(used), Some(measured)) => used
            .parse::<u64>()
            .ok()
            .zip(measured.parse::<u64>().ok())
            .map(|(u, m)| (i128::from(u) - i128::from(m)).to_string()),
        _ => None,
    };
    observation.accounting_note = if observation
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
    if !current.volume.identity_stable
        || !previous.volume.identity_stable
        || current
            .locations
            .iter()
            .chain(&previous.locations)
            .any(|l| !l.identity_stable)
    {
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
    if current.status != "complete"
        || previous.status != "complete"
        || current
            .locations
            .iter()
            .chain(&previous.locations)
            .any(|l| !l.complete)
    {
        return refusal("Measurements have incomplete coverage");
    }
    let mut old = BTreeMap::new();
    for location in &previous.locations {
        old.insert(&location.id, location);
    }
    if current.locations.len() != old.len() {
        return refusal("Locations appeared or disappeared");
    }
    let mut changes = Vec::new();
    for location in &current.locations {
        let Some(prior) = old.get(&location.id) else {
            return refusal("Location identity changed");
        };
        if location.path != prior.path {
            return refusal("A location moved");
        }
        if location.category != prior.category || location.owners != prior.owners {
            return refusal("Location attribution changed");
        }
        let Some((now, before)) = location
            .allocated
            .as_deref()
            .and_then(|n| n.parse::<u64>().ok())
            .zip(
                prior
                    .allocated
                    .as_deref()
                    .and_then(|n| n.parse::<u64>().ok()),
            )
        else {
            return refusal("Allocated size is unavailable");
        };
        changes.push(Change {
            location_id: location.id.clone(),
            path: location.path.clone(),
            bytes: (i128::from(now) - i128::from(before)).to_string(),
        });
    }
    changes.sort_by_key(|c| std::cmp::Reverse(c.bytes.parse::<i128>().unwrap_or_default().abs()));
    Comparison {
        baseline_at: Some(previous.finished_at),
        reason: None,
        changes,
    }
}
