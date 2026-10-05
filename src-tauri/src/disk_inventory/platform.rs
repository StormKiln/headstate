use super::model::Volume;
use std::{
    fs::Metadata,
    path::Path,
    sync::{atomic::AtomicBool, Arc},
};

#[derive(Clone, Debug)]
pub struct FileFacts {
    pub identity: String,
    pub volume: String,
    pub logical: u64,
    pub allocated: Option<u64>,
    pub links: u64,
}
#[cfg(unix)]
pub fn file_facts(_path: &Path, metadata: &Metadata) -> Result<FileFacts, String> {
    use std::os::unix::fs::MetadataExt;
    Ok(FileFacts {
        identity: format!("{}:{}", metadata.dev(), metadata.ino()),
        volume: metadata.dev().to_string(),
        logical: metadata.len(),
        allocated: metadata.blocks().checked_mul(512),
        links: metadata.nlink(),
    })
}
#[cfg(windows)]
pub fn file_facts(path: &Path, metadata: &Metadata) -> Result<FileFacts, String> {
    windows::facts(path, metadata)
}
#[cfg(not(any(unix, windows)))]
pub fn file_facts(_path: &Path, _metadata: &Metadata) -> Result<FileFacts, String> {
    Err("File allocation and identity are unavailable on this platform".into())
}

pub fn volumes(stop: &Arc<AtomicBool>) -> Vec<Volume> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut out: Vec<Volume> = disks.iter().map(|d| {
        let facts = std::fs::symlink_metadata(d.mount_point()).ok().and_then(|m| file_facts(d.mount_point(), &m).ok());
        let apfs = d.file_system().to_string_lossy().eq_ignore_ascii_case("apfs");
        Volume { id: format!("unverified:{}", d.mount_point().display()), device: facts.map(|f| f.volume).unwrap_or_default(), identity_stable: false, mount: d.mount_point().to_string_lossy().into_owned(), label: d.name().to_string_lossy().into_owned(), scope: if apfs { "APFS scope not yet verified" } else { "filesystem" }.into(), capacity: Some(d.total_space().to_string()), available: Some(d.available_space().to_string()), used: if apfs { None } else { d.total_space().checked_sub(d.available_space()).map(|n| n.to_string()) }, method: "filesystem-reported-total-minus-available".into(), limitation: Some("File allocation can include shared extents. Reclaimable space is not established.".into()) }
    }).collect();
    #[cfg(target_os = "macos")]
    if let Ok(value) = apfs_metadata(stop) {
        apply_apfs(&mut out, &value);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = stop;
    identify_volumes(&mut out);
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    out
}

fn identify_volumes(volumes: &mut [Volume]) {
    #[cfg(target_os = "linux")]
    if let Ok(entries) = std::fs::read_dir("/dev/disk/by-uuid") {
        for entry in entries.take(512).flatten() {
            let Ok(target) = entry.path().canonicalize() else {
                continue;
            };
            for volume in volumes.iter_mut() {
                if std::path::Path::new(&volume.label)
                    .canonicalize()
                    .ok()
                    .as_ref()
                    == Some(&target)
                {
                    volume.id = format!("filesystem-uuid:{}", entry.file_name().to_string_lossy());
                    volume.identity_stable = true;
                }
            }
        }
    }
    #[cfg(windows)]
    for volume in volumes.iter_mut() {
        if let Some(guid) = windows::volume_guid(Path::new(&volume.mount)) {
            volume.id = guid;
            volume.identity_stable = true;
        }
    }
    let _ = volumes;
}
#[cfg(target_os = "macos")]
pub(super) fn apfs_metadata(stop: &Arc<AtomicBool>) -> Result<serde_json::Value, String> {
    use std::ffi::OsStr;
    let plist = super::process::read(
        "/usr/sbin/diskutil",
        &[OsStr::new("apfs"), OsStr::new("list"), OsStr::new("-plist")],
        None,
        stop,
    )?;
    let json = super::process::read(
        "/usr/bin/plutil",
        &[
            OsStr::new("-convert"),
            OsStr::new("json"),
            OsStr::new("-o"),
            OsStr::new("-"),
            OsStr::new("--"),
            OsStr::new("-"),
        ],
        Some(&plist),
        stop,
    )?;
    serde_json::from_slice(&json).map_err(|e| e.to_string())
}
#[cfg(any(target_os = "macos", test))]
pub fn apply_apfs(volumes: &mut [Volume], payload: &serde_json::Value) {
    apply_apfs_devices(volumes, payload, |path| mount_device(Path::new(path)));
}
#[cfg(any(target_os = "macos", test))]
pub fn apply_apfs_devices(
    volumes: &mut [Volume],
    payload: &serde_json::Value,
    resolve: impl Fn(&str) -> Option<String>,
) {
    let Some(containers) = payload.get("Containers").and_then(|v| v.as_array()) else {
        return;
    };
    for container in containers {
        let Some(members) = container.get("Volumes").and_then(|v| v.as_array()) else {
            continue;
        };
        for member in members {
            let mount = member.get("MountPoint").and_then(|v| v.as_str());
            let device = member.get("DeviceIdentifier").and_then(|v| v.as_str());
            for volume in volumes.iter_mut().filter(|v| {
                mount == Some(v.mount.as_str())
                    || device.is_some_and(|expected| resolve(&v.mount).as_deref() == Some(expected))
            }) {
                // Keep the device identity used by the file walker as the ledger key;
                // record persistent UUID separately in the method/scope fingerprint.
                let Some(uuid) = member.get("APFSVolumeUUID").and_then(|v| v.as_str()) else {
                    continue;
                };
                let Some(used) = member.get("CapacityInUse").and_then(|v| v.as_u64()) else {
                    continue;
                };
                volume.id = format!("apfs:{uuid}");
                volume.identity_stable = true;
                volume.scope =
                    format!("APFS volume {uuid}; capacity and available shared with its container");
                volume.used = Some(used.to_string());
                volume.capacity = container
                    .get("CapacityCeiling")
                    .and_then(|v| v.as_u64())
                    .map(|v| v.to_string());
                volume.available = container
                    .get("CapacityFree")
                    .and_then(|v| v.as_u64())
                    .map(|v| v.to_string());
                volume.method = "apfs-volume-capacity-in-use".into();
                volume.limitation = Some("Used space is this volume's reported allocation; capacity and available are shared by its APFS container. Clones and snapshots can prevent exact folder reconciliation.".into());
            }
        }
    }
}

