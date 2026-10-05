/** Decimal byte strings preserve native integer precision across IPC. */
export interface DiskVolume {
    id: string;
    identity_stable: boolean;
    mount: string;
    label: string;
    scope: string;
    capacity: string | null;
    used: string | null;
    available: string | null;
    method: string;
    limitation: string | null;
}
export interface DiskLocation {
    id: string;
    identity_stable: boolean;
    path: string;
    aliases: string[];
    owners: string[];
    category: string;
    evidence: string[];
    logical: string | null;
    allocated: string | null;
    reclaimable: string | null;
    complete: boolean;
    active: boolean;
    review_only: boolean;
}
export interface DiskObservation {
    id: string;
    volume: DiskVolume;
    method: string;
    configuration: string;
    started_at: number;
    finished_at: number;
    status: string;
    visited: number;
    locations: DiskLocation[];
    categories: {
        name: string;
        allocated: string | null;
        logical: string | null;
        locations: string[];
    }[];
    coverage: {
        path: string;
        reason: string;
    }[];
    measured: string | null;
    remainder: string | null;
    accounting_note: string | null;
    approximate: boolean;
    comparison: {
        baseline_at: number | null;
        reason: string | null;
        changes: {
            location_id: string;
            path: string;
            bytes: string;
        }[];
    };
    history_error: string | null;
}
export interface DiskStatus {
    run_id: string | null;
    running: boolean;
    visited: number;
    current_path: string | null;
    observations: DiskObservation[];
    error: string | null;
}
export interface DiskSettings {
    external_roots: string[];
    additional_locations: boolean;
}
