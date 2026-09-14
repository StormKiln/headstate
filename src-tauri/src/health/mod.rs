//! What the machine is doing, and what Headstate is costing it.
//!
//! One sample a minute while the app runs, kept for 24 hours
//! (`store::health`). The view reads the current sample live and the
//! series for its charts.
//!
//! # What is NOT here
//!
//! **Temperature in degrees.** On macOS the SMC is only readable with
//! elevated privileges -- `powermetrics --samplers smc` requires sudo,
//! and there is no unprivileged path -- so a degrees reading would work
//! only for a user who ran the app as root. What IS available is the
//! platform's thermal PRESSURE, a coarse label, and that is what
//! [`Sample::thermal`] carries. The UI says so rather than letting a
//! label that looks like a temperature imply one.
//!
//! **The GPU, on most platforms.** macOS answers through IOKit and
//! Linux answers for AMD cards through sysfs. Intel on Linux genuinely
//! cannot be read without `CAP_PERFMON`; Windows and NVIDIA-on-Linux
//! could be, but each needs machinery (a PDH loop, a hand-declared NVML
//! ABI) that `sysinfo` is about to provide for free. [`Sample::gpus`]
//! is empty on all of them and the view draws no panel, which is the
//! same rule as everything else here. [`gpu`] carries the evidence
//! behind each verdict.
//!
//! **Per-process network, on every platform but macOS.** macOS answers
//! through `nettop`, at a cost of ~5 SECONDS a reading -- two orders of
//! magnitude past anything else here, and equal to the whole live poll
//! interval. So it is deliberately NOT part of [`Sample`] and not on
//! this timer at all: it has its own command and runs only while the
//! Network detail page is open. Linux has no unprivileged route and
//! Windows' is real work nobody has done; both report nothing and the
//! view says why. [`netproc`] carries the measurements and the
//! per-platform evidence.
//!
//! # Absent is not zero
//!
//! Every optional field is `None` when the platform does not expose it,
//! never `0.0`. A zero that means "not measured" reads as a real
//! measurement, which is the failure `packages::run::missing_tool`
//! exists to avoid on the other side of the app: "no updates" and "the
//! check did not run" are opposite answers.
//!
//! # The two halves of "what Headstate costs"
//!
//! [`Sample`] is the machine. [`Footprint`] (`footprint`) is this app's
//! own share of it -- our process, the `git`/`gh`/Docker subprocesses we
//! spawn, and the Docker daemon we keep resident. That is the LIVE half
//! only: the disk half of the same panel is `size_worktrees`,
//! `size_artifacts`, `size_venvs` and `docker_disk_usage`, which already
//! exist and are slow enough (~13s, #661) that they must stay behind an
//! explicit "Measure" and out of the sampler.

use serde::{Deserialize, Serialize};

pub mod alerts;
pub mod collect;
pub mod footprint;
pub mod gpu;
pub mod netproc;
/// The CPU runaway rules (#791): the aggregate alert that ships, and
/// the shadow log for the two per-process tiers that do not yet.
pub mod runaway;

pub use footprint::Footprint;
pub use gpu::Gpu;
pub use netproc::NetProcess;