// Match the mounted BSD device, never the user-editable volume label. A mounted
// snapshot whose identifier differs from its volume remains unknown rather than
// guessing away a snapshot suffix or using the shared container's used bytes.
#[cfg(any(target_os = "macos", test))]
fn mount_device(path: &Path) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut facts: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(path.as_ptr(), &mut facts) } != 0 {
            return None;
        }
        let name = unsafe { std::ffi::CStr::from_ptr(facts.f_mntfromname.as_ptr()) }
            .to_str()
            .ok()?;
        Some(name.strip_prefix("/dev/")?.to_owned())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        None
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    #[repr(C)]
    struct Info {
        attributes: u32,
        created: [u32; 2],
        accessed: [u32; 2],
        written: [u32; 2],
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *mut std::ffi::c_void,
            creation: u32,
            flags: u32,
            template: *mut std::ffi::c_void,
        ) -> *mut std::ffi::c_void;
        fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut Info) -> i32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        fn GetCompressedFileSizeW(name: *const u16, high: *mut u32) -> u32;
        fn SetLastError(error: u32);
        fn GetLastError() -> u32;
        fn GetVolumeNameForVolumeMountPointW(mount: *const u16, name: *mut u16, size: u32) -> i32;
    }
    pub fn volume_guid(path: &Path) -> Option<String> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut out = [0u16; 64];
        if unsafe { GetVolumeNameForVolumeMountPointW(wide.as_ptr(), out.as_mut_ptr(), 64) } == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(
            &out[..out.iter().position(|c| *c == 0).unwrap_or(out.len())],
        ))
    }
    pub fn facts(path: &Path, metadata: &Metadata) -> Result<FileFacts, String> {
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // Metadata access only, share read/write/delete, open existing, no reparse traversal.
        unsafe {
            let handle = CreateFileW(
                path.as_ptr(),
                0,
                7,
                std::ptr::null_mut(),
                3,
                0x02200000,
                std::ptr::null_mut(),
            );
            if handle as isize == -1 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let mut info: Info = std::mem::zeroed();
            let ok = GetFileInformationByHandle(handle, &mut info);
            CloseHandle(handle);
            if ok == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            SetLastError(0);
            let mut high = 0;
            let low = GetCompressedFileSizeW(path.as_ptr(), &mut high);
            let allocated = if low == u32::MAX && GetLastError() != 0 {
                None
            } else {
                Some((u64::from(high) << 32) | u64::from(low))
            };
            Ok(FileFacts {
                identity: format!(
                    "{}:{}:{}:{}:{}",
                    info.volume, info.index_high, info.index_low, info.created[0], info.created[1]
                ),
                volume: info.volume.to_string(),
                logical: metadata.len(),
                allocated,
                links: u64::from(info.links),
            })
        }
    }
}

pub fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.is_symlink()
    }
}
