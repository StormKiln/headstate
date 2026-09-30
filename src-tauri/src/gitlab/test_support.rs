//! Drive real subprocess fixtures without racing the OS scheduler against the
//! production clock. A script marks "$0.timeout" only when the request that
//! should time out has actually started; earlier pages must already be read.
use std::{future::Future, path::Path, time::Duration};

pub async fn scripted<T>(program: &Path, operation: impl Future<Output = T>) -> T {
    tokio::time::pause();
    let marker = program.with_extension("timeout");
    let started = std::time::Instant::now();
    let clock = async {
        loop {
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "subprocess fixture stalled"
            );
            if marker.exists() {
                std::fs::remove_file(&marker).unwrap();
                tokio::time::advance(Duration::from_secs(120)).await;
            }
            // Keep the runtime runnable: paused time must not auto-advance
            // while waiting for the operating system to launch/read a child.
            tokio::task::yield_now().await;
        }
    };
    let result = tokio::select! {
        result = operation => result,
        () = clock => unreachable!(),
    };
    tokio::time::resume();
    result
}
