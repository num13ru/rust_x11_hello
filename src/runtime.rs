//! X11-independent PaperPad application runtime.

use crate::app::{Activation, AppState, GeometryUpdate, Redraw, RemotePointerEvent};
use crate::display::{DisplayBackend, RedrawCause, RemoteFrame};
use crate::latency::{self, Stamp};
use crate::net::Paperspoon;
use crate::ui::button::{PointerEvent, PointerEventKind};
use crate::ui::screen::ScreenLayout;
use anyhow::{Context, Result};
use paper_protocol::V2PointerPhase;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeControl {
    Continue,
    ShutdownRequested,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GeometryOutcome {
    IgnoredZero,
    Unchanged,
    Redrawn,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RuntimePointerInput {
    event: PointerEvent,
    received: Stamp,
    source_time: Option<u64>,
}

impl RuntimePointerInput {
    pub(crate) fn new(event: PointerEvent, received: Stamp, source_time: Option<u64>) -> Self {
        Self {
            event,
            received,
            source_time,
        }
    }
}

pub(crate) struct PaperpadRuntime {
    app: AppState,
    displayed_connection: Option<bool>,
}

impl PaperpadRuntime {
    pub(crate) fn new(dimensions: (u16, u16)) -> Self {
        Self {
            app: AppState::new(dimensions),
            displayed_connection: None,
        }
    }

    pub(crate) fn service(
        &mut self,
        display: &mut dyn DisplayBackend,
        paperspoon: &mut Paperspoon,
    ) -> Result<()> {
        let connected = paperspoon.is_connected();
        if connection_indicator_changed(&mut self.displayed_connection, connected) {
            display
                .draw_system_ui(connected)
                .context("failed to draw connection state")?;
            eprintln!("ui connection indicator connected={connected}");
        }

        if let Some(frame) = paperspoon.poll_frame() {
            match RemoteFrame::new(
                frame.frame_id(),
                frame.width(),
                frame.height(),
                frame.pixel_format(),
                frame.stride(),
                frame.pixels(),
                frame.receive_decode_us(),
            ) {
                Ok(frame) => {
                    let frame_id = frame.frame_id();
                    latency::record(
                        "frame_present_start",
                        None,
                        &format!(",\"frame_id\":{frame_id}"),
                    );
                    display.display_remote_frame(frame);
                    latency::record(
                        "frame_present_return",
                        None,
                        &format!(",\"frame_id\":{frame_id}"),
                    );
                }
                Err(error) => eprintln!(
                    "frame validation error id={} width={} height={}: {error}",
                    frame.frame_id(),
                    frame.width(),
                    frame.height()
                ),
            }
        }

        display
            .maintain()
            .context("failed to maintain display backend")
    }

    pub(crate) fn redraw_surface(
        &self,
        display: &mut dyn DisplayBackend,
        cause: RedrawCause,
        connected: bool,
    ) -> Result<()> {
        draw_surface(display, self.app.redraw(), cause, connected)
    }

    pub(crate) fn update_geometry(
        &mut self,
        reported: (u16, u16),
        display: &mut dyn DisplayBackend,
        paperspoon: &mut Paperspoon,
    ) -> Result<GeometryOutcome> {
        match self.app.update_geometry(reported) {
            GeometryUpdate::IgnoredZero => Ok(GeometryOutcome::IgnoredZero),
            GeometryUpdate::Unchanged => Ok(GeometryOutcome::Unchanged),
            GeometryUpdate::Redraw(redraw) => {
                let (width, height) = redraw.size();
                let viewport = remote_viewport_size((width, height));
                if let Err(error) = paperspoon.send_viewport_changed(viewport) {
                    eprintln!("transport error sending viewport change: {error:#}");
                }
                if let Some(frame) = display.set_dimensions((width, height))? {
                    eprintln!(
                        "frame cache invalidated id={} width={} height={} format={:?} stride={} viewport_width={} viewport_height={}",
                        frame.frame_id(),
                        frame.dimensions().0,
                        frame.dimensions().1,
                        frame.pixel_format(),
                        frame.stride(),
                        viewport.0,
                        viewport.1
                    );
                }
                draw_surface(
                    display,
                    redraw,
                    RedrawCause::GeometryChange,
                    paperspoon.is_connected(),
                )?;
                Ok(GeometryOutcome::Redrawn)
            }
        }
    }

    pub(crate) fn handle_pointer(
        &mut self,
        input: RuntimePointerInput,
        paperspoon: &mut Paperspoon,
    ) -> RuntimeControl {
        let event = input.event;
        let outcome = self.app.handle_pointer(event);
        if let Some(pointer) = outcome.remote() {
            let trace = latency::new_trace(input.received);
            let phase = pointer_phase(pointer.kind());
            let source_time = input
                .source_time
                .map(|time| format!(",\"source_time\":{time}"))
                .unwrap_or_default();
            latency::record_at(
                input.received,
                "input_received",
                Some(trace),
                &format!(
                    ",\"phase\":\"{}\",\"physical_x\":{},\"physical_y\":{}{}",
                    phase_label(phase),
                    event.point.x,
                    event.point.y,
                    source_time
                ),
            );
            forward_remote_pointer(pointer, paperspoon, trace);
        }

        control_for_activation(outcome.activation())
    }

    pub(crate) fn cancel_contact(&mut self) {
        self.app.cancel_contact();
    }
}

pub(crate) fn remote_viewport_size(physical_size: (u16, u16)) -> (u16, u16) {
    ScreenLayout::new(physical_size.0, physical_size.1)
        .map(|layout| (layout.remote_viewport.width, layout.remote_viewport.height))
        .unwrap_or((0, 0))
}

fn connection_indicator_changed(displayed: &mut Option<bool>, connected: bool) -> bool {
    if *displayed == Some(connected) {
        return false;
    }
    *displayed = Some(connected);
    true
}

fn control_for_activation(activation: Option<Activation>) -> RuntimeControl {
    match activation {
        Some(Activation::Exit) => {
            eprintln!("ui action=activate system=exit");
            RuntimeControl::ShutdownRequested
        }
        None => RuntimeControl::Continue,
    }
}

fn draw_surface(
    display: &mut dyn DisplayBackend,
    redraw: Redraw,
    cause: RedrawCause,
    connected: bool,
) -> Result<()> {
    debug_assert_eq!(display.dimensions(), redraw.size());
    display.draw_system_ui(connected)?;
    display.redraw_cached_frame(cause);
    Ok(())
}

fn forward_remote_pointer(
    pointer: RemotePointerEvent,
    paperspoon: &mut Paperspoon,
    trace: paper_protocol::V2PointerTrace,
) {
    let phase = pointer_phase(pointer.kind());
    let point = pointer.point();
    // Capture before handing the message to the writer thread. The writer can
    // consume a successful try_send before this thread resumes, so taking the
    // stamp afterward would make measured queue delay negative.
    let queued = latency::stamp();
    if let Err(error) = paperspoon.send_traced_pointer(phase, point.x, point.y, trace) {
        latency::record(
            "pointer_queue_error",
            Some(trace),
            &format!(
                ",\"phase\":\"{}\",\"x\":{},\"y\":{}",
                phase_label(phase),
                point.x,
                point.y
            ),
        );
        eprintln!(
            "pointer transport error phase={phase:?} x={} y={}: {error:#}",
            point.x, point.y
        );
    } else {
        latency::record_at(
            queued,
            "pointer_queued",
            Some(trace),
            &format!(
                ",\"phase\":\"{}\",\"x\":{},\"y\":{}",
                phase_label(phase),
                point.x,
                point.y
            ),
        );
        eprintln!(
            "remote pointer queued phase={phase:?} x={} y={}",
            point.x, point.y
        );
    }
}

fn pointer_phase(kind: PointerEventKind) -> V2PointerPhase {
    match kind {
        PointerEventKind::Press => V2PointerPhase::Down,
        PointerEventKind::Release => V2PointerPhase::Up,
    }
}

fn phase_label(phase: V2PointerPhase) -> &'static str {
    match phase {
        V2PointerPhase::Down => "down",
        V2PointerPhase::Up => "up",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_indicator_redraws_initially_and_only_on_transitions() {
        let mut displayed = None;
        assert!(connection_indicator_changed(&mut displayed, false));
        assert!(!connection_indicator_changed(&mut displayed, false));
        assert!(connection_indicator_changed(&mut displayed, true));
        assert!(!connection_indicator_changed(&mut displayed, true));
        assert!(connection_indicator_changed(&mut displayed, false));
    }

    #[test]
    fn remote_viewport_excludes_local_system_ui() {
        assert_eq!(remote_viewport_size((1272, 1696)), (1272, 1624));
        assert_eq!(remote_viewport_size((100, 40)), (100, 0));
        assert_eq!(remote_viewport_size((0, 40)), (0, 0));
    }

    #[test]
    fn local_exit_requests_explicit_runtime_shutdown() {
        assert_eq!(
            control_for_activation(Some(Activation::Exit)),
            RuntimeControl::ShutdownRequested
        );
        assert_eq!(control_for_activation(None), RuntimeControl::Continue);
    }
}
