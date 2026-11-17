mod config;

use std::env;
use std::error::Error;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use config::{Options, ParseOutcome};
use paperview::export::{export_source_png, source_png_filename};
use paperview::framebuffer::{ConversionError, PresentationBuffer, convert_frame};
use paperview::receiver::ReceivedFrame;
use paperview::viewport::{
    LETTERBOX_RGB, PAN_STEP_SOURCE_PIXELS, ViewMode, ViewportState, ZoomLevel, render_view,
};
use paperview::worker::{
    ConnectionStatus, WorkerConfig, WorkerHandle, WorkerUpdates, start_worker,
};
use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

type PreviewSurface = Surface<Rc<Window>, Rc<Window>>;

#[derive(Clone, Copy, Debug)]
enum UserEvent {
    NetworkWake,
}

struct NetworkState {
    target_label: String,
    status: ConnectionStatus,
    last_frame: Option<ReceivedFrame>,
    presentation: Option<PresentationBuffer>,
}

impl NetworkState {
    fn new(target_label: String) -> Self {
        Self {
            target_label,
            status: ConnectionStatus::Connecting,
            last_frame: None,
            presentation: None,
        }
    }

    fn apply(&mut self, updates: WorkerUpdates) -> Result<bool, ConversionError> {
        if let Some(status) = updates.status {
            self.status = status;
        }
        if let Some(frame) = updates.frame {
            if let Some(presentation) = self.presentation.as_mut() {
                presentation.update_from_frame(frame.frame())?;
            } else {
                self.presentation = Some(convert_frame(frame.frame())?);
            }
            self.last_frame = Some(frame);
            return Ok(true);
        }
        Ok(false)
    }

    fn title(&self, view_label: &str) -> String {
        let status = match self.status {
            ConnectionStatus::Connecting => "connecting",
            ConnectionStatus::Connected => "connected",
            ConnectionStatus::Disconnected => "disconnected",
            ConnectionStatus::Error(_) => "error",
            ConnectionStatus::Stopped => "stopped",
        };
        let frame = self.last_frame.as_ref().map_or_else(String::new, |frame| {
            format!(
                " — frame {} {:?}",
                frame.frame_id(),
                frame.frame().pixel_format()
            )
        });
        format!(
            "PaperView — {status}{frame} — view {view_label} — {}",
            self.target_label
        )
    }
}

struct PreviewApp {
    network_config: WorkerConfig,
    network_proxy: EventLoopProxy<UserEvent>,
    network_worker: Option<WorkerHandle>,
    network_state: NetworkState,
    viewport: ViewportState,
    save_dir: Option<PathBuf>,
    last_logged_connection_error: Option<String>,
    window: Option<Rc<Window>>,
    surface: Option<PreviewSurface>,
    fatal_error: Option<String>,
}

