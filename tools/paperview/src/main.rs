mod config;

use std::env;
use std::error::Error;
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::rc::Rc;

use config::{Options, ParseOutcome};
use paperview::receiver::ReceivedFrame;
use paperview::worker::{
    ConnectionStatus, WorkerConfig, WorkerHandle, WorkerUpdates, start_worker,
};
use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

const LIGHT_SQUARE: u32 = 0x00e8_e8e8;
const DARK_SQUARE: u32 = 0x0028_2828;
const CHECKER_SIZE: u32 = 32;

type PreviewSurface = Surface<Rc<Window>, Rc<Window>>;

#[derive(Clone, Copy, Debug)]
enum UserEvent {
    NetworkWake,
}

struct NetworkState {
    target_label: String,
    status: ConnectionStatus,
    last_frame: Option<ReceivedFrame>,
}

impl NetworkState {
    fn new(target_label: String) -> Self {
        Self {
            target_label,
            status: ConnectionStatus::Connecting,
            last_frame: None,
        }
    }

    fn apply(&mut self, updates: WorkerUpdates) {
        if let Some(status) = updates.status {
            self.status = status;
        }
        if let Some(frame) = updates.frame {
            self.last_frame = Some(frame);
        }
    }

    fn title(&self) -> String {
        let status = match self.status {
            ConnectionStatus::Connecting => "connecting",
            ConnectionStatus::Connected => "connected",
            ConnectionStatus::Disconnected => "disconnected",
            ConnectionStatus::Error(_) => "error",
            ConnectionStatus::Stopped => "stopped",
        };
        let frame = self.last_frame.as_ref().map_or_else(String::new, |frame| {
            format!(
                " — received frame {} {:?} (not displayed)",
                frame.frame_id(),
                frame.frame().pixel_format()
            )
        });
        format!(
            "PaperView — {status}{frame} — checkerboard — {}",
            self.target_label
        )
    }
}

struct PreviewApp {
    network_config: WorkerConfig,
    network_proxy: EventLoopProxy<UserEvent>,
    network_worker: Option<WorkerHandle>,
    network_state: NetworkState,
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
                    .with_title(self.network_state.title())
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
        self.network_state.apply(updates);
        if let Some(window) = self.window.as_ref() {
            window.set_title(&self.network_state.title());
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
        for y in 0..height.get() {
            for x in 0..width.get() {
                let square = (x / CHECKER_SIZE) + (y / CHECKER_SIZE);
                buffer[(y * width.get() + x) as usize] = if square.is_multiple_of(2) {
                    LIGHT_SQUARE
                } else {
                    DARK_SQUARE
                };
            }
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
            WindowEvent::Resized(_) => {
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }
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
        state.apply(WorkerUpdates {
            status: Some(ConnectionStatus::Connected),
            frame: Some(received_frame(7)),
        });
        assert_eq!(
            state.last_frame.as_ref().map(ReceivedFrame::frame_id),
            Some(7)
        );
        assert!(state.title().contains("connected"));
        assert!(
            state
                .title()
                .contains("received frame 7 Gray8 (not displayed)")
        );

        state.apply(WorkerUpdates {
            status: Some(ConnectionStatus::Error("connection lost".to_string())),
            frame: None,
        });
        assert_eq!(
            state.last_frame.as_ref().map(ReceivedFrame::frame_id),
            Some(7)
        );
        assert!(state.title().contains("error"));
        assert!(state.title().contains("not displayed"));
    }
}
