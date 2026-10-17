//! The system-health series: writing samples, trimming to 24 hours, and
//! handing back a bounded history.
//!
//! # Why the history is downsampled here
//!
//! At one sample a minute the table holds ~1440 rows. Sending all of
//! them to draw a chart a few hundred pixels wide is waste on the
//! desktop and a real cost on the phone, which reads this over the LAN:
//! `size_worktrees` taught that lesson when a slow command over the
//! remote surface timed out at two minutes (#661). So the SQL picks a
//! bounded number of buckets and the caller cannot ask for more.

use super::schema::StoreError;
use crate::health::Sample;
use rusqlite::{params, Connection};

/// How long a sample is kept.
pub const RETENTION_HOURS: i64 = 24;

/// The most points `history` will ever return.
///
/// A chart is a few hundred pixels wide; more points than that draw on
/// top of each other. 120 is one every twelve minutes across a day,
/// which is enough to see a shape without being enough to be slow.
pub const MAX_POINTS: usize = 120;

/// Write one sample and drop anything older than [`RETENTION_HOURS`].
///
/// The trim runs on every write rather than on a timer: a timer is a
/// second thing to schedule and to get wrong, and deleting a handful of
/// rows beside an insert costs nothing. Without it the table is a slow
/// leak that only shows up weeks later.
pub fn record(conn: &Connection, s: &Sample) -> Result<(), StoreError> {
    let detail = serde_json::to_string(s)?;
    conn.execute(
        "INSERT OR REPLACE INTO health_samples
           (sampled_at, load_1, load_5, load_15, cpu_percent,
            mem_total, mem_used, mem_available,
            battery_percent, on_ac, battery_capacity_percent,
            thermal, uptime_secs, detail)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            s.sampled_at,
            s.load.map(|l| l[0]),
            s.load.map(|l| l[1]),
            s.load.map(|l| l[2]),
            s.cpu_percent,
            s.memory.total as i64,
            s.memory.used as i64,
            s.memory.available as i64,
            s.battery.as_ref().map(|b| b.percent),
            s.battery.as_ref().map(|b| i64::from(b.on_ac)),
            // CHARGE is `battery_percent` above; this is CAPACITY, a
            // different number entirely. Both are stored because a
            // query that confused them would be silently wrong -- see
            // `health::Battery`.
            s.battery.as_ref().and_then(|b| b.capacity_percent),
            s.thermal,
            s.uptime_secs as i64,
            detail,
        ],
    )?;
    // Compared as strings, which is sound because every timestamp here
    // is RFC 3339 in UTC: that format sorts lexicographically in time
    // order, and mixing offsets would break far more than this query.
    let cutoff = (chrono::Utc::now() - chrono::Duration::hours(RETENTION_HOURS)).to_rfc3339();
    conn.execute(
        "DELETE FROM health_samples WHERE sampled_at < ?1",
        params![cutoff],
    )?;
    Ok(())
}