impl PreviewApp {
    fn new(options: &Options, network_proxy: EventLoopProxy<UserEvent>) -> Self {
        Self {
            network_config: WorkerConfig::new(
                options.host.clone(),
                options.port,
                options.viewport_width,
                options.viewport_height,
            ),
            network_proxy,
            network_worker: None,
            network_state: NetworkState::new(options.target_label()),
            viewport: ViewportState::default(),
            save_dir: options.save_dir.clone(),
            last_logged_connection_error: None,
            window: None,
            surface: None,
            fatal_error: None,
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, message: impl Into<String>) {
        let message = message.into();
        self.fatal_error = Some(message);
        event_loop.exit();
    }

    fn create_surface(&mut self, event_loop: &ActiveEventLoop) {
        let window = match self.window.as_ref() {
            Some(window) => Rc::clone(window),
            None => {
                let attributes = Window::default_attributes()
                    .with_title(self.network_state.title(&self.viewport.label()))
                    .with_inner_size(LogicalSize::new(640.0, 480.0));
                match event_loop.create_window(attributes) {
                    Ok(window) => {
                        let window = Rc::new(window);
                        self.window = Some(Rc::clone(&window));
                        window
                    }
                    Err(error) => {
                        self.fail(event_loop, format!("could not create window: {error}"));
                        return;
                    }
                }
            }
        };

        let context = match Context::new(Rc::clone(&window)) {
            Ok(context) => context,
            Err(error) => {
                self.fail(
                    event_loop,
                    format!("could not create software display context: {error}"),
                );
                return;
            }
        };
        self.surface = match Surface::new(&context, Rc::clone(&window)) {
            Ok(surface) => Some(surface),
            Err(error) => {
                self.fail(
                    event_loop,
                    format!("could not create software window surface: {error}"),
                );
                return;
            }
        };
        window.focus_window();
        window.request_redraw();
    }

    fn start_network(&mut self, event_loop: &ActiveEventLoop) {
        let proxy = self.network_proxy.clone();
        match start_worker(self.network_config.clone(), move || {
            proxy.send_event(UserEvent::NetworkWake).is_ok()
        }) {
            Ok(worker) => self.network_worker = Some(worker),
            Err(error) => self.fail(
                event_loop,
                format!("could not start network worker: {error}"),
            ),
        }
    }

    fn apply_network_updates(&mut self) {
        let Some(worker) = self.network_worker.as_ref() else {
            return;
        };
        let updates = worker.take_updates();
        let previous_dimensions = self
            .network_state
            .presentation
            .as_ref()
            .map(|frame| (u32::from(frame.width()), u32::from(frame.height())));
        match updates.status.as_ref() {
            Some(ConnectionStatus::Connected) => self.last_logged_connection_error = None,
            Some(ConnectionStatus::Error(error))
                if self.last_logged_connection_error.as_deref() != Some(error) =>
            {
                eprintln!("paperview: connection error: {error}");
                self.last_logged_connection_error = Some(error.clone());
            }
            _ => {}
        }
        let frame_changed = match self.network_state.apply(updates) {
            Ok(frame_changed) => frame_changed,
            Err(error) => {
                eprintln!("paperview: could not convert received frame: {error}");
                false
            }
        };
        if frame_changed {
            let (source_width, source_height, target_width, target_height) = self.view_geometry();
            if previous_dimensions != Some((source_width, source_height)) {
                self.viewport
                    .center(source_width, source_height, target_width, target_height);
            } else {
                self.viewport
                    .clamp(source_width, source_height, target_width, target_height);
            }
        }
        if let Some(window) = self.window.as_ref() {
            window.set_title(&self.network_state.title(&self.viewport.label()));
            if frame_changed {
                window.request_redraw();
            }
        }
    }

    fn view_geometry(&self) -> (u32, u32, u32, u32) {
        let (source_width, source_height) = self
            .network_state
            .presentation
            .as_ref()
            .map_or((0, 0), |frame| {
                (u32::from(frame.width()), u32::from(frame.height()))
            });
        let (target_width, target_height) = self.window.as_ref().map_or((0, 0), |window| {
            let size = window.inner_size();
            (size.width, size.height)
        });
        (source_width, source_height, target_width, target_height)
    }

    fn handle_key(&mut self, event: &KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        let PhysicalKey::Code(key) = event.physical_key else {
            return;
        };
        if key == KeyCode::KeyS {
            if !event.repeat {
                self.save_source_frame();
            }
            return;
        }
        let (source_width, source_height, target_width, target_height) = self.view_geometry();
        let changed = match key {
            KeyCode::KeyF => self.viewport.select(
                ViewMode::Fit,
                source_width,
                source_height,
                target_width,
                target_height,
            ),
            KeyCode::Digit1 => self.viewport.select(
                ViewMode::Zoom(ZoomLevel::X1),
                source_width,
                source_height,
                target_width,
                target_height,
            ),
            KeyCode::Digit2 => self.viewport.select(
                ViewMode::Zoom(ZoomLevel::X2),
                source_width,
                source_height,
                target_width,
                target_height,
            ),
            KeyCode::Digit4 => self.viewport.select(
                ViewMode::Zoom(ZoomLevel::X4),
                source_width,
                source_height,
                target_width,
                target_height,
            ),
            KeyCode::Home => {
                self.viewport
                    .center(source_width, source_height, target_width, target_height)
            }
            KeyCode::ArrowLeft => self.viewport.pan_by(
                -PAN_STEP_SOURCE_PIXELS,
                0,
                source_width,
                source_height,
                target_width,
                target_height,
            ),
            KeyCode::ArrowRight => self.viewport.pan_by(
                PAN_STEP_SOURCE_PIXELS,
                0,
                source_width,
                source_height,
                target_width,
                target_height,
            ),
            KeyCode::ArrowUp => self.viewport.pan_by(
                0,
                -PAN_STEP_SOURCE_PIXELS,
                source_width,
                source_height,
                target_width,
                target_height,
            ),
            KeyCode::ArrowDown => self.viewport.pan_by(
                0,
                PAN_STEP_SOURCE_PIXELS,
                source_width,
                source_height,
                target_width,
                target_height,
            ),
            _ => false,
        };
        if changed && let Some(window) = self.window.as_ref() {
            window.set_title(&self.network_state.title(&self.viewport.label()));
            window.request_redraw();
        }
    }

    fn save_source_frame(&self) {
        let Some(directory) = self.save_dir.as_ref() else {
            eprintln!("paperview: source PNG export is disabled; restart with --save-dir DIR");
            return;
        };
        let Some(frame) = self.network_state.last_frame.as_ref() else {
            eprintln!("paperview: cannot save source PNG before a valid frame is received");
            return;
        };
        let path = directory.join(source_png_filename(frame.frame_id(), frame.frame()));
        match export_source_png(frame.frame(), &path) {
            Ok(()) => eprintln!(
                "paperview: saved source frame {} to {}",
                frame.frame_id(),
                path.display()
            ),
            Err(error) => eprintln!(
                "paperview: could not save source frame {}: {error}",
                frame.frame_id()
            ),
        }
    }

    fn handle_view_resize(&mut self, target_width: u32, target_height: u32) {
        let (source_width, source_height) = self
            .network_state
            .presentation
            .as_ref()
            .map_or((0, 0), |frame| {
                (u32::from(frame.width()), u32::from(frame.height()))
            });
        if target_width != 0 && target_height != 0 {
            self.viewport
                .clamp(source_width, source_height, target_width, target_height);
        }
        if let Some(window) = self.window.as_ref() {
            window.set_title(&self.network_state.title(&self.viewport.label()));
            window.request_redraw();
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(surface)) = (self.window.as_ref(), self.surface.as_mut()) else {
            return;
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };

        if let Err(error) = surface.resize(width, height) {
            self.fail(event_loop, format!("could not resize surface: {error}"));
            return;
        }
        let mut buffer = match surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(error) => {
                self.fail(
                    event_loop,
                    format!("could not access surface buffer: {error}"),
                );
                return;
            }
        };
        if let Some(presentation) = self.network_state.presentation.as_ref() {
            if let Err(error) = render_view(
                self.viewport,
                presentation,
                width.get(),
                height.get(),
                &mut buffer,
            ) {
                self.fail(event_loop, format!("could not render frame: {error}"));
                return;
            }
        } else {
            buffer.fill(LETTERBOX_RGB);
        }
        if let Err(error) = buffer.present() {
            self.fail(
                event_loop,
                format!("could not present surface buffer: {error}"),
            );
        }
    }
}

