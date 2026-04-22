use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use tokio::sync::mpsc;

/// Start watching a directory for file changes.
/// Returns a receiver that emits batched file paths after 300ms debounce.
pub fn start_watcher(
    root: PathBuf,
    exclude: Vec<String>,
) -> Result<mpsc::UnboundedReceiver<Vec<PathBuf>>> {
    let (tx, rx) = mpsc::unbounded_channel();

    let (debounce_tx, debounce_rx) = std::sync::mpsc::channel();

    let mut debouncer = new_debouncer(Duration::from_millis(300), debounce_tx)?;

    debouncer.watcher().watch(
        root.as_ref(),
        notify::RecursiveMode::Recursive,
    )?;

    // Spawn a thread to read debounced events and forward to async channel
    std::thread::spawn(move || {
        let _debouncer = debouncer; // keep alive
        loop {
            match debounce_rx.recv() {
                Ok(Ok(events)) => {
                    let paths: Vec<PathBuf> = events
                        .into_iter()
                        .filter(|e| e.kind == DebouncedEventKind::Any)
                        .map(|e| e.path)
                        .filter(|p| {
                            let path_str = p.to_string_lossy();
                            !exclude.iter().any(|ex| path_str.contains(ex.as_str()))
                        })
                        .filter(|p| p.is_file())
                        .collect();

                    if !paths.is_empty() {
                        if tx.send(paths).is_err() {
                            break;
                        }
                    }
                }
                Ok(Err(_)) => continue,
                Err(_) => break,
            }
        }
    });

    Ok(rx)
}
