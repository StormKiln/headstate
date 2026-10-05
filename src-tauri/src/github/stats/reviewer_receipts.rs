//! Client-owned reviewer reuse. The registry owns weak flights; the last
//! caller leaving drops the shared future and its bounded loader/JoinSets.
use super::{receipt::StatsReceipt, Reviewers, Slice, StatsQuery};
use crate::{github::client::GitHubClient, store::stats_owner::StatsOwner};
use futures_util::{
    future::{BoxFuture, Shared},
    FutureExt,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, Weak},
};
use tokio::time::{Duration, Instant};

#[derive(Clone)]
pub(crate) struct Loaded {
    pub answer: Reviewers,
    pub fresh: Option<Reviewers>,
}
type Answer = Result<Loaded, String>;
struct Flight {
    budget: super::Budget,
    future: Shared<BoxFuture<'static, Answer>>,
}
#[derive(Default)]
struct State {
    receipts: VecDeque<(String, Reviewers, Instant)>,
    flights: HashMap<String, Weak<Flight>>,
}
#[derive(Default)]
pub(crate) struct Reads(Mutex<State>);

impl Reads {
    #[allow(clippy::too_many_arguments)]
    pub async fn get(
        self: &Arc<Self>,
        client: &GitHubClient,
        db: std::path::PathBuf,
        owner: StatsOwner,
        q: StatsQuery,
        window: Slice,
        logins: Vec<String>,
        refresh: bool,
    ) -> Answer {
        let ceiling = Instant::now() + Duration::from_secs(60);
        let caller_deadline = client.stats_deadline().unwrap_or(ceiling);
        let key = serde_json::to_string(&(
            &db,
            &owner,
            q.cache_key(owner.viewer()),
            &window.from,
            &window.to,
            &logins,
        ))
        .map_err(|e| e.to_string())?;
        let (flight, joined, fallback) = {
            let mut state = self.0.lock().map_err(|e| e.to_string())?;
            state.flights.retain(|_, flight| flight.strong_count() > 0);
            let old = state
                .receipts
                .iter()
                .position(|(k, _, _)| k == &key)
                .and_then(|i| state.receipts.remove(i));
            if let Some((_, value, measured)) = &old {
                state
                    .receipts
                    .push_back((key.clone(), value.clone(), *measured));
                if !refresh
                    && value.is_complete()
                    && !value.receipt.as_ref().is_some_and(|r| r.retained)
                    && measured.elapsed() < Duration::from_secs(300)
                {
                    let mut value = value.clone();
                    value.spend = client.request_budget().snapshot();
                    if let Some(receipt) = &mut value.receipt {
                        receipt.reused = true;
                    }
                    return Ok(Loaded {
                        answer: value,
                        fresh: None,
                    });
                }
            }
            let fallback = old.as_ref().map(|(_, value, _)| value.clone());
            if let Some(flight) = state.flights.get(&key).and_then(Weak::upgrade) {
                (flight, true, fallback)
            } else {
                if state.flights.len() >= 16 {
                    if let Some((_, mut value, _)) = old {
                        value.spend = client.request_budget().snapshot();
                        if let Some(receipt) = &mut value.receipt {
                            receipt.reused = true;
                            receipt.retained = true;
                            receipt.qualification=Some("Saved measurements shown; other statistics loads are still running. Retry when they finish.".into());
                        }
                        return Ok(Loaded {
                            answer: value,
                            fresh: None,
                        });
                    }
                    return Err(
                        "Other statistics loads are still running. Retry when they finish.".into(),
                    );
                }
                let cache = self.clone();
                let save_key = key.clone();
                // A producer gets one fixed 60-second ceiling. Each caller
                // independently waits only until its own deadline. Preserve
                // the caller's bulk lane, but not its private shorter deadline.
                let mut context = crate::github::admission::ReadContext::new(
                    client.read_context().class,
                    Duration::from_secs(60),
                );
                context.deadline = ceiling;
                let operation = client.with_scan_context(context);
                let budget = operation.request_budget();
                let producer_budget = budget.clone();
                let future = async move {
                    let budget = producer_budget;
                    let projected = (logins.len() as u64)
                        .div_ceil(super::query::ALIAS_CHUNK as u64) + 1;
                    let measured = chrono::Utc::now();
                    let result = if budget.permits(projected) {
                        super::load_reviewers(&operation, &q, &logins, &window, &budget)
                            .await.map_err(|error| error.to_string())
                    } else {
                        Err("The GitHub budget is too low to retry these reviewer measurements yet.".into())
                    };
                    let fresh = result.as_ref().ok().map(|value| {
                        let mut value = value.clone();
                        value.receipt = Some(StatsReceipt::measured(measured, owner.clone()));
                        value
                    });
                    let mut receipt = StatsReceipt::measured(measured, owner.clone());
                    let mut value = match (result, old) {
                        (Ok(mut value), Some((_, old, _))) => {
                            for row in &old.rows {
                                if value.unmeasured.contains(&row.login) {
                                    value.rows.push(row.clone());
                                    value.unmeasured.retain(|login| login != &row.login);
                                    receipt.retained = true;
                                }
                            }
                            if receipt.retained {
                                receipt.fetched_at = old.receipt.as_ref()
                                    .map(|receipt| receipt.fetched_at).unwrap_or(measured);
                                receipt.qualification = Some(
                                    "Some saved reviewer measurements are shown because the retry did not measure everything.".into()
                                );
                            }
                            value
                        }
                        (Ok(value), None) => value,
                        (Err(error), Some((_, mut old, _))) => {
                            receipt = old.receipt.clone().unwrap_or(receipt);
                            receipt.retained = true;
                            receipt.qualification = Some(format!(
                                "Saved reviewer measurements shown; retry failed: {error}"
                            ));
                            old.spend = budget.snapshot();
                            old
                        }
                        (Err(error), None) => return Err(error),
                    };
                    value.rows.sort_by(|a, b| b.reviews.cmp(&a.reviews)
                        .then_with(|| a.login.cmp(&b.login)));
                    value.receipt = Some(receipt);
                    let mut state = cache.0.lock().map_err(|error| error.to_string())?;
                    state.receipts.retain(|(key, _, _)| key != &save_key);
                    state.receipts.push_back((save_key, value.clone(), Instant::now()));
                    while state.receipts.len() > 64 {
                        state.receipts.pop_front();
                    }
                    Ok(Loaded { answer: value, fresh })
                }.boxed().shared();
                let flight = Arc::new(Flight { future, budget });
                state.flights.insert(key, Arc::downgrade(&flight));
                (flight, false, fallback)
            }
        };
        let mut value = match tokio::time::timeout_at(caller_deadline, flight.future.clone()).await
        {
            Ok(result) => result?,
            Err(_) => {
                let mut answer =
                    fallback
                        .filter(|value| !value.rows.is_empty())
                        .ok_or_else(|| {
                            "Reviewer measurements timed out; retry to continue.".to_string()
                        })?;
                answer.spend = flight.budget.snapshot();
                if let Some(receipt) = &mut answer.receipt {
                    receipt.reused = true;
                    receipt.retained = true;
                    receipt.qualification = Some("Saved reviewer measurements shown; this caller timed out. Retry to continue.".into());
                }
                Loaded {
                    answer,
                    fresh: None,
                }
            }
        };
        if joined {
            value.answer.spend = client.request_budget().snapshot();
            if let Some(receipt) = &mut value.answer.receipt {
                receipt.reused = true;
            }
        }
        Ok(value)
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::Reads;
    pub(crate) fn live(reads: &Reads) -> (usize, usize) {
        let state = reads.0.lock().unwrap();
        (
            state
                .flights
                .values()
                .filter(|f| f.strong_count() > 0)
                .count(),
            state
                .flights
                .values()
                .map(|f| f.strong_count())
                .max()
                .unwrap_or(0),
        )
    }
}
