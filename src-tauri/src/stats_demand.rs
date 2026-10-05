//! Bounded process-local read interest. Durable registrations are history,
//! never permission for perpetual provider spend. Lock order is registry then
//! capability read; retirement takes capability write and never registry.
use crate::remote::context::DispatchContext;
use crate::store::{
    pr_backfill_scope::BackfillScope,
    stats_owner::{self, StatsOwner},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

pub const TTL: Duration = Duration::from_secs(180);
const MAX_TOTAL: usize = 512;
const MAX_PRINCIPAL: usize = 64;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Request {
    #[serde(rename_all = "camelCase")]
    Acquire {
        scope_kind: String,
        scope_value: Option<String>,
        measure: String,
        days: i64,
    },
    Renew {
        handle: String,
        sequence: u64,
    },
    Release {
        handle: String,
        sequence: u64,
    },
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub handle: String,
    pub owner: StatsOwner,
}

struct Lease {
    handle: String,
    owner: StatsOwner,
    context: DispatchContext,
    scope: BackfillScope,
    until: Instant,
    sequence: u64,
    worked: u64,
    legacy: bool,
}
#[derive(Default)]
struct Inner {
    leases: Vec<Lease>,
    turn: u64,
}
#[derive(Default)]
pub struct Registry(Mutex<Inner>);

pub struct Pick {
    pub scope: BackfillScope,
    pub from: String,
    pub to: String,
    pub uncovered: Vec<String>,
    pub pages: Vec<crate::store::pr_backfill_page::Page>,
    pub partial: bool,
}

impl Registry {
    pub fn has_live_interest(&self, now: Instant) -> bool {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner
            .leases
            .retain(|lease| now < lease.until && lease.context.guard().is_ok());
        !inner.leases.is_empty()
    }

    pub fn acquire(
        &self,
        tx: &rusqlite::Transaction<'_>,
        owner: &StatsOwner,
        context: &DispatchContext,
        scope: BackfillScope,
        now: Instant,
        legacy: bool,
    ) -> Result<Receipt, String> {
        stats_owner::require_current(tx, owner).map_err(|e| e.to_string())?;
        if scope.scope_key.len() > 1024
            || scope.scope_value.len() > 256
            || !(1..=90).contains(&scope.horizon_days)
            || !crate::github::stats::backfill::walkable(&scope.measure)
            || crate::github::stats::backfill::query_for(
                &scope.scope_kind,
                &scope.scope_value,
                &scope.measure,
            )
            .is_none()
        {
            return Err("invalid stats demand".into());
        }
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        Self::gc(&mut inner, owner, now);
        let _guard = context.guard()?;
        if legacy {
            if let Some(lease) = inner.leases.iter_mut().find(|l| {
                l.legacy
                    && l.context.principal() == context.principal()
                    && l.scope.scope_key == scope.scope_key
            }) {
                lease.scope = scope;
                lease.until = now + TTL;
                return Ok(Receipt {
                    handle: lease.handle.clone(),
                    owner: owner.clone(),
                });
            }
            // A modern observer owns its own lifecycle; legacy board refreshes
            // must not retain a wider horizon after that observer releases.
            if let Some(lease) = inner.leases.iter().find(|l| {
                !l.legacy
                    && l.context.principal() == context.principal()
                    && l.scope.scope_key == scope.scope_key
            }) {
                return Ok(Receipt {
                    handle: lease.handle.clone(),
                    owner: owner.clone(),
                });
            }
        } else {
            inner.leases.retain(|l| {
                !(l.legacy
                    && l.context.principal() == context.principal()
                    && l.scope.scope_key == scope.scope_key)
            });
        }
        if inner.leases.len() >= MAX_TOTAL
            || inner
                .leases
                .iter()
                .filter(|l| l.context.principal() == context.principal())
                .count()
                >= MAX_PRINCIPAL
        {
            return Err("too many active stats observers".into());
        }
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let handle = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        inner.leases.push(Lease {
            handle: handle.clone(),
            owner: owner.clone(),
            context: context.clone(),
            scope,
            until: now + TTL,
            sequence: 0,
            worked: 0,
            legacy,
        });
        Ok(Receipt {
            handle,
            owner: owner.clone(),
        })
    }
    pub fn update(
        &self,
        tx: &rusqlite::Transaction<'_>,
        context: &DispatchContext,
        handle: &str,
        sequence: u64,
        release: bool,
        now: Instant,
    ) -> Result<Receipt, String> {
        if handle.len() != 64 || sequence == 0 {
            return Err("invalid stats lease operation".into());
        }
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = context.guard()?;
        let index = inner
            .leases
            .iter()
            .position(|l| l.handle == handle && l.context.principal() == context.principal())
            .ok_or("stats lease expired or released")?;
        let lease = &mut inner.leases[index];
        stats_owner::require_current(tx, &lease.owner).map_err(|e| e.to_string())?;
        if now >= lease.until || sequence <= lease.sequence {
            return Err("stats lease expired or operation superseded".into());
        }
        let receipt = Receipt {
            handle: lease.handle.clone(),
            owner: lease.owner.clone(),
        };
        if release {
            inner.leases.remove(index);
        } else {
            lease.sequence = sequence;
            lease.until = now + TTL;
        }
        Ok(receipt)
    }
    fn gc(inner: &mut Inner, owner: &StatsOwner, now: Instant) {
        inner
            .leases
            .retain(|l| l.owner == *owner && now < l.until && l.context.guard().is_ok());
    }

    pub fn pick(
        &self,
        tx: &rusqlite::Transaction<'_>,
        owner: &StatsOwner,
        now: Instant,
        wall: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<Pick>, String> {
        stats_owner::require_current(tx, owner).map_err(|e| e.to_string())?;
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        Self::gc(&mut inner, owner, now);
        // Collapse observers to each scope's currently widest live request.
        let mut candidates = HashMap::<String, (usize, u64)>::new();
        for (i, lease) in inner.leases.iter().enumerate() {
            if lease.context.guard().is_err() {
                continue;
            }
            candidates
                .entry(lease.scope.scope_key.clone())
                .and_modify(|(chosen, worked)| {
                    *worked = (*worked).max(lease.worked);
                    if lease.scope.horizon_days > inner.leases[*chosen].scope.horizon_days {
                        *chosen = i;
                    }
                })
                .or_insert((i, lease.worked));
        }
        let mut candidates: Vec<_> = candidates.into_values().collect();
        candidates.sort_by_key(|(i, worked)| (*worked, *i));
        for (index, _) in candidates {
            let lease = &inner.leases[index];
            let context = lease.context.clone();
            let Ok(_guard) = context.guard() else {
                continue;
            };
            let scope = lease.scope.clone();
            let Some((from, to)) =
                crate::github::stats::backfill::horizon_window(wall, scope.horizon_days)
            else {
                continue;
            };
            let uncovered =
                crate::store::pr_slice::uncovered_days(tx, &scope.scope_key, &from, &to)
                    .map_err(|e| e.to_string())?;
            if uncovered.is_empty() {
                continue;
            }
            let pages = crate::store::pr_backfill_page::select_in(
                tx,
                owner,
                &scope.scope_key,
                &uncovered,
                crate::github::stats::backfill::GROUP_SLICES,
            )?;
            let coverage = crate::store::pr_slice::coverage(tx, &scope.scope_key, &from, &to)
                .map_err(|e| e.to_string())?;
            inner.turn = inner
                .turn
                .checked_add(1)
                .ok_or("stats demand sequence exhausted")?;
            let turn = inner.turn;
            for lease in &mut inner.leases {
                if lease.scope.scope_key == scope.scope_key {
                    lease.worked = turn;
                }
            }
            // Fairness is reserved before the provider await, including error
            // or cancellation. A retired caller may finish this bounded turn.
            return Ok(Some(Pick {
                scope,
                from,
                to,
                uncovered,
                pages,
                partial: coverage.partial,
            }));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope(name: &str, days: u32) -> BackfillScope {
        BackfillScope {
            scope_key: format!("merged|*|org:{name}"),
            scope_kind: "org".into(),
            scope_value: name.into(),
            measure: "merged".into(),
            horizon_days: days,
        }
    }
    #[test]
    fn fifty_live_complete_scopes_are_skipped_within_one_local_selection() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let owner = stats_owner::capture_verified(&conn, "fixture").unwrap();
        let registry = Registry::default();
        let now = Instant::now();
        let wall = chrono::Utc::now();
        let context = DispatchContext::desktop();
        let (from, to) = crate::github::stats::backfill::horizon_window(wall, 1).unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        for i in 0..50 {
            let scope = scope(&format!("complete-{i}"), 1);
            registry
                .acquire(&tx, &owner, &context, scope.clone(), now, false)
                .unwrap();
            crate::store::pr_slice::put(
                &tx,
                &scope.scope_key,
                &crate::store::pr_slice::SliceRow {
                    from: from.clone(),
                    to: to.clone(),
                    state: crate::store::pr_slice::SliceState::Complete,
                    issue_count: 0,
                    retrieved: 0,
                    refused_fields: 0,
                },
                wall,
            )
            .unwrap();
        }
        registry
            .acquire(&tx, &owner, &context, scope("active", 1), now, false)
            .unwrap();
        assert_eq!(
            registry
                .pick(&tx, &owner, now, wall)
                .unwrap()
                .unwrap()
                .scope
                .scope_value,
            "active"
        );
        // Tomorrow can create work only while interest remains live.
        assert!(registry
            .pick(&tx, &owner, now + TTL, wall + chrono::Duration::days(1))
            .unwrap()
            .is_none());
        assert_eq!(crate::store::pr_slice::total_rows(&tx).unwrap(), 50);
    }

    #[test]
    fn leases_expire_shrink_sequence_and_fairly_reserve_before_work() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let owner = stats_owner::capture_verified(&conn, "fixture").unwrap();
        let registry = Registry::default();
        let context = DispatchContext::desktop();
        let now = Instant::now();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let wide = registry
            .acquire(&tx, &owner, &context, scope("one", 90), now, false)
            .unwrap();
        let narrow = registry
            .acquire(&tx, &owner, &context, scope("one", 7), now, false)
            .unwrap();
        registry
            .acquire(&tx, &owner, &context, scope("two", 30), now, false)
            .unwrap();
        assert_eq!(
            registry
                .pick(&tx, &owner, now, chrono::Utc::now())
                .unwrap()
                .unwrap()
                .scope
                .horizon_days,
            90
        );
        // Drop that work (failure/cancellation). The reservation still yields.
        assert_eq!(
            registry
                .pick(&tx, &owner, now, chrono::Utc::now())
                .unwrap()
                .unwrap()
                .scope
                .scope_value,
            "two"
        );
        registry
            .update(&tx, &context, &wide.handle, 2, true, now)
            .unwrap();
        assert!(registry
            .update(&tx, &context, &wide.handle, 3, false, now)
            .is_err());
        assert_eq!(
            registry
                .pick(&tx, &owner, now, chrono::Utc::now())
                .unwrap()
                .unwrap()
                .scope
                .horizon_days,
            7
        );
        registry
            .update(
                &tx,
                &context,
                &narrow.handle,
                2,
                false,
                now + Duration::from_secs(60),
            )
            .unwrap();
        assert!(registry
            .update(&tx, &context, &narrow.handle, 1, true, now)
            .is_err());
        assert!(registry
            .update(
                &tx,
                &context,
                &narrow.handle,
                2,
                false,
                now + Duration::from_secs(120)
            )
            .is_err());
        assert!(registry
            .update(
                &tx,
                &context,
                &narrow.handle,
                3,
                false,
                now + Duration::from_secs(240)
            )
            .is_err());
        assert!(registry
            .pick(
                &tx,
                &owner,
                now + Duration::from_secs(240),
                chrono::Utc::now()
            )
            .unwrap()
            .is_none());
    }
    #[test]
    fn owner_aba_and_capacity_cannot_resurrect_old_demand() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let owner = stats_owner::capture_verified(&conn, "fixture-a").unwrap();
        let registry = Registry::default();
        let context = DispatchContext::desktop();
        let now = Instant::now();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let old = registry
            .acquire(&tx, &owner, &context, scope("one", 1), now, false)
            .unwrap();
        for i in 1..MAX_PRINCIPAL {
            registry
                .acquire(
                    &tx,
                    &owner,
                    &context,
                    scope(&format!("s{i}"), 1),
                    now,
                    false,
                )
                .unwrap();
        }
        assert!(registry
            .acquire(&tx, &owner, &context, scope("overflow", 1), now, false)
            .is_err());
        tx.commit().unwrap();
        stats_owner::capture_verified(&conn, "fixture-b").unwrap();
        let current = stats_owner::capture_verified(&conn, "fixture-a").unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        assert!(registry
            .update(&tx, &context, &old.handle, 1, false, now)
            .is_err());
        assert!(registry
            .acquire(&tx, &owner, &context, scope("old", 1), now, false)
            .is_err());
        assert!(registry
            .pick(&tx, &current, now, chrono::Utc::now())
            .unwrap()
            .is_none());
        registry
            .acquire(&tx, &current, &context, scope("fresh", 1), now, false)
            .unwrap();
        assert!(Registry::default()
            .pick(&tx, &current, now, chrono::Utc::now())
            .unwrap()
            .is_none());
    }
}