impl ApplicationHandler<UserEvent> for PreviewApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.surface.is_none() {
            self.create_surface(event_loop);
        }
        if self.surface.is_some() && self.network_worker.is_none() {
            self.start_network(event_loop);
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::NetworkWake => self.apply_network_updates(),
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.surface = None;
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self
            .window
            .as_ref()
            .is_none_or(|window| window.id() != window_id)
        {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::Resized(size) => self.handle_view_resize(size.width, size.height),
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = self
                    .window
                    .as_ref()
                    .map(|window| window.inner_size())
                    .unwrap_or_default();
                self.handle_view_resize(size.width, size.height);
            }
            WindowEvent::KeyboardInput { event, .. } => self.handle_key(&event),
            _ => {}
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(worker) = self.network_worker.take() {
            worker.shutdown();
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let options = match config::parse(env::args_os()) {
        Ok(ParseOutcome::Run(options)) => options,
        Ok(ParseOutcome::Help) => {
            println!("{}", config::USAGE);
            return Ok(());
        }
        Err(error) => return Err(format!("{error}\n\n{}", config::USAGE).into()),
    };
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let network_proxy = event_loop.create_proxy();

    let mut app = PreviewApp::new(&options, network_proxy);
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.fatal_error {
        return Err(error.into());
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("paperview: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use paper_protocol::{Gray8Frame, encode_v2_frame};
    use paperview::receiver::read_next_frame;

    use super::*;

    fn received_frame(frame_id: u64) -> ReceivedFrame {
        let frame = Gray8Frame::new(2, 2, vec![128; 4]).expect("valid Gray8 frame");
        let encoded = encode_v2_frame(frame_id, &frame).expect("encode frame");
        read_next_frame(&mut Cursor::new(encoded), 2, 2)
            .expect("read frame")
            .expect("frame present")
    }

    #[test]
    fn network_state_retains_last_frame_across_errors_without_claiming_display() {
        let mut state = NetworkState::new("test endpoint".to_string());
        assert!(
            state
                .apply(WorkerUpdates {
                    status: Some(ConnectionStatus::Connected),
                    frame: Some(received_frame(7)),
                })
                .expect("convert frame")
        );
        assert_eq!(
            state.last_frame.as_ref().map(ReceivedFrame::frame_id),
            Some(7)
        );
        assert!(state.title("fit").contains("connected"));
        assert!(state.title("fit").contains("frame 7 Gray8"));
        assert!(state.presentation.is_some());

        assert!(
            state
                .apply(WorkerUpdates {
                    status: None,
                    frame: Some(received_frame(8)),
                })
                .expect("reuse presentation buffer")
        );
        assert_eq!(
            state.last_frame.as_ref().map(ReceivedFrame::frame_id),
            Some(8)
        );

        assert!(
            !state
                .apply(WorkerUpdates {
                    status: Some(ConnectionStatus::Error("connection lost".to_string())),
                    frame: None,
                })
                .expect("status-only update")
        );
        assert_eq!(
            state.last_frame.as_ref().map(ReceivedFrame::frame_id),
            Some(8)
        );
        assert!(state.title("fit").contains("error"));
        assert!(state.presentation.is_some());
    }
}
