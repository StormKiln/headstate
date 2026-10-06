use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

static PIPES: AtomicUsize = AtomicUsize::new(0);
struct PipeSlot;
impl PipeSlot {
    fn acquire() -> Result<Self, String> {
        PIPES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 4).then_some(n + 1)
            })
            .map(|_| Self)
            .map_err(|_| "Metadata pipe capacity is occupied; location remains unknown".into())
    }
}
impl Drop for PipeSlot {
    fn drop(&mut self) {
        PIPES.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Fixed read-only tools only. No unbounded wait for inherited output pipes.
pub fn read(
    program: &str,
    args: &[&std::ffi::OsStr],
    input: Option<&[u8]>,
    stop: &Arc<AtomicBool>,
) -> Result<Vec<u8>, String> {
    if stop.load(Ordering::Relaxed) {
        return Err("Scan canceled".into());
    }
    let reader_slot = PipeSlot::acquire()?;
    let writer_slot = input.map(|_| PipeSlot::acquire()).transpose()?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x08000000);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("Could not read command output")?;
    let (reader_tx, reader_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _slot = reader_slot;
        let mut bytes = Vec::new();
        let result = stdout
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = reader_tx.send(result);
    });
    let writer_rx = input.and_then(|input| {
        child.stdin.take().map(|mut stdin| {
            let input = input.to_vec();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _slot = writer_slot;
                let _ = tx.send(stdin.write_all(&input));
            });
            rx
        })
    });
    let started = Instant::now();
    let outcome = loop {
        if stop.load(Ordering::Relaxed) || started.elapsed() >= Duration::from_secs(3) {
            let _ = child.kill();
            let _ = child.wait();
            break Err("Command canceled or exceeded its three-second limit".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                break if status.success() {
                    Ok(())
                } else {
                    Err("Could not read location metadata".into())
                }
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(e.to_string());
            }
        }
    };
    // Only our dedicated child group. Retire descendants retaining inherited pipes.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    // Windows may retain a descendant pipe. Its thread keeps its slot until EOF;
    // timed-out readers cannot accumulate beyond the process-wide four-slot bound.
    let bytes = reader_rx
        .recv_timeout(Duration::from_millis(200))
        .map_err(|_| "Metadata output did not close within its bounded wait")?
        .map_err(|e| e.to_string())?;
    if let Some(rx) = writer_rx {
        rx.recv_timeout(Duration::from_millis(200))
            .map_err(|_| "Metadata input did not close within its bounded wait")?
            .map_err(|e| e.to_string())?;
    }
    outcome?;
    if bytes.len() > 1024 * 1024 {
        return Err("Metadata output exceeded one MiB".into());
    }
    Ok(bytes)
}
