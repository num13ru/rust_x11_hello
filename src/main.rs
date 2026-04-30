use std::fs::OpenOptions;
use std::io::Write;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const LOG_PATH: &str = "/mnt/us/extensions/rust_hello/hello.log";

fn main() -> std::io::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG_PATH)?;

    writeln!(log, "Hello from Rust on Kindle via KUAL. ts={now}")?;

    writeln!(log, "---- Rust-side diagnostics ----")?;
    writeln!(log, "target_arch: {}", std::env::consts::ARCH)?;
    writeln!(log, "target_os: {}", std::env::consts::OS)?;
    writeln!(log, "current_dir: {:?}", std::env::current_dir())?;

    if let Ok(output) = Command::new("uname").arg("-a").output() {
        writeln!(
            log,
            "uname -a: {}",
            String::from_utf8_lossy(&output.stdout).trim()
        )?;
    }

    writeln!(log, "---- env ----")?;
    for (key, value) in std::env::vars() {
        writeln!(log, "{key}={value}")?;
    }

    writeln!(log, "---- done ----")?;
    Ok(())
}
