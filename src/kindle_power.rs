//! One-shot subscription to Kindle powerd transitions that end a PaperPad session.

use anyhow::{Context, Result, ensure};
use std::io::Read;
use std::os::fd::{AsFd, BorrowedFd};
use std::process::{Child, ChildStdout, Command, Stdio};

const EVENTS: &str =
    "goingToScreenSaver,readyToSuspend,wakeupFromSuspend,exitingScreenSaver,outOfScreenSaver";

pub(crate) struct KindlePowerMonitor {
    child: Child,
    output: ChildStdout,
}

impl KindlePowerMonitor {
    pub(crate) fn start() -> Result<Self> {
        let mut child = Command::new("lipc-wait-event")
            .args(["-s", "0", "com.lab126.powerd", EVENTS])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .context("start Kindle power listener (lipc-wait-event)")?;
        let output = child.stdout.take().expect("piped Kindle power output");
        Ok(Self { child, output })
    }

    pub(crate) fn fd(&self) -> BorrowedFd<'_> {
        self.output.as_fd()
    }

    /// Read a record only after poll reported data or a closed pipe.
    pub(crate) fn report_transition(&mut self) -> Result<()> {
        let mut buffer = [0; 128];
        let count = self
            .output
            .read(&mut buffer)
            .context("read Kindle power event")?;
        let record = String::from_utf8_lossy(&buffer[..count]);
        let name = record.split_whitespace().next().unwrap_or_default();
        ensure!(
            EVENTS.split(',').any(|event| event == name),
            "Kindle power listener ended without a subscribed event: {:?}",
            record.trim()
        );
        eprintln!("PaperPad native runtime stopped reason=kindle_power event={name}");
        Ok(())
    }
}

impl Drop for KindlePowerMonitor {
    fn drop(&mut self) {
        // On ordinary Exit the one-shot listener may still be waiting.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
