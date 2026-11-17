//! Render the pinned-font Mono1 application UI to a portable bitmap.

use std::error::Error;
use std::fs;
use std::path::PathBuf;

use paperspoon::application_renderer::ApplicationRenderer;
use paperspoon::application_ui::ApplicationUi;

const WIDTH: u16 = 1272;
const HEIGHT: u16 = 1624;

fn main() -> Result<(), Box<dyn Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".cache/typography-spike/application-ui-1272x1624.pbm"));
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let mut ui = ApplicationUi::default();
    ui.set_status("PaperSpoon: ready — Привет");
    let frame = ApplicationRenderer::new()?.render(&ui, WIDTH, HEIGHT)?;
    let mut bytes = format!("P4\n{} {}\n", frame.width(), frame.height()).into_bytes();
    bytes.extend_from_slice(frame.pixels());
    fs::write(&output, bytes)?;
    println!("{}", output.display());
    Ok(())
}