/// One moment, as the UI consumes it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Sample {
    /// RFC 3339, matching every other timestamp this app stores.
    pub sampled_at: String,
    /// 1, 5 and 15 minute load averages. `None` on platforms without
    /// them (Windows has no equivalent).
    pub load: Option<[f64; 3]>,
    /// Whole-machine CPU use, 0-100.
    pub cpu_percent: Option<f64>,
    /// Per-core, 0-100, in the platform's own core order.
    pub cpu_per_core: Vec<f64>,
    pub memory: Memory,
    /// Every GPU the platform will describe, which on several is none.
    ///
    /// Empty is "nothing discoverable", and the view draws no panel for
    /// it rather than a panel of zeroes -- see `health::gpu` for which
    /// platforms can be read unprivileged and which cannot.
    ///
    /// A `Vec` rather than an `Option<Gpu>` because a machine can have
    /// two (an Intel Mac with integrated and discrete graphics), and
    /// collapsing them would have to pick one and hide the other.
    pub gpus: Vec<Gpu>,
    pub disks: Vec<Volume>,
    pub battery: Option<Battery>,
    /// `nominal`, `fair`, `serious`, `critical` -- NOT degrees. See the
    /// module docs.
    pub thermal: Option<String>,
    pub networks: Vec<Interface>,
    /// Seconds since boot.
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Memory {
    pub total: u64,
    pub used: u64,
    /// What the OS believes is reclaimable, which is not `total - used`
    /// on any modern platform: cache counts as used and is available.
    pub available: u64,
    /// Swap, where the platform reports it.
    pub swap_total: u64,
    pub swap_used: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Volume {
    pub mount: String,
    pub total: u64,
    pub available: u64,
    /// True for the volume the app itself lives on, which is the one a
    /// user filling their disk cares about first.
    pub is_root: bool,
}

/// The battery, which carries TWO different percentages.
///
/// `percent` is CHARGE -- how full the cell is right now. It is what
/// every "battery is low" alert is about, and it moves minute to
/// minute.
///
/// `capacity_percent` is HEALTH -- how much the cell can still hold
/// relative to the day it was made. It moves over years, and a battery
/// at 100% charge and 71% capacity is completely normal for a
/// three-year-old laptop.
///
/// They are stored as separate fields, and the UI renders them in
/// separate panels with different words, because conflating them is the
/// single most likely misreading of this struct: "battery health: 84%"
/// beside a charge bar reads as a charge figure, and a user who sees it
/// fall from 100 to 84 concludes their battery is draining when in fact
/// it has aged.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Battery {
    /// CHARGE, 0-100. How full the cell is at this instant.
    pub percent: f64,
    pub on_ac: bool,
    /// CAPACITY relative to design, 0-100 -- the figure usually called
    /// "battery health". `None` on every platform but macOS, and `None`
    /// on macOS when `ioreg` does not publish the pair it is derived
    /// from.
    ///
    /// Never 0 for "not measured": a battery at 0% of its design
    /// capacity is a dead battery, which is the opposite claim from
    /// "we did not look". See the module docs.
    #[serde(default)]
    pub capacity_percent: Option<f64>,
    /// Charge cycles the cell has been through. `None` where the
    /// platform does not publish it.
    ///
    /// Kept beside `capacity_percent` because it is the context that
    /// makes it readable: 84% capacity after 400 cycles is ordinary
    /// ageing, and after 40 it is a fault.
    #[serde(default)]
    pub cycle_count: Option<u32>,
    /// How fast power is moving in or out of the cell RIGHT NOW (#773).
    ///
    /// The third distinct thing this struct carries, and the only one
    /// that is a RATE rather than a level. `percent` says how full,
    /// `capacity_percent` says how full it can get, and this says which
    /// way and how fast it is moving -- which is the question #720's
    /// "discharging while plugged in" alert raises and nothing in the
    /// app could answer.
    ///
    /// `None` where the platform does not publish it. Never a zero
    /// standing in for that: zero watts is a REAL and common reading --
    /// a full battery on mains draws nothing -- so a fabricated zero
    /// here is indistinguishable from a measurement.
    #[serde(default)]
    pub power: Option<PowerFlow>,
}

