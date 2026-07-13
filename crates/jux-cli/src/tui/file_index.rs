use notify::{Event, RecursiveMode, Watcher};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const REFRESH_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileIndexKind {
    Git,
    Filesystem,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileIndexSnapshot {
    pub kind: FileIndexKind,
    pub files: Vec<String>,
}

pub struct FileIndexService {
    receiver: Receiver<FileIndexSnapshot>,
    shutdown: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl FileIndexService {
    /// Starts indexing without blocking the TUI thread on Git or filesystem IO.
    ///
    /// The worker publishes an initial snapshot before watcher setup, then
    /// reconciles once after registration. Reconciliation closes the race where
    /// files change after the first snapshot but before notifications are active.
    #[must_use]
    pub fn start(root: PathBuf) -> Self {
        let (sender, receiver) = mpsc::channel();
        let (shutdown, shutdown_receiver) = mpsc::channel();
        let worker = thread::spawn(move || run_index_thread(root, sender, shutdown_receiver));
        Self {
            receiver,
            shutdown,
            worker: Some(worker),
        }
    }

    pub fn try_recv_latest(&self) -> Option<FileIndexSnapshot> {
        let mut latest = None;
        while let Ok(snapshot) = self.receiver.try_recv() {
            latest = Some(snapshot);
        }
        latest
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<FileIndexSnapshot> {
        self.receiver.recv_timeout(timeout).ok()
    }
}

impl Drop for FileIndexService {
    fn drop(&mut self) {
        // Explicit shutdown prevents detached watcher threads from accumulating
        // across session switches, tests, or repeated TUI construction.
        let _ = self.shutdown.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_index_thread(
    root: PathBuf,
    sender: mpsc::Sender<FileIndexSnapshot>,
    shutdown: Receiver<()>,
) {
    let mut current = build_file_index(&root);
    if sender.send(current.clone()).is_err() {
        return;
    }
    let (watch_sender, watch_receiver) = mpsc::channel::<notify::Result<Event>>();
    let Ok(mut watcher) = notify::recommended_watcher(move |event| {
        let _ = watch_sender.send(event);
    }) else {
        return;
    };
    let _ = watcher.watch(&root, RecursiveMode::Recursive);
    if let Some(git_dir) = git_directory(&root) {
        let _ = watcher.watch(&git_dir, RecursiveMode::NonRecursive);
    }
    let reconciled = build_file_index(&root);
    if reconciled != current {
        current = reconciled;
        if sender.send(current.clone()).is_err() {
            return;
        }
    }
    loop {
        if shutdown.try_recv().is_ok() {
            return;
        }
        match watch_receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(_) => {
                let deadline = Instant::now() + REFRESH_DEBOUNCE;
                while Instant::now() < deadline {
                    if shutdown.try_recv().is_ok() {
                        return;
                    }
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    match watch_receiver.recv_timeout(remaining.min(Duration::from_millis(50))) {
                        Ok(_) => {}
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                let refreshed = build_file_index(&root);
                if refreshed != current {
                    current = refreshed;
                    if sender.send(current.clone()).is_err() {
                        return;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

pub(crate) fn build_file_index(root: &Path) -> FileIndexSnapshot {
    git_files(root).map_or_else(
        || FileIndexSnapshot {
            kind: FileIndexKind::Filesystem,
            files: filesystem_files(root),
        },
        |files| FileIndexSnapshot {
            kind: FileIndexKind::Git,
            files,
        },
    )
}

fn git_files(root: &Path) -> Option<Vec<String>> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["ls-files", "-z"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut files = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .filter_map(|path| String::from_utf8(path.to_vec()).ok())
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    Some(files)
}

fn git_directory(root: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

fn filesystem_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    visit_directory(root, root, &mut files);
    files.sort();
    files
}

fn visit_directory(root: &Path, directory: &Path, files: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if entry.file_name() != ".git" && !file_type.is_symlink() {
                visit_directory(root, &path, files);
            }
        } else if file_type.is_file()
            && let Ok(relative) = path.strip_prefix(root)
        {
            files.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}
