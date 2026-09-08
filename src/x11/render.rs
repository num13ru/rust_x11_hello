//! X11 rendering adapter for the wire-independent UI layout.

use crate::ui::geometry::{STATUS_BAR_HEIGHT, draw_layout};
use anyhow::{Context, Result};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, Gcontext, Rectangle, Window};
use x11rb::rust_connection::RustConnection;

/// Left inset of status text drawn in the status strip.
const STATUS_TEXT_X: u16 = 20;
/// Vertical distance from the window's bottom edge to the status baseline.
///
/// The baseline sits inside the status strip, below the exit bar, with room
/// for the text's ascent.
const STATUS_TEXT_BOTTOM_MARGIN: u16 = 10;

const _: () = {
    assert!(STATUS_BAR_HEIGHT > STATUS_TEXT_BOTTOM_MARGIN);
};

pub(super) fn draw(
    conn: &RustConnection,
    win: Window,
    gc: Gcontext,
    width: u16,
    height: u16,
    status_text: Option<&str>,
) -> Result<()> {
    let Some(layout) = draw_layout(width, height) else {
        return Ok(());
    };

    conn.clear_area(false, win, 0, 0, width, height)
        .context("failed to send clear-area request")?
        .check()
        .context("X11 server rejected clear-area request")?;

    let rectangles: Vec<Rectangle> = layout
        .rectangles
        .iter()
        .map(|rectangle| Rectangle {
            x: rectangle.x,
            y: rectangle.y,
            width: rectangle.width,
            height: rectangle.height,
        })
        .collect();
    if !rectangles.is_empty() {
        conn.poly_rectangle(win, gc, &rectangles)
            .context("failed to send rectangle draw request")?
            .check()
            .context("X11 server rejected rectangle draw request")?;
    }

    for placement in layout.text {
        if placement.y > 0 {
            draw_text(conn, win, gc, placement.x, placement.y, placement.text)?;
        }
    }

    if let Some(text) = status_text {
        // Baseline inside the status strip, below the exit bar.
        let status_y = height
            .saturating_sub(STATUS_TEXT_BOTTOM_MARGIN)
            .min(i16::MAX as u16) as i16;
        draw_text(
            conn,
            win,
            gc,
            STATUS_TEXT_X as i16,
            status_y,
            text.as_bytes(),
        )?;
    }

    conn.flush().context("failed to flush draw requests")?;
    Ok(())
}

fn draw_text(
    conn: &RustConnection,
    win: Window,
    gc: Gcontext,
    x: i16,
    y: i16,
    text: &[u8],
) -> Result<()> {
    conn.image_text8(win, gc, x, y, text)
        .context("failed to send text draw request")?
        .check()
        .context("X11 server rejected text draw request")?;
    Ok(())
}
