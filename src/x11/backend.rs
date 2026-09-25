//! X11 implementation of the backend-neutral display operations.

use super::framebuffer::{PreparedBitmap, X11BitmapAdapter};
use super::render::draw;
use crate::display::{
    CachedFrameMetadata, DisplayBackend, RedrawCause, RemoteFrame, RemoteFrameCache,
};
use crate::ui::screen::ScreenLayout;
use anyhow::Result;
use std::time::Instant;
use x11rb::protocol::xproto::{Gcontext, Window};
use x11rb::rust_connection::RustConnection;

pub(crate) struct X11DisplayBackend<'a> {
    conn: &'a RustConnection,
    win: Window,
    gc: Gcontext,
    dimensions: (u16, u16),
    framebuffer_adapter: Option<X11BitmapAdapter>,
    remote_frame_cache: RemoteFrameCache<PreparedBitmap>,
}

impl<'a> X11DisplayBackend<'a> {
    pub(crate) fn new(
        conn: &'a RustConnection,
        win: Window,
        gc: Gcontext,
        dimensions: (u16, u16),
    ) -> Self {
        eprintln!("display backend=x11");
        let framebuffer_adapter = match X11BitmapAdapter::from_connection(conn) {
            Ok(adapter) => {
                eprintln!("framebuffer adapter ready {adapter}");
                Some(adapter)
            }
            Err(error) => {
                eprintln!("framebuffer adapter unavailable: {error:#}");
                None
            }
        };

        Self {
            conn,
            win,
            gc,
            dimensions,
            framebuffer_adapter,
            remote_frame_cache: RemoteFrameCache::default(),
        }
    }
}

impl DisplayBackend for X11DisplayBackend<'_> {
    fn dimensions(&self) -> (u16, u16) {
        self.dimensions
    }

    fn set_dimensions(&mut self, dimensions: (u16, u16)) -> Option<CachedFrameMetadata> {
        self.dimensions = dimensions;
        let viewport = remote_viewport_size(dimensions);
        self.remote_frame_cache
            .invalidate_mismatched(viewport)
            .map(|frame| CachedFrameMetadata::new(frame.frame_id(), frame.dimensions()))
    }

    fn display_remote_frame(&mut self, frame: RemoteFrame<'_>) {
        let Some(adapter) = self.framebuffer_adapter.as_ref() else {
            eprintln!(
                "frame accepted id={} width={} height={} stride={} bytes={} receive_decode_us={} render=unavailable",
                frame.frame_id(),
                frame.width(),
                frame.height(),
                frame.stride(),
                frame.pixels().len(),
                frame.receive_decode_us()
            );
            return;
        };

        let receive_decode_us = frame.receive_decode_us();
        let prepare_started = Instant::now();
        let prepared = adapter.prepare(frame.width(), frame.height(), frame.pixels());
        let prepare_us = prepare_started.elapsed().as_micros();
        match prepared {
            Ok(bitmap) => {
                // Checked PutImage requests and flush are included in upload time.
                let upload_started = Instant::now();
                let uploaded = adapter.blit(
                    self.conn,
                    self.win,
                    self.gc,
                    remote_viewport_origin(self.dimensions),
                    &bitmap,
                );
                let x11_upload_us = upload_started.elapsed().as_micros();
                match uploaded {
                    Ok(chunks) => {
                        eprintln!(
                            "frame uploaded id={} width={} height={} wire_stride={} wire_bytes={} x11_stride={} x11_bytes={} chunks={} receive_decode_us={receive_decode_us} prepare_us={prepare_us} x11_upload_us={x11_upload_us} cache=updated",
                            frame.frame_id(),
                            frame.width(),
                            frame.height(),
                            frame.stride(),
                            frame.pixels().len(),
                            bitmap.stride(),
                            bitmap.bytes().len(),
                            chunks
                        );
                        self.remote_frame_cache.replace(
                            frame.frame_id(),
                            frame.width(),
                            frame.height(),
                            bitmap,
                        );
                    }
                    Err(error) => eprintln!(
                        "frame upload error id={} width={} height={} receive_decode_us={receive_decode_us} prepare_us={prepare_us} x11_upload_us={x11_upload_us}: {error:#}",
                        frame.frame_id(),
                        frame.width(),
                        frame.height()
                    ),
                }
            }
            Err(error) => eprintln!(
                "frame prepare error id={} width={} height={} receive_decode_us={receive_decode_us} prepare_us={prepare_us}: {error:#}",
                frame.frame_id(),
                frame.width(),
                frame.height()
            ),
        }
    }

    fn redraw_cached_frame(&self, cause: RedrawCause) {
        let (Some(adapter), Some(frame)) = (
            self.framebuffer_adapter.as_ref(),
            self.remote_frame_cache.current(),
        ) else {
            return;
        };
        let cause = match cause {
            RedrawCause::SurfaceDamage => "Expose",
            RedrawCause::GeometryChange => "ConfigureNotify",
        };
        match adapter.blit(
            self.conn,
            self.win,
            self.gc,
            remote_viewport_origin(self.dimensions),
            frame.payload(),
        ) {
            Ok(chunks) => eprintln!(
                "frame redrawn id={} width={} height={} chunks={} cause={} cache=hit",
                frame.frame_id(),
                frame.dimensions().0,
                frame.dimensions().1,
                chunks,
                cause
            ),
            Err(error) => eprintln!(
                "frame redraw error id={} width={} height={} cause={}: {error:#}",
                frame.frame_id(),
                frame.dimensions().0,
                frame.dimensions().1,
                cause
            ),
        }
    }

    fn draw_system_ui(&self) -> Result<()> {
        draw(
            self.conn,
            self.win,
            self.gc,
            self.dimensions.0,
            self.dimensions.1,
        )
    }
}

fn remote_viewport_size(physical_size: (u16, u16)) -> (u16, u16) {
    ScreenLayout::new(physical_size.0, physical_size.1)
        .map(|layout| (layout.remote_viewport.width, layout.remote_viewport.height))
        .unwrap_or((0, 0))
}

fn remote_viewport_origin(physical_size: (u16, u16)) -> (i16, i16) {
    ScreenLayout::new(physical_size.0, physical_size.1)
        .map(|layout| {
            (
                layout.remote_viewport.x as i16,
                layout.remote_viewport.y as i16,
            )
        })
        .unwrap_or((0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_geometry_excludes_local_system_ui() {
        assert_eq!(remote_viewport_size((1272, 1696)), (1272, 1624));
        assert_eq!(remote_viewport_size((100, 40)), (100, 0));
        assert_eq!(remote_viewport_size((0, 40)), (0, 0));
        assert_eq!(remote_viewport_origin((1272, 1696)), (0, 0));
    }
}