/// The series, at most [`MAX_POINTS`] rows, oldest first.
///
/// Bucketed by row number rather than by time, so a period when the app
/// was closed does not become a run of empty buckets the caller has to
/// filter. The GAP is still visible -- consecutive rows are simply far
/// apart in time -- which is what lets the chart draw it as a gap rather
/// than interpolating a flat line through hours nobody measured.
pub fn history(conn: &Connection) -> Result<Vec<Sample>, StoreError> {
    // Divide ordinal positions into MAX_POINTS - 1 intervals and keep the
    // first row plus every interval transition. This includes both endpoints
    // and never exceeds the cap, even when the row count is not divisible.
    // COUNT and row numbering share one SQLite snapshot.
    let mut stmt = conn.prepare(
        "SELECT detail FROM (
             SELECT detail, ROW_NUMBER() OVER (ORDER BY sampled_at) - 1 AS n,
                    COUNT(*) OVER () AS total
             FROM health_samples
         ) WHERE total <= ?1 OR n = 0
             OR (n * (?1 - 1)) / (total - 1) > ((n - 1) * (?1 - 1)) / (total - 1)
         ORDER BY n",
    )?;
    let rows = stmt.query_map(params![MAX_POINTS as i64], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        // A row that does not parse is skipped rather than failing the
        // whole history: one bad sample from an older shape must not
        // cost the user every chart.
        if let Ok(s) = serde_json::from_str::<Sample>(&row?) {
            out.push(s);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::{Memory, Sample};

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::store::migrate(&c).unwrap();
        c
    }

    fn sample(at: &str) -> Sample {
        Sample {
            sampled_at: at.to_string(),
            load: Some([1.0, 2.0, 3.0]),
            cpu_percent: Some(12.5),
            cpu_per_core: vec![10.0, 15.0],
            memory: Memory {
                total: 100,
                used: 40,
                available: 60,
                swap_total: 0,
                swap_used: 0,
            },
            gpus: vec![],
            disks: vec![],
            battery: None,
            thermal: Some("nominal".into()),
            networks: vec![],
            uptime_secs: 42,
        }
    }

    #[test]
    fn a_sample_round_trips() {
        let c = conn();
        // Dated NOW, not a fixed date: `record` trims anything outside
        // the retention window, so a hardcoded timestamp deletes itself
        // the moment it falls out of the last 24 hours.
        record(&c, &sample(&chrono::Utc::now().to_rfc3339())).unwrap();
        let got = history(&c).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].cpu_percent, Some(12.5));
        assert_eq!(got[0].thermal.as_deref(), Some("nominal"));
    }

    /// The GPU survives the round trip, absences included.
    ///
    /// GPUs live in the `detail` JSON rather than in a column, so this
    /// is the only thing standing between a stored sample and a panel
    /// that silently loses its readings. The `None`s matter as much as
    /// the values: a field that came back as 0 instead of absent would
    /// turn "not measured" into a measurement on every historical
    /// sample at once.
    #[test]
    fn a_gpu_round_trips_with_its_absent_fields_still_absent() {
        let c = conn();
        let now = chrono::Utc::now().to_rfc3339();
        let mut s = sample(&now);
        s.gpus = vec![crate::health::Gpu {
            name: "Apple M2 Max".into(),
            utilization_percent: Some(7.0),
            memory_used: Some(1_202_913_280),
            memory_total: None,
            unified_memory: true,
            // The pipeline stages (#717): one present, one absent, so
            // the round trip is asserted for both cases of the pair the
            // GPU detail page reads.
            renderer_percent: Some(91.0),
            tiler_percent: None,
        }];
        record(&c, &s).unwrap();
        let got = history(&c).unwrap();
        assert_eq!(got[0].gpus.len(), 1);
        assert_eq!(got[0].gpus[0].name, "Apple M2 Max");
        assert_eq!(got[0].gpus[0].utilization_percent, Some(7.0));
        assert_eq!(got[0].gpus[0].memory_total, None, "absent stays absent");
        assert!(got[0].gpus[0].unified_memory);
        assert_eq!(got[0].gpus[0].renderer_percent, Some(91.0));
        assert_eq!(got[0].gpus[0].tiler_percent, None, "absent stays absent");
    }

    /// Charge and CAPACITY are different numbers, and both survive.
    ///
    /// The failure this guards is not a lost field but a SWAPPED one:
    /// `battery_percent` and `battery_capacity_percent` are adjacent
    /// columns holding two percentages that mean opposite things, and a
    /// transposed pair would render a three-year-old battery at 84%
    /// charge as one at 84% capacity, or worse the reverse. Asserted
    /// with deliberately different values so a swap cannot pass.
    //
    // `assert_eq!` on an f64 expands to `==` and so trips float_cmp
    // (#892). Exact equality is the assertion: this test is about a
    // SQLite round trip, where the value read back must be the value
    // written bit for bit. A margin would let a lossy column type pass,
    // which is the failure being checked for.
    #[allow(clippy::float_cmp)]
    #[test]
    fn charge_and_capacity_round_trip_without_being_confused() {
        let c = conn();
        let now = chrono::Utc::now().to_rfc3339();
        let mut s = sample(&now);
        s.battery = Some(crate::health::Battery {
            percent: 62.0,
            on_ac: true,
            capacity_percent: Some(84.0),
            cycle_count: Some(413),
            // A DISCHARGE, so the sign is part of what is being
            // asserted. The power flow lives only in the `detail` JSON
            // -- it has no column of its own -- and a serialisation
            // that dropped or unsigned it would turn every historical
            // discharge into a charge on the #773 chart.
            power: Some(crate::health::PowerFlow {
                watts: -12.8,
                milliamps: -1008,
                millivolts: 12654,
            }),
        });
        record(&c, &s).unwrap();

        let got = history(&c).unwrap();
        let b = got[0].battery.as_ref().expect("the battery survives");
        assert_eq!(b.percent, 62.0, "charge");
        assert_eq!(b.capacity_percent, Some(84.0), "capacity, not charge");
        assert_eq!(b.cycle_count, Some(413));
        let p = b.power.expect("the power flow survives the round trip");
        assert_eq!(p.watts, -12.8, "still a discharge, not a charge");
        assert_eq!(p.milliamps, -1008);
        assert_eq!(p.millivolts, 12654);

        // And the column carries the CAPACITY, not the charge -- the
        // detail JSON round-tripping correctly would hide a swap here.
        let col: Option<f64> = c
            .query_row(
                "SELECT battery_capacity_percent FROM health_samples",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col, Some(84.0));
    }

    /// A battery whose capacity the platform will not report stays
    /// absent rather than becoming a zero.
    ///
    /// A battery at 0% of its design capacity is a dead cell. Reporting
    /// "we did not look" as that number would tell a Linux user -- where
    /// no capacity is read at all -- that their battery has failed.
    #[test]
    fn an_unreported_capacity_stays_absent() {
        let c = conn();
        let mut s = sample(&chrono::Utc::now().to_rfc3339());
        s.battery = Some(crate::health::Battery {
            percent: 91.0,
            on_ac: false,
            capacity_percent: None,
            cycle_count: None,
            power: None,
        });
        record(&c, &s).unwrap();
        let got = history(&c).unwrap();
        let b = got[0].battery.as_ref().unwrap();
        assert_eq!(b.capacity_percent, None, "absent is never zero");
        assert_eq!(b.cycle_count, None);
        // Zero watts is a REAL reading -- a full battery on mains draws
        // nothing -- so an unread power flow must come back absent
        // rather than as a plausible-looking 0 W.
        assert!(b.power.is_none(), "absent is never zero watts");
    }

    /// Per-interface counters survive, which is the raw material #719
    /// differences into a rate.
    ///
    /// They live in `detail`, and had this not round-tripped there
    /// would be no network history to compute from at all -- the
    /// feature is entirely a consumer of what this test asserts.
    #[test]
    fn per_interface_counters_round_trip() {
        let c = conn();
        let mut s = sample(&chrono::Utc::now().to_rfc3339());
        s.networks = vec![
            crate::health::Interface {
                name: "en0".into(),
                rx_bytes: 4_000_000_000,
                tx_bytes: 1_000_000_000,
            },
            crate::health::Interface {
                name: "lo0".into(),
                rx_bytes: 12,
                tx_bytes: 12,
            },
        ];
        record(&c, &s).unwrap();
        let got = history(&c).unwrap();
        assert_eq!(got[0].networks.len(), 2);
        assert_eq!(got[0].networks[0].name, "en0");
        // Above 2^32: a counter narrowed to u32 somewhere in the round
        // trip would wrap and read as a reset, which the UI draws as a
        // gap -- so the wrong type here would silently blank the chart.
        assert_eq!(got[0].networks[0].rx_bytes, 4_000_000_000);
    }

    /// Anything older than the window is gone, so the table cannot grow
    /// without bound.
    #[test]
    fn samples_older_than_a_day_are_dropped() {
        let c = conn();
        let old = (chrono::Utc::now() - chrono::Duration::hours(RETENTION_HOURS + 1)).to_rfc3339();
        let now = chrono::Utc::now().to_rfc3339();
        record(&c, &sample(&old)).unwrap();
        record(&c, &sample(&now)).unwrap();
        let got = history(&c).unwrap();
        assert_eq!(got.len(), 1, "the day-old sample must be gone");
        assert_eq!(got[0].sampled_at, now);
    }

    /// The history is bounded however long the app has been running.
    ///
    /// This is the property the phone depends on: an unbounded series
    /// over the LAN is the mistake that made `size_worktrees` time out
    /// (#661).
    #[test]
    fn the_history_is_bounded_and_stays_in_order() {
        let c = conn();
        let base = chrono::Utc::now() - chrono::Duration::hours(20);
        for i in 0..1440 {
            let at = (base + chrono::Duration::minutes(i)).to_rfc3339();
            record(&c, &sample(&at)).unwrap();
        }
        let got = history(&c).unwrap();
        assert!(
            got.len() <= MAX_POINTS,
            "{} points is more than the cap",
            got.len()
        );
        assert!(got.len() > 1, "a full day should not collapse to one point");
        // Oldest first, and strictly increasing.
        for w in got.windows(2) {
            assert!(w[0].sampled_at < w[1].sampled_at, "out of order");
        }
    }

    #[test]
    fn sampling_caps_non_divisible_sizes_and_preserves_endpoints_and_absence() {
        let c = conn();
        for total in [0, 1, 119, 120, 121, 239, 240, 1439, 1440, 5000] {
            c.execute("DELETE FROM health_samples", []).unwrap();
            let base = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap();
            let mut expected = Vec::new();
            for i in 0..total {
                // Actual gaps survive sampling; no interpolated timestamps.
                let at = (base + chrono::Duration::seconds(i * 17)).to_rfc3339();
                let mut value = sample(&at);
                value.cpu_percent = None;
                c.execute(
                    "INSERT INTO health_samples (sampled_at, detail) VALUES (?1, ?2)",
                    params![at, serde_json::to_string(&value).unwrap()],
                )
                .unwrap();
                expected.push(at);
            }
            let got = history(&c).unwrap();
            assert_eq!(got.len(), (total as usize).min(MAX_POINTS), "size {total}");
            let times: Vec<_> = got.iter().map(|s| s.sampled_at.clone()).collect();
            if total <= MAX_POINTS as i64 {
                assert_eq!(times, expected);
            } else {
                assert_eq!(times.first(), expected.first());
                assert_eq!(times.last(), expected.last());
                assert!(times.windows(2).all(|w| w[0] < w[1]));
                assert!(times.iter().all(|t| expected.contains(t)));
            }
            assert!(got.iter().all(|s| s.cpu_percent.is_none()));
        }
    }

    /// Fewer samples than buckets: every row comes back, and nothing
    /// divides by zero.
    #[test]
    fn a_short_history_is_returned_whole() {
        let c = conn();
        let base = chrono::Utc::now() - chrono::Duration::minutes(5);
        for i in 0..5 {
            record(
                &c,
                &sample(&(base + chrono::Duration::minutes(i)).to_rfc3339()),
            )
            .unwrap();
        }
        assert_eq!(history(&c).unwrap().len(), 5);
    }

    /// An empty table is an empty history, not an error.
    #[test]
    fn no_samples_is_not_a_failure() {
        assert!(history(&conn()).unwrap().is_empty());
    }
}
