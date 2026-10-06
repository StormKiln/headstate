use super::*;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    path::Path,
    sync::mpsc::{Receiver, TryRecvError},
    time::Duration,
};
pub(super) enum Control {
    #[cfg(test)]
    Pause {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    },
    #[cfg(test)]
    FlushAcknowledged {
        cutoff: u64,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    Flush {
        cutoff: u64,
    },
    Export {
        cutoff: u64,
        loss: Box<Loss>,
        deferred: Vec<DeferredAggregate>,
        lifecycle: Lifecycle,
        destination: PathBuf,
        reply: tokio::sync::oneshot::Sender<Result<ExportReceipt, ExportError>>,
        ready: tokio::sync::oneshot::Sender<()>,
        cancel: Arc<AtomicBool>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Segment {
    pub number: u64,
    pub bytes: u64,
    pub records: u64,
    pub oldest: Option<u64>,
    pub newest: Option<u64>,
    pub first_epoch: Option<String>,
    pub last_epoch: Option<String>,
    pub epochs: u32,
    pub malformed: u64,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    segments: Vec<Segment>,
    loss: Loss,
    lifecycle: Lifecycle,
    #[serde(default)]
    deferred_pending: bool,
}
pub(super) struct Writer {
    pub config: Config,
    pub caps: Caps,
    pub shared: Arc<Shared>,
    pub segments: Vec<Segment>,
    file: Option<BufWriter<File>>,
    pending: usize,
    last_flush: Instant,
    processed: u64,
}
pub(super) fn safe_path(path: &Path) -> io::Result<()> {
    for ancestor in path.ancestors() {
        if let Ok(meta) = fs::symlink_metadata(ancestor) {
            if meta.file_type().is_symlink() {
                return Err(io::Error::other("symlink"));
            }
        }
    }
    Ok(())
}
pub(super) fn private_file(path: &Path) -> io::Result<File> {
    safe_path(path)?;
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}
pub(super) fn segment_path(root: &Path, n: u64) -> PathBuf {
    root.join(format!("segment-{n}.jsonl"))
}
impl Writer {
    pub fn open(config: Config, caps: Caps, shared: Arc<Shared>) -> Result<Self, ExportError> {
        safe_path(&config.directory).map_err(|_| ExportError::Unavailable)?;
        fs::create_dir_all(&config.directory).map_err(|_| ExportError::Unavailable)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&config.directory, fs::Permissions::from_mode(0o700))
                .map_err(|_| ExportError::Unavailable)?;
        }
        let manifest_path = config.directory.join("manifest.json");
        safe_path(&manifest_path).map_err(|_| ExportError::Unavailable)?;
        let mut manifest = Manifest::default();
        if manifest_path.exists() {
            let m = fs::metadata(&manifest_path).map_err(|_| ExportError::Unavailable)?;
            if m.len() >= 65536 {
                return Err(ExportError::Unavailable);
            }
            manifest = serde_json::from_reader(
                File::open(manifest_path).map_err(|_| ExportError::Unavailable)?,
            )
            .map_err(|_| ExportError::Unavailable)?;
            if manifest.segments.len() > caps.segments {
                return Err(ExportError::Unavailable);
            }
        }
        if manifest.deferred_pending {
            manifest.loss.deferred_aggregate_gaps =
                manifest.loss.deferred_aggregate_gaps.saturating_add(1);
        }
        if manifest.lifecycle.active_capture.is_some() {
            manifest.loss.unclean_capture = manifest.loss.unclean_capture.saturating_add(1);
        }
        // Reconcile a crash between segment flush and manifest replacement by
        // reading only known bounded files. Never sweep arbitrary app files.
        let mut numbers = Vec::new();
        for entry in fs::read_dir(&config.directory).map_err(|_| ExportError::Unavailable)? {
            let entry = entry.map_err(|_| ExportError::Unavailable)?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(n) = name
                .strip_prefix("segment-")
                .and_then(|s| s.strip_suffix(".jsonl"))
                .and_then(|s| s.parse::<u64>().ok())
            {
                if entry
                    .file_type()
                    .map_err(|_| ExportError::Unavailable)?
                    .is_symlink()
                {
                    return Err(ExportError::Unavailable);
                }
                numbers.push(n);
                if numbers.len() > caps.segments + 1 {
                    return Err(ExportError::Unavailable);
                }
            }
        }
        numbers.sort_unstable();
        let previous_bad: HashMap<u64, u64> = manifest
            .segments
            .iter()
            .map(|s| (s.number, s.malformed))
            .collect();
        let previous = std::mem::take(&mut manifest.segments);
        for n in numbers {
            let path = segment_path(&config.directory, n);
            if fs::metadata(&path)
                .map_err(|_| ExportError::Unavailable)?
                .len()
                > caps.segment_bytes
            {
                return Err(ExportError::Unavailable);
            }
            let mut seg = Segment {
                number: n,
                bytes: 0,
                records: 0,
                oldest: None,
                newest: None,
                first_epoch: None,
                last_epoch: None,
                epochs: 0,
                malformed: 0,
            };
            let mut detected = Loss::default();
            export::read_records(
                &path,
                |line, record| {
                    seg.bytes += line.len() as u64;
                    seg.records += 1;
                    if seg.last_epoch.as_ref() != Some(&record.epoch) {
                        seg.epochs = seg.epochs.saturating_add(1);
                    }
                    if seg.first_epoch.is_none() {
                        seg.first_epoch = Some(record.epoch.clone());
                    }
                    seg.last_epoch = Some(record.epoch.clone());
                    seg.oldest = Some(
                        seg.oldest
                            .map_or(record.wall_time_ms, |x| x.min(record.wall_time_ms)),
                    );
                    seg.newest = Some(
                        seg.newest
                            .map_or(record.wall_time_ms, |x| x.max(record.wall_time_ms)),
                    );
                    Ok(())
                },
                &mut detected,
            )
            .map_err(|_| ExportError::Unavailable)?;
            seg.malformed = detected.malformed;
            manifest.loss.malformed = manifest.loss.malformed.saturating_add(
                detected
                    .malformed
                    .saturating_sub(*previous_bad.get(&n).unwrap_or(&0)),
            );
            seg.bytes = fs::metadata(path)
                .map_err(|_| ExportError::Unavailable)?
                .len();
            manifest.segments.push(seg);
        }
        for old in previous {
            let current = manifest.segments.iter().find(|s| s.number == old.number);
            let records = old.records.saturating_sub(current.map_or(0, |s| s.records));
            let bytes = old.bytes.saturating_sub(current.map_or(0, |s| s.bytes));
            if current.is_none() || records > 0 || bytes > 0 {
                manifest.loss.durable_gap_segments =
                    manifest.loss.durable_gap_segments.saturating_add(1);
                manifest.loss.durable_gap_records =
                    manifest.loss.durable_gap_records.saturating_add(records);
                manifest.loss.durable_gap_bytes =
                    manifest.loss.durable_gap_bytes.saturating_add(bytes);
            }
        }
        *shared.loss.lock().unwrap() = manifest.loss;
        let mut writer = Self {
            config,
            caps,
            shared,
            segments: manifest.segments,
            file: None,
            pending: 0,
            last_flush: Instant::now(),
            processed: 0,
        };
        writer.trim().map_err(|_| ExportError::Unavailable)?;
        writer.publish();
        Ok(writer)
    }
    fn trim(&mut self) -> io::Result<()> {
        while self.segments.len() > self.caps.segments {
            let old = self.segments.remove(0);
            fs::remove_file(segment_path(&self.config.directory, old.number))?;
            let mut l = self.shared.loss.lock().unwrap();
            l.rotated_out = l.rotated_out.saturating_add(old.records);
            l.rotated_bytes = l.rotated_bytes.saturating_add(old.bytes);
        }
        Ok(())
    }
    fn rotate(&mut self) -> io::Result<()> {
        self.flush()?;
        self.file = None;
        let number = self.segments.last().map_or(0, |s| s.number + 1);
        let file = private_file(&segment_path(&self.config.directory, number))?;
        file.sync_all()?;
        self.segments.push(Segment {
            number,
            bytes: 0,
            records: 0,
            oldest: None,
            newest: None,
            first_epoch: None,
            last_epoch: None,
            epochs: 0,
            malformed: 0,
        });
        self.file = Some(BufWriter::new(file));
        self.trim()?;
        self.persist()
    }
    fn write(&mut self, record: Envelope) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(&record)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_RECORD {
            return Err(io::Error::other("record bound"));
        }
        if self.file.is_none()
            || self
                .segments
                .last()
                .is_some_and(|s| s.bytes + bytes.len() as u64 > self.caps.segment_bytes)
        {
            self.rotate()?
        }
        self.file.as_mut().unwrap().write_all(&bytes)?;
        let s = self.segments.last_mut().unwrap();
        s.bytes += bytes.len() as u64;
        s.records += 1;
        if s.last_epoch.as_ref() != Some(&record.epoch) {
            s.epochs = s.epochs.saturating_add(1);
        }
        if s.first_epoch.is_none() {
            s.first_epoch = Some(record.epoch.clone());
        }
        s.last_epoch = Some(record.epoch.clone());
        s.oldest = Some(
            s.oldest
                .map_or(record.wall_time_ms, |x| x.min(record.wall_time_ms)),
        );
        s.newest = Some(
            s.newest
                .map_or(record.wall_time_ms, |x| x.max(record.wall_time_ms)),
        );
        self.processed = record.seq;
        self.pending += 1;
        if self.pending >= 128 {
            self.flush()?
        }
        Ok(())
    }
    fn persist(&self) -> io::Result<()> {
        let manifest = Manifest {
            segments: self.segments.clone(),
            loss: self.shared.loss.lock().unwrap().clone(),
            lifecycle: self.shared.lifecycle.lock().unwrap().clone(),
            deferred_pending: self.shared.deferred_pending.load(Ordering::Acquire),
        };
        let data = serde_json::to_vec(&manifest)?;
        if data.len() >= 65536 {
            return Err(io::Error::other("manifest bound"));
        }
        let temp = self.config.directory.join("manifest.pending");
        // Our known interrupted staging file contains no authority and is never
        // read as a manifest. Refuse symlinks rather than following them.
        safe_path(&temp)?;
        if temp.exists() {
            fs::remove_file(&temp)?
        }
        let mut file = private_file(&temp)?;
        file.write_all(&data)?;
        file.sync_all()?;
        fs::rename(&temp, self.config.directory.join("manifest.json"))
    }
    fn publish(&self) {
        let mut status = self.shared.status.lock().unwrap();
        status.durable_records = self.segments.iter().map(|s| s.records).sum();
        status.bytes = self.segments.iter().map(|s| s.bytes).sum();
        status.oldest_wall_ms = self.segments.iter().filter_map(|s| s.oldest).min();
        status.newest_wall_ms = self.segments.iter().filter_map(|s| s.newest).max();
        status.durable_seq = self.processed;
        let mut previous = None;
        let mut epochs = 0u32;
        for segment in &self.segments {
            epochs = epochs.saturating_add(segment.epochs);
            if previous.is_some() && previous == segment.first_epoch.as_ref() {
                epochs = epochs.saturating_sub(1)
            }
            if segment.last_epoch.is_some() {
                previous = segment.last_epoch.as_ref();
            }
        }
        status.epochs = epochs;
    }
    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.flush()?;
            file.get_ref().sync_all()?
        }
        self.persist()?;
        self.pending = 0;
        self.last_flush = Instant::now();
        self.publish();
        Ok(())
    }
    fn fail(&self) {
        self.shared.unavailable.store(true, Ordering::Release);
        self.shared.loss.lock().unwrap().writer += 1;
    }
    fn through(
        &mut self,
        cutoff: u64,
        data: &Receiver<Envelope>,
        pending: &mut Option<Envelope>,
    ) -> io::Result<()> {
        while self.processed < cutoff {
            let record = match pending.take() {
                Some(record) => record,
                None => data
                    .recv_timeout(Duration::from_millis(100))
                    .map_err(|_| io::Error::other("missing accepted record"))?,
            };
            self.write(record)?;
        }
        self.flush()
    }
    pub fn run(mut self, data: Receiver<Envelope>, controls: Receiver<Control>) {
        let mut pending = None;
        loop {
            let control = controls.try_recv();
            let handled = control.is_ok();
            match control {
                #[cfg(test)]
                Ok(Control::Pause { entered, release }) => {
                    let _ = entered.send(());
                    let _ = release.recv();
                }
                #[cfg(test)]
                Ok(Control::FlushAcknowledged { cutoff, reply }) => {
                    let success = self.through(cutoff, &data, &mut pending).is_ok();
                    if !success {
                        self.fail();
                    }
                    let _ = reply.send(success);
                }
                Ok(Control::Flush { cutoff }) => {
                    if self.through(cutoff, &data, &mut pending).is_err() {
                        self.fail();
                    }
                }
                Ok(Control::Export {
                    cutoff,
                    mut loss,
                    deferred,
                    lifecycle,
                    destination,
                    reply,
                    ready,
                    cancel,
                }) => {
                    let result = if self.shared.unavailable.load(Ordering::Acquire) {
                        Err(ExportError::Unavailable)
                    } else if self.through(cutoff, &data, &mut pending).is_err() {
                        self.fail();
                        Err(ExportError::Unavailable)
                    } else {
                        let _ = ready.send(());
                        let current = self.shared.loss.lock().unwrap();
                        loss.rotated_out = current.rotated_out;
                        loss.rotated_bytes = current.rotated_bytes;
                        drop(current);
                        export::write_report(
                            &self,
                            &destination,
                            cutoff,
                            *loss,
                            deferred,
                            lifecycle,
                            &cancel,
                        )
                    };
                    // Completion must release admission before waking the caller.
                    self.shared.exporting.store(false, Ordering::Release);
                    let _ = reply.send(result);
                    #[cfg(test)]
                    if let Some(gate) = self.shared.export_reply_gate.lock().unwrap().take() {
                        let _ = gate.recv_timeout(Duration::from_secs(2));
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => break,
            }
            // Drain earlier controls before admitting post-cutoff data to disk.
            if handled {
                continue;
            }
            if let Some(record) = pending.take() {
                if self.shared.unavailable.load(Ordering::Acquire) {
                    self.shared.loss.lock().unwrap().dropped += 1;
                } else if self.write(record).is_err() {
                    self.fail();
                }
            }
            if self.shared.stopping.load(Ordering::Acquire) {
                while let Ok(record) = data.try_recv() {
                    if self.write(record).is_err() {
                        self.fail();
                        break;
                    }
                }
                if self.flush().is_err() {
                    self.fail()
                }
                break;
            }
            match data.recv_timeout(Duration::from_millis(50)) {
                Ok(record) => {
                    pending = Some(record);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if !self.shared.unavailable.load(Ordering::Acquire)
                && self.last_flush.elapsed() >= Duration::from_secs(5)
                && self.flush().is_err()
            {
                self.fail()
            }
        }
    }
}
