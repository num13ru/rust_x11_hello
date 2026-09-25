//! PaperPad-owned system UI rendering.

use crate::ui::system;
use anyhow::{Context, Result, ensure};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, Gcontext, Rectangle, Window};
use x11rb::rust_connection::RustConnection;

/// Core X11 `ImageText8` encodes string length in one byte.
const MAX_IMAGE_TEXT8_BYTES: usize = u8::MAX as usize;

pub(super) fn draw(
    conn: &RustConnection,
    win: Window,
    gc: Gcontext,
    width: u16,
    height: u16,
) -> Result<()> {
    let system_layout = system::draw_layout(width, height);

    conn.clear_area(false, win, 0, 0, width, height)
        .context("failed to send clear-area request")?
        .check()
        .context("X11 server rejected clear-area request")?;

    if let Some(layout) = system_layout {
        conn.poly_rectangle(
            win,
            gc,
            &[Rectangle {
                x: layout.rectangle.x,
                y: layout.rectangle.y,
                width: layout.rectangle.width,
                height: layout.rectangle.height,
            }],
        )
        .context("failed to send system rectangle draw request")?
        .check()
        .context("X11 server rejected system rectangle draw request")?;

        if layout.text.y >= 0 {
            image_text(
                conn,
                win,
                gc,
                layout.text.x,
                layout.text.y,
                layout.text.text,
            )?;
        }
    }

    conn.flush().context("failed to flush X11 draw requests")?;
    Ok(())
}

fn image_text(
    conn: &RustConnection,
    win: Window,
    gc: Gcontext,
    x: i16,
    y: i16,
    text: &[u8],
) -> Result<()> {
    ensure!(
        text.len() <= MAX_IMAGE_TEXT8_BYTES,
        "X11 ImageText8 payload exceeds {MAX_IMAGE_TEXT8_BYTES} bytes"
    );
    conn.image_text8(win, gc, x, y, text)
        .context("failed to send text draw request")?
        .check()
        .context("X11 server rejected text draw request")?;
    Ok(())
}
