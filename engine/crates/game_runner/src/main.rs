use std::sync::Arc;

use engine_core::window::{AppLifecycle, has_renderable_size};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

struct GameApp {
    window: Option<Arc<Window>>,
    lifecycle: AppLifecycle,
    occluded: bool,
}

impl GameApp {
    fn new() -> Self {
        Self {
            window: None,
            lifecycle: AppLifecycle::Initializing,
            occluded: false,
        }
    }

    fn transition(&mut self, next: AppLifecycle) {
        if let Err(error) = self.lifecycle.transition(next) {
            eprintln!("[lifecycle] {error}");
            return;
        }
        println!("[lifecycle] {:?}", self.lifecycle);
    }

    fn update_pause_state(&mut self, size: PhysicalSize<u32>) {
        let should_pause = self.occluded || !has_renderable_size(size.width, size.height);
        match (should_pause, self.lifecycle) {
            (true, AppLifecycle::Running) => self.transition(AppLifecycle::Paused),
            (false, AppLifecycle::Paused) => self.transition(AppLifecycle::Running),
            _ => {}
        }
        if should_pause {
            println!(
                "[window] simulation paused: size={}x{} occluded={}",
                size.width, size.height, self.occluded
            );
        } else {
            println!("[window] renderable size: {}x{}", size.width, size.height);
        }
    }
}

impl ApplicationHandler for GameApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("P2 RustLua Engine - M1-01 Window")
            .with_inner_size(PhysicalSize::new(1280, 720));
        let window = Arc::new(event_loop.create_window(attributes).expect("create window"));
        println!("[window] created: {:?}", window.id());
        self.window = Some(window);
        self.transition(AppLifecycle::Running);
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
            WindowEvent::CloseRequested => {
                println!("[window] close requested");
                self.transition(AppLifecycle::Exiting);
                event_loop.exit();
                self.transition(AppLifecycle::Exited);
            }
            WindowEvent::Resized(size) => self.update_pause_state(size),
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                let size = self.window.as_ref().expect("window exists").inner_size();
                self.update_pause_state(size);
            }
            WindowEvent::RedrawRequested => {
                if self.lifecycle.is_simulation_active() {
                    println!("[window] redraw");
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && event.logical_key
                        == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) =>
            {
                println!("[window] Escape pressed; requesting close");
                self.transition(AppLifecycle::Exiting);
                event_loop.exit();
                self.transition(AppLifecycle::Exited);
            }
            _ => {}
        }
    }
}

fn main() -> Result<(), winit::error::EventLoopError> {
    println!("p2 game runner {}", engine_core::version());
    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut GameApp::new())
}
