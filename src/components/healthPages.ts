import {
  BatteryCharging,
  Cpu,
  Gauge,
  HardDrive,
  MemoryStick,
  MonitorCog,
  Network,
} from "lucide-react";
import type { HealthPage } from "../store/filters";

/// Apart from `SystemHealthSidebar.tsx` so that file exports only
/// components (#1965), which is what lets dev hot-reload swap it in place.

/// Every System Health page, in sidebar order, with its label and icon.
///
/// Exported because the PHONE renders the same list as a row of tappable
/// cards on the overview -- there is no persistent sidebar at that width
/// (see `SystemHealthPage`), so the class list has to appear somewhere
/// the thumb can reach. One array so the two cannot drift into offering
/// different pages, which is the failure #675 fixed for `VIEWS`.
///
/// The `blurb` is for the phone only: a sidebar row is read beside the
/// page it opens, so its label is enough, but a card on the overview is
/// read BEFORE anything is open and has to say what is behind it.
export const HEALTH_PAGES: {
  id: HealthPage;
  label: string;
  blurb: string;
  Icon: typeof Cpu;
}[] = [
  {
    id: "overview",
    label: "Overview",
    blurb: "Everything at a glance",
    Icon: Gauge,
  },
  {
    id: "cpu",
    label: "CPU",
    blurb: "Per-core, load, and what is using it",
    Icon: Cpu,
  },
  {
    id: "memory",
    label: "Memory",
    blurb: "Pressure, swap, and what is holding it",
    Icon: MemoryStick,
  },
  { id: "disk", label: "Disk", blurb: "Every volume, and Headstate's share", Icon: HardDrive },
  {
    id: "gpu",
    label: "GPU",
    blurb: "Utilization, memory, and the pipeline stages",
    Icon: MonitorCog,
  },
  {
    id: "network",
    label: "Network",
    blurb: "Every interface, and what is using it",
    Icon: Network,
  },
  {
    id: "power",
    label: "Power and uptime",
    blurb: "Battery, thermal pressure, uptime",
    Icon: BatteryCharging,
  },
];

/// The pages worth offering on THIS machine.
///
/// Every page in `HEALTH_PAGES` but GPU is offered unconditionally: a
/// machine always has a CPU, memory, volumes, interfaces and an uptime,
/// and where one of those cannot be read the page says "Not measured",
/// which is informative.
///
/// GPU is the exception, and it is the same rule #705 set for the
/// overview panel. An empty GPU page would claim we found a graphics
/// device and could not read it -- which on Windows and on Intel or
/// NVIDIA Linux is not what happened: we did not look, because there is
/// no unprivileged way to. So a machine with no discoverable GPU is
/// offered no GPU page at all, exactly as #717 asks.
///
/// Derived from the live sample rather than from a capability flag, for
/// the same reason the panel is: `gpus` being empty IS the fact, and a
/// second source for it could disagree with the first.
///
/// Exported so the phone's card list and the sidebar cannot drift into
/// offering different pages -- the failure `HEALTH_PAGES` itself exists
/// to prevent.
export function healthPagesFor(gpuCount: number) {
  return HEALTH_PAGES.filter((p) => p.id !== "gpu" || gpuCount > 0);
}
