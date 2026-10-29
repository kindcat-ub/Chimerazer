//! Device I/O off the UI thread. One worker thread runs one job at a time;
//! the device is opened (and locked) only for the duration of a job.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use chimerazer_devices::UsbId;
use chimerazer_devices::naga_pro::params::Param;
use chimerazer_devices::naga_pro::restore::{self, Entry, RestoreReport};
use chimerazer_devices::naga_pro::state::{self, Snapshot};
use chimerazer_transport::Hidraw;

/// A Naga Pro control interface found on the system.
#[derive(Clone, Debug)]
pub struct Device {
    pub node: PathBuf,
    pub usb: &'static UsbId,
}

#[derive(Debug)]
pub enum Job {
    Snapshot,
    /// Writes each value in normal mode, then reads a fresh snapshot.
    Set(Vec<(Param, String)>),
    Restore {
        dump: BTreeMap<String, String>,
        dry_run: bool,
    },
}

#[derive(Debug)]
pub enum Done {
    Snapshot(Result<Snapshot, String>),
    Set(Result<(Vec<Entry>, Snapshot), String>),
    Restore(Result<RestoreReport, String>, bool),
}

#[derive(Debug)]
pub struct Worker {
    jobs: Sender<(Device, Job)>,
    pub done: Receiver<Done>,
}

impl Worker {
    /// `wake` is called after each finished job (to repaint the UI).
    pub fn spawn(wake: impl Fn() + Send + 'static) -> Self {
        let (jobs, inbox) = channel::<(Device, Job)>();
        let (outbox, done) = channel();
        thread::spawn(move || {
            for (device, job) in inbox {
                if outbox.send(run(&device, job)).is_err() {
                    break;
                }
                wake();
            }
        });
        Self { jobs, done }
    }

    pub fn submit(&self, device: Device, job: Job) {
        // The worker only stops when the UI is gone.
        let _ = self.jobs.send((device, job));
    }
}

fn run(device: &Device, job: Job) -> Done {
    let open = || Hidraw::open(&device.node).map_err(|e| format!("{}: {e}", device.node.display()));
    match job {
        Job::Snapshot => {
            Done::Snapshot(open().map(|mut d| state::read_snapshot(&mut d, device.usb)))
        }
        Job::Set(items) => Done::Set(open().map(|mut d| {
            let mut entries = Vec::new();
            for (param, text) in items {
                let e = restore::set(&mut d, device.usb, param, &text);
                let go_on = matches!(
                    e.outcome,
                    restore::Outcome::Restored { .. } | restore::Outcome::Unchanged
                );
                entries.push(e);
                if !go_on {
                    break;
                }
            }
            (entries, state::read_snapshot(&mut d, device.usb))
        })),
        Job::Restore { dump, dry_run } => Done::Restore(
            open().map(|mut d| restore::restore(&mut d, device.usb, &dump, dry_run)),
            dry_run,
        ),
    }
}
