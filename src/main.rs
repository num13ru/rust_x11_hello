use anyhow::{Context, Result};
use std::env;
use std::thread;
use std::time::Duration;
use x11rb::COPY_DEPTH_FROM_PARENT;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, Gcontext, Rectangle, WindowClass,
};
use x11rb::rust_connection::RustConnection;

fn main() -> Result<()> {
    println!("Rust X11 hello for Kindle");
    println!("target_arch: {}", env::consts::ARCH);
    println!("target_os: {}", env::consts::OS);
    println!("DISPLAY={:?}", env::var("DISPLAY").ok());
    println!("XAUTHORITY={:?}", env::var("XAUTHORITY").ok());

    let (conn, screen_num) = RustConnection::connect(None)
        .context("failed to connect to X11 display; check DISPLAY and /tmp/.X11-unix/X0")?;

    let screen = &conn.setup().roots[screen_num];

    println!("connected to X11");
    println!("screen_num: {screen_num}");
    println!("root: 0x{:x}", screen.root);
    println!("width: {}", screen.width_in_pixels);
    println!("height: {}", screen.height_in_pixels);
    println!("root_depth: {}", screen.root_depth);
    println!("black_pixel: {}", screen.black_pixel);
    println!("white_pixel: {}", screen.white_pixel);

    let win = conn.generate_id().context("failed to generate window id")?;

    let width: u16 = 760;
    let height: u16 = 360;
    let x: i16 = 80;
    let y: i16 = 120;

    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        x,
        y,
        width,
        height,
        2,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .background_pixel(screen.white_pixel)
            .border_pixel(screen.black_pixel)
            .override_redirect(1)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY),
    )
    .context("failed to create X11 window")?;

    let gc: Gcontext = conn.generate_id().context("failed to generate gc id")?;

    conn.create_gc(
        gc,
        win,
        &CreateGCAux::new()
            .foreground(screen.black_pixel)
            .background(screen.white_pixel),
    )
    .context("failed to create graphics context")?;

    conn.map_window(win).context("failed to map X11 window")?;
    conn.flush().context("failed to flush map_window")?;

    println!("window mapped: 0x{win:x}");

    // Give the server a moment to map/expose the window.
    thread::sleep(Duration::from_millis(300));

    draw(&conn, win, gc)?;

    println!("drawn; sleeping 5 seconds");
    thread::sleep(Duration::from_secs(5));

    conn.free_gc(gc).ok();
    conn.destroy_window(win).ok();
    conn.flush().ok();

    println!("window destroyed; done");

    Ok(())
}

fn draw(conn: &RustConnection, win: u32, gc: Gcontext) -> Result<()> {
    conn.poly_rectangle(
        win,
        gc,
        &[
            Rectangle {
                x: 20,
                y: 20,
                width: 720,
                height: 320,
            },
            Rectangle {
                x: 40,
                y: 40,
                width: 680,
                height: 280,
            },
        ],
    )
    .context("failed to draw rectangles")?;

    conn.image_text8(win, gc, 70, 110, b"Rust X11 Hello on Kindle")
        .context("failed to draw title text")?;

    conn.image_text8(win, gc, 70, 160, b"DISPLAY=:0.0")
        .context("failed to draw display text")?;

    conn.image_text8(win, gc, 70, 210, b"Static ARMv7 musl + x11rb")
        .context("failed to draw build text")?;

    conn.image_text8(win, gc, 70, 260, b"Auto-exit in 5 seconds")
        .context("failed to draw exit text")?;

    conn.flush().context("failed to flush draw")?;

    Ok(())
}
