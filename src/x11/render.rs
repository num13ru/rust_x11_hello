//! PaperPad-owned system UI rendering.

use crate::ui::screen::ScreenLayout;
use crate::ui::system;
use anyhow::{Context, Result, ensure};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Arc, ConnectionExt, Gcontext, Rectangle, Window};
use x11rb::rust_connection::RustConnection;

/// Core X11 `ImageText8` encodes string length in one byte.
const MAX_IMAGE_TEXT8_BYTES: usize = u8::MAX as usize;
const FULL_CIRCLE_ANGLE: i16 = 360 * 64;

pub(super) fn draw(
    conn: &RustConnection,
    win: Window,
    gc: Gcontext,
    width: u16,
    height: u16,
    connected: bool,
) -> Result<()> {
    let Some(screen) = ScreenLayout::new(width, height) else {
        return Ok(());
    };
    let system_layout = system::draw_layout(width, height);
    let region = screen.system_ui_region;

    conn.clear_area(
        false,
        win,
        region.x as i16,
        region.y as i16,
        region.width,
        region.height,
    )
    .context("failed to send system-strip clear-area request")?
    .check()
    .context("X11 server rejected system-strip clear-area request")?;

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
        if let Some(indicator) = layout.connection_indicator {
            let arc = Arc {
                x: indicator.x,
                y: indicator.y,
                width: indicator.width,
                height: indicator.height,
                angle1: 0,
                angle2: FULL_CIRCLE_ANGLE,
            };
            let cookie = if connected {
                conn.poly_fill_arc(win, gc, &[arc])
                    .context("failed to send connected-indicator fill request")?
            } else {
                conn.poly_arc(win, gc, &[arc])
                    .context("failed to send disconnected-indicator outline request")?
            };
            cookie
                .check()
                .context("X11 server rejected connection-indicator draw request")?;
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
