use std::error::Error;
use std::num::NonZeroU32;
use std::rc::Rc;

use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

const LIGHT_SQUARE: u32 = 0x00e8_e8e8;
const DARK_SQUARE: u32 = 0x0028_2828;
const CHECKER_SIZE: u32 = 32;

type PreviewSurface = Surface<Rc<Window>, Rc<Window>>;

#[derive(Default)]
struct PreviewApp {
    window: Option<Rc<Window>>,
    surface: Option<PreviewSurface>,
    fatal_error: Option<String>,
}

impl PreviewApp {
    fn fail(&mut self, event_loop: &ActiveEventLoop, message: impl Into<String>) {
        let message = message.into();
        eprintln!("paperview: {message}");
        self.fatal_error = Some(message);
        event_loop.exit();
    }

    fn create_surface(&mut self, event_loop: &ActiveEventLoop) {
        let window = match self.window.as_ref() {
            Some(window) => Rc::clone(window),
            None => {
                let attributes = Window::default_attributes()
                    .with_title("PaperView — Stage 0 checkerboard")
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

impl ApplicationHandler for PreviewApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.surface.is_none() {
            self.create_surface(event_loop);
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
}

fn main() -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = PreviewApp::default();
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.fatal_error {
        return Err(error.into());
    }
    Ok(())
}