/// The power moving in or out of the battery at one instant (#773).
///
/// # The sign is the whole point
///
/// `watts` is POSITIVE when power is flowing INTO the cell (charging)
/// and NEGATIVE when it is flowing out (discharging). One signed number
/// rather than a magnitude plus a direction enum, because every
/// consumer -- the chart, the card, the sentence under it -- wants to
/// compare against zero, and a magnitude that has to be re-signed at
/// each site is a sign error waiting to happen at one of them.
///
/// The platforms disagree about how they encode that sign, and neither
/// encoding survives being assumed: macOS prints a two's-complement
/// `i64` as an unsigned decimal, and Linux publishes an unsigned
/// magnitude with the direction in a separate string. Both are
/// normalised to this convention where they are read, so nothing
/// downstream has to know which machine it is describing. See
/// `collect::power_flow` and `collect::linux_power`.
///
/// # Why the current and voltage come too
///
/// The wattage is the answer, but it is a PRODUCT, and a reader who
/// sees an implausible one has no way to tell which half is wrong.
/// Carrying both factors makes the detail page able to show its
/// working, at the cost of two integers per sample.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PowerFlow {
    /// Watts. Positive into the cell, negative out of it.
    pub watts: f64,
    /// Milliamps, on the same sign convention as `watts`.
    pub milliamps: i64,
    /// Millivolts at the terminals. Always positive -- a non-positive
    /// voltage is a platform that did not answer, and produces `None`
    /// for the whole reading rather than a zero-watt product.
    pub millivolts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Interface {
    pub name: String,
    /// Cumulative since boot, not since the last sample: a consumer that
    /// wants a rate subtracts two samples, and one that wants a total
    /// does not have to add them all up.
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// One health condition, flattened for a caller that cannot run the
/// rules itself (#789).
///
/// # Why this type exists
///
/// The phone needs to notify about the DESKTOP's health, and the
/// alternative was for the phone to fetch `system_health_history` and
/// evaluate the rules over it. That would be a second implementation of
/// every threshold in [`alerts`] and [`runaway`], in a separate crate
/// with a separate lockfile -- and the two copies would drift silently,
/// which is the worst possible failure for a rule whose whole job is
/// deciding when to interrupt someone.
///
/// So the rules run in exactly one place, the desktop, and this is what
/// crosses the wire: the verdict, not the data it was drawn from.
///
/// # Why the wording comes too
///
/// `key` alone would be enough to notify, if the caller held a copy of
/// every `title()` and `body()`. It would also be a second place for the
/// wording to live -- and a phone showing different words for the same
/// condition is the same drift in a more visible form. The desktop owns
/// the sentence; the phone adds only the machine's name in front of it,
/// which is the one thing the desktop cannot know (see
/// `src-mobile/src/notify.rs` on why a health notification on a phone
/// must name whose machine it is about).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AlertReport {
    /// The stable condition identity: `alerts::Alert::key` or
    /// `runaway::Alert::key`.
    ///
    /// Keyed on the condition and never on the numbers, which is what
    /// lets a caller deduplicate a standing condition without
    /// re-notifying as the figures wander. Both modules document that
    /// choice on their own `key`.
    pub key: String,
    pub title: String,
    pub body: String,
    /// The process this condition is about, where it is about exactly one
    /// (#943).
    ///
    /// # Why a structured field on a type that is otherwise prose
    ///
    /// Because the wording cannot carry it usefully. Every condition was
    /// flattened into three strings, so the CPU watch notice's own closing
    /// sentence -- "Worth a look" -- reached the UI with nothing the UI
    /// could attach an action to: no targeted affordance was possible for
    /// any of the six conditions, not just that one. A pid interpolated
    /// into `body` would be text a reader retypes, which is the defect
    /// rather than the fix.
    ///
    /// It does not weaken the "verdicts, not data" rule this type exists
    /// for. The rules still run only on the desktop and the phone still
    /// holds no threshold; this is one identifier the desktop already
    /// computed, travelling beside the sentence that is about it.
    ///
    /// `None` for every condition that is not about a single process: the
    /// five `alerts::Alert` variants, the machine-wide oversubscription
    /// notice, and a watch notice that collapsed several processes of one
    /// name. See `runaway::Notice::pid` on why a collapsed row must not
    /// carry one of its members' pids.
    ///
    /// Serde gives `Option` no `skip_serializing_if`, so the key is always
    /// present on the wire as `null` -- which is what the TypeScript
    /// `number | null` says, and what stops "absent" reading as 0.
    #[serde(default)]
    pub pid: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire shape, pinned, because nothing generates the TypeScript
    /// (#943).
    ///
    /// `AlertReport` is mirrored by hand at `src/types/pr.ts` as
    /// `{ key, title, body, pid: number | null }`, and the phone mirrors it
    /// again as `notify::HealthAlert`. Three hand-written copies of one
    /// struct is exactly the drift `install::tests::the_wire_shape_matches_
    /// the_typescript_type` exists to catch for the hooks status, and this is
    /// the same guard for the same reason.
    ///
    /// The ABSENT case is the one worth pinning. A `skip_serializing_if`
    /// would drop the key entirely, which arrives in TypeScript as
    /// `undefined` rather than `null` -- and `a.pid === null` would then be
    /// false for a machine-wide alert, putting a copy button on a condition
    /// that is about no process. The key must be present and `null`.
    #[test]
    fn the_pid_is_always_on_the_wire_and_null_when_absent() {
        let machine = AlertReport {
            key: "diffuse_cpu".into(),
            title: "Several processes are using the CPU".into(),
            body: "no single process accounts for it".into(),
            pid: None,
        };
        let json = serde_json::to_value(&machine).expect("serialises");
        assert_eq!(
            json.get("pid"),
            Some(&serde_json::Value::Null),
            "the key is present and null, never omitted: {json}"
        );

        let one = AlertReport {
            pid: Some(14779),
            ..machine
        };
        assert_eq!(
            serde_json::to_value(&one).expect("serialises").get("pid"),
            Some(&serde_json::json!(14779))
        );
    }

    /// An older payload with no `pid` still decodes.
    ///
    /// `#[serde(default)]` is what makes that true, and it matters on the
    /// wire between a phone and a desktop on independent tags: adding a field
    /// to a response does not bump `PROTOCOL_VERSION`, so a newer phone
    /// reading an older desktop's answer must not fail to parse it.
    #[test]
    fn a_payload_without_a_pid_still_decodes() {
        let old = serde_json::json!({ "key": "battery_low", "title": "t", "body": "b" });
        let parsed: AlertReport = serde_json::from_value(old).expect("decodes without pid");
        assert_eq!(parsed.pid, None);
    }
}
