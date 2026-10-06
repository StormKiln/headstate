import { call } from "./transport";
import type { DiskObservation, DiskSettings, DiskStatus } from "../types/disk";
export const diskStatus = () => call<DiskStatus>("disk_inventory_status");
export const diskHistory = () => call<DiskObservation[]>("disk_inventory_history");
export const diskSettings = () => call<DiskSettings>("disk_inventory_settings");
export const saveDiskSettings = (settings: DiskSettings) => call<void>("set_disk_inventory_settings", { settings });
export const startDiskScan = () => call<DiskStatus>("start_disk_inventory");
export const cancelDiskScan = (runId: string) => call<void>("cancel_disk_inventory", { runId });
