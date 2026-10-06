use super::*;
use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader, Write},
    path::Path,
};
/// Bounded reader: read_until alone can allocate an attacker-sized line. Each
/// take is capped and an overlong remainder is discarded in fixed chunks.
pub(super) fn read_records(
    path: &Path,
    mut accept: impl FnMut(&[u8], Envelope) -> io::Result<()>,
    loss: &mut Loss,
) -> io::Result<()> {
    writer::safe_path(path)?;
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = Vec::with_capacity(MAX_RECORD);
    loop {
        line.clear();
        let mut over = false;
        let mut finished = false;
        loop {
            let buf = reader.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            let n = buf
                .iter()
                .position(|b| *b == b'\n')
                .map_or(buf.len(), |n| n + 1);
            let ends = buf[n - 1] == b'\n';
            if line.len() + n <= MAX_RECORD && !over {
                line.extend_from_slice(&buf[..n]);
            } else {
                over = true
            }
            reader.consume(n);
            if ends {
                finished = true;
                break;
            }
        }
        if line.is_empty() && !over {
            break;
        }
        if over || !finished {
            loss.malformed = loss.malformed.saturating_add(1);
            if !finished {
                break;
            }
            continue;
        }
        match serde_json::from_slice::<Envelope>(&line) {
            Ok(e)
                if e.schema == 1
                    && e.epoch.len() == 32
                    && e.epoch.bytes().all(|b| b.is_ascii_hexdigit())
                    && e.event
                        .handles()
                        .iter()
                        .all(|h| h.epoch == e.epoch && h.capture == e.capture && h.id > 0) =>
            {
                accept(&line, e)?
            }
            _ => loss.malformed = loss.malformed.saturating_add(1),
        }
    }
    Ok(())
}
#[derive(serde::Serialize)]
struct Header<'a> {
    kind: &'static str,
    schema: u8,
    build: &'a str,
    platform: Platform,
    role: Role,
    cutoff_seq: u64,
    cutoff_epoch: &'a str,
    oldest_wall_ms: Option<u64>,
    newest_wall_ms: Option<u64>,
    records: u64,
    loss: &'a Loss,
    caps: &'a Caps,
    epochs: u32,
    captures: u32,
    incomplete: bool,
    deferred: &'a [DeferredAggregate],
    lifecycle: &'a Lifecycle,
    #[serde(skip_serializing_if = "Option::is_none")]
    omitted_prefix: Option<&'a OmittedPrefix>,
}
#[derive(Default, serde::Serialize)]
struct OmittedPrefix {
    records: u64,
    oldest_wall_ms: Option<u64>,
    newest_wall_ms: Option<u64>,
}
#[derive(serde::Serialize)]
struct Trailer {
    kind: &'static str,
    schema: u8,
    records: u64,
    cutoff_seq: u64,
    complete: bool,
    incomplete: bool,
}
#[allow(clippy::too_many_arguments)] // One writer-owned immutable export snapshot.
pub(super) fn write_report(
    writer: &writer::Writer,
    destination: &Path,
    cutoff: u64,
    mut loss: Loss,
    deferred: Vec<DeferredAggregate>,
    lifecycle: Lifecycle,
    cancel: &AtomicBool,
    max_bytes: Option<u64>,
) -> Result<ExportReceipt, ExportError> {
    let mut perform = || -> Result<ExportReceipt, ExportError> {
        writer::safe_path(destination).map_err(|_| ExportError::Destination)?;
        let parent = destination.parent().ok_or(ExportError::Destination)?;
        let staging = parent.join(format!(
            ".headstate-measurement-{}.pending",
            writer
                .config
                .epoch
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ));
        let mut output = writer::private_file(&staging).map_err(|_| ExportError::Destination)?;
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_file(&self.0);
            }
        }
        let _cleanup = Cleanup(staging.clone());
        // Reserve a fixed bounded metadata allowance, never materialize history.
        // Selection is a suffix of validated whole records in writer order.
        const METADATA_BYTES: u64 = 64 * 1024;
        let mut remaining_bytes = 0u64;
        if max_bytes.is_some() {
            for segment in &writer.segments {
                read_records(
                    &writer::segment_path(&writer.config.directory, segment.number),
                    |line, _| {
                        if cancel.load(Ordering::Acquire) {
                            return Err(io::Error::other("canceled"));
                        }
                        remaining_bytes = remaining_bytes.saturating_add(line.len() as u64);
                        Ok(())
                    },
                    &mut Loss::default(),
                )
                .map_err(|_| ExportError::Unavailable)?;
            }
        }
        let payload_bound = max_bytes.map(|limit| limit.saturating_sub(METADATA_BYTES));
        let mut omitted = OmittedPrefix::default();
        let mut count = 0u64;
        let mut oldest = None::<u64>;
        let mut newest = None::<u64>;
        let mut epochs = 0u32;
        let mut captures = 0u32;
        let mut previous = None::<(String, u64)>;
        // Frozen writer-owned files: validate/count, then stream selected records.
        // Recent export adds the byte-count pass above. Rotation cannot run.
        let mut detected = Loss::default();
        for segment in &writer.segments {
            read_records(
                &writer::segment_path(&writer.config.directory, segment.number),
                |line, record| {
                    if cancel.load(Ordering::Acquire) {
                        return Err(io::Error::other("canceled"));
                    }
                    if payload_bound.is_some_and(|bound| remaining_bytes > bound) {
                        remaining_bytes = remaining_bytes.saturating_sub(line.len() as u64);
                        omitted.records += 1;
                        omitted.oldest_wall_ms = Some(
                            omitted
                                .oldest_wall_ms
                                .map_or(record.wall_time_ms, |x| x.min(record.wall_time_ms)),
                        );
                        omitted.newest_wall_ms = Some(
                            omitted
                                .newest_wall_ms
                                .map_or(record.wall_time_ms, |x| x.max(record.wall_time_ms)),
                        );
                        return Ok(());
                    }
                    count += 1;
                    oldest =
                        Some(oldest.map_or(record.wall_time_ms, |x| x.min(record.wall_time_ms)));
                    newest =
                        Some(newest.map_or(record.wall_time_ms, |x| x.max(record.wall_time_ms)));
                    if previous.as_ref().is_none_or(|p| p.0 != record.epoch) {
                        epochs += 1
                    }
                    if previous.as_ref() != Some(&(record.epoch.clone(), record.capture)) {
                        captures += 1
                    }
                    previous = Some((record.epoch, record.capture));
                    Ok(())
                },
                &mut detected,
            )
            .map_err(|_| ExportError::Unavailable)?;
        }
        let known: u64 = writer.segments.iter().map(|s| s.malformed).sum();
        loss.malformed = loss
            .malformed
            .saturating_add(detected.malformed.saturating_sub(known));
        let incomplete = loss.incomplete() || omitted.records > 0;
        let header = Header {
            kind: "header",
            schema: 1,
            build: &writer.config.build,
            platform: writer.config.platform,
            role: writer.config.role,
            cutoff_seq: cutoff,
            cutoff_epoch: &writer
                .config
                .epoch
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            oldest_wall_ms: oldest,
            newest_wall_ms: newest,
            records: count,
            loss: &loss,
            caps: &writer.caps,
            epochs,
            captures,
            incomplete,
            deferred: &deferred,
            lifecycle: &lifecycle,
            omitted_prefix: max_bytes.map(|_| &omitted),
        };
        let header_bytes = serde_json::to_vec(&header).map_err(|_| ExportError::Destination)?;
        if header_bytes.len() as u64 + 1024 > METADATA_BYTES {
            return Err(ExportError::Destination);
        }
        output
            .write_all(&header_bytes)
            .map_err(|_| ExportError::Destination)?;
        output
            .write_all(b"\n")
            .map_err(|_| ExportError::Destination)?;
        let mut skip = omitted.records;
        for segment in &writer.segments {
            read_records(
                &writer::segment_path(&writer.config.directory, segment.number),
                |line, _| {
                    if cancel.load(Ordering::Acquire) {
                        return Err(io::Error::other("canceled"));
                    }
                    if skip > 0 {
                        skip -= 1;
                        return Ok(());
                    }
                    output.write_all(line)
                },
                &mut Loss::default(),
            )
            .map_err(|_| ExportError::Destination)?;
        }
        serde_json::to_writer(
            &mut output,
            &Trailer {
                kind: "trailer",
                schema: 1,
                records: count,
                cutoff_seq: cutoff,
                complete: true,
                incomplete,
            },
        )
        .map_err(|_| ExportError::Destination)?;
        output
            .write_all(b"\n")
            .map_err(|_| ExportError::Destination)?;
        output
            .flush()
            .and_then(|_| output.sync_all())
            .map_err(|_| ExportError::Destination)?;
        let bytes = output
            .metadata()
            .map_err(|_| ExportError::Destination)?
            .len();
        if bytes > max_bytes.unwrap_or(129 * 1024 * 1024) {
            return Err(ExportError::Destination);
        }
        if cancel.load(Ordering::Acquire) {
            return Err(ExportError::Canceled);
        }
        drop(output);
        fs::rename(&staging, destination).map_err(|_| ExportError::Destination)?;
        Ok(ExportReceipt {
            canceled: false,
            records: count,
            bytes,
            oldest_wall_ms: oldest,
            newest_wall_ms: newest,
            incomplete,
        })
    };
    perform()
}
