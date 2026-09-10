//! X11 rendering adapter for the wire-independent UI layout.

use crate::ui::geometry::{STATUS_BAR_HEIGHT, draw_layout};
use crate::ui::screen::ScreenLayout;
use crate::ui::system;
use anyhow::{Context, Result, ensure};
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
/// Core X11 `ImageText8` encodes its string length in one byte.
const MAX_IMAGE_TEXT8_BYTES: usize = u8::MAX as usize;

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
    let Some(screen_layout) = ScreenLayout::new(width, height) else {
        return Ok(());
    };
    let viewport = screen_layout.remote_viewport;
    let application_layout = draw_layout(viewport.width, viewport.height);
    let system_layout = system::draw_layout(width, height);

    conn.clear_area(false, win, 0, 0, width, height)
        .context("failed to send clear-area request")?
        .check()
        .context("X11 server rejected clear-area request")?;

    let mut rectangles = Vec::new();
    if let Some(layout) = &application_layout {
        rectangles.extend(layout.rectangles.iter().map(|rectangle| Rectangle {
            x: physical_coordinate(viewport.x, rectangle.x),
            y: physical_coordinate(viewport.y, rectangle.y),
            width: rectangle.width,
            height: rectangle.height,
        }));
    }
    if let Some(system_layout) = system_layout {
        rectangles.push(Rectangle {
            x: system_layout.rectangle.x,
            y: system_layout.rectangle.y,
            width: system_layout.rectangle.width,
            height: system_layout.rectangle.height,
        });
    }
    if !rectangles.is_empty() {
        conn.poly_rectangle(win, gc, &rectangles)
            .context("failed to send rectangle draw request")?
            .check()
            .context("X11 server rejected rectangle draw request")?;
    }

    if let Some(layout) = application_layout {
        for placement in layout.text {
            if placement.y > 0 {
                draw_text(
                    conn,
                    win,
                    gc,
                    physical_coordinate(viewport.x, placement.x),
                    physical_coordinate(viewport.y, placement.y),
                    placement.text,
                )?;
            }
        }
    }
    match system_layout {
        Some(system_layout) if system_layout.text.y > 0 => {
            draw_text(
                conn,
                win,
                gc,
                system_layout.text.x,
                system_layout.text.y,
                system_layout.text.text,
            )?;
        }
        Some(_) | None => {}
    }

    if let Some(text) = status_text.filter(|_| viewport.height > 0) {
        // Baseline inside the remote viewport's legacy status strip, above Exit.
        let status_y = status_baseline(screen_layout);
        let encoded = encode_status_text(text);
        draw_text(conn, win, gc, STATUS_TEXT_X as i16, status_y, &encoded)?;
    }

    conn.flush().context("failed to flush draw requests")?;
    Ok(())
}

fn status_baseline(screen_layout: ScreenLayout) -> i16 {
    let viewport = screen_layout.remote_viewport;
    physical_coordinate(
        viewport.y,
        viewport
            .height
            .saturating_sub(STATUS_TEXT_BOTTOM_MARGIN)
            .min(i16::MAX as u16) as i16,
    )
}

fn physical_coordinate(origin: u16, relative: i16) -> i16 {
    (i32::from(origin) + i32::from(relative)).clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn draw_text(
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

/// Convert UTF-8 status text to the printable-ASCII subset supported by the
/// core X11 font, bounded to `ImageText8`'s one-byte length field.
fn encode_status_text(text: &str) -> Vec<u8> {
    text.chars()
        .take(MAX_IMAGE_TEXT8_BYTES)
        .map(|character| {
            if (' '..='~').contains(&character) {
                character as u8
            } else {
                b'?'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_encoding_preserves_printable_ascii() {
        assert_eq!(
            encode_status_text("PaperSpoon: connected!"),
            b"PaperSpoon: connected!"
        );
    }

    #[test]
    fn portrait_status_baseline_stays_inside_remote_viewport() {
        let layout = ScreenLayout::new(1272, 1696).expect("screen layout");

        assert_eq!(status_baseline(layout), 1614);
        assert!(status_baseline(layout) < layout.system_ui_region.y as i16);
    }

    #[test]
    fn status_encoding_replaces_controls_and_non_ascii_scalars() {
        assert_eq!(encode_status_text("ok\té🙂"), b"ok???");
    }

    #[test]
    fn status_encoding_stays_within_image_text8_limit() {
        let encoded = encode_status_text(&"é".repeat(MAX_IMAGE_TEXT8_BYTES + 1));

        assert_eq!(encoded.len(), MAX_IMAGE_TEXT8_BYTES);
        assert!(encoded.iter().all(|byte| *byte == b'?'));
    }
}
