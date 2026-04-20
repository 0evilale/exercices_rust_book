use std::num::NonZeroU32;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use softbuffer::{Context, Surface};
use tracing::{debug, error, info, warn};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::types::VideoFrame;

// ── Custom user event ─────────────────────────────────────────────────────────

/// Events sent from the pipeline threads to wake the winit event loop.
#[derive(Debug)]
pub enum PreviewEvent {
    /// A new composed frame is ready — request a redraw.
    NewFrame,
    /// Signal the preview to close.
    Quit,
}

// ── Preview window ────────────────────────────────────────────────────────────

/// Manages the winit window and softbuffer surface for the real-time preview.
///
/// Must live on the main thread (required by winit on macOS and some Wayland
/// compositors).  Frames arrive via an `mpsc::Receiver<VideoFrame>` that is
/// populated by the compositor thread.
pub struct PreviewApp {
    /// Channel from which the latest composed frame is received.
    frame_rx: Receiver<Arc<VideoFrame>>,
    /// The latest frame to display (updated on `NewFrame` events).
    latest_frame: Option<Arc<VideoFrame>>,
    /// winit window, created in `resumed()`.
    window: Option<Arc<Window>>,
    /// softbuffer context, created in `resumed()`.
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    /// Initial window title.
    title: String,
    /// If true, the event loop should exit on next iteration.
    should_quit: bool,
}

impl PreviewApp {
    pub fn new(frame_rx: Receiver<Arc<VideoFrame>>, title: impl Into<String>) -> Self {
        PreviewApp {
            frame_rx,
            latest_frame: None,
            window: None,
            surface: None,
            title: title.into(),
            should_quit: false,
        }
    }
}

impl ApplicationHandler<PreviewEvent> for PreviewApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 720u32));

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                error!("Failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        let ctx = match Context::new(Arc::clone(&window)) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to create softbuffer context: {e}");
                event_loop.exit();
                return;
            }
        };

        let surface = match Surface::new(&ctx, Arc::clone(&window)) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to create softbuffer surface: {e}");
                event_loop.exit();
                return;
            }
        };

        info!("Preview window created");
        self.window = Some(window);
        self.surface = Some(surface);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: PreviewEvent) {
        match event {
            PreviewEvent::NewFrame => {
                // Drain the channel so we always display the newest frame.
                while let Ok(f) = self.frame_rx.try_recv() {
                    self.latest_frame = Some(f);
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            PreviewEvent::Quit => {
                self.should_quit = true;
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                info!("Window close requested");
                event_loop.exit();
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyQ),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                info!("Q pressed — quitting");
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                self.render();
            }
            WindowEvent::Resized(_) => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Nothing to do here; redraws are driven by NewFrame user-events.
    }
}

impl PreviewApp {
    fn render(&mut self) {
        let (Some(window), Some(surface)) = (self.window.as_ref(), self.surface.as_mut()) else {
            return;
        };

        let size = window.inner_size();
        let win_w = size.width;
        let win_h = size.height;

        if win_w == 0 || win_h == 0 {
            return;
        }

        let nw = match NonZeroU32::new(win_w) {
            Some(v) => v,
            None => return,
        };
        let nh = match NonZeroU32::new(win_h) {
            Some(v) => v,
            None => return,
        };

        if let Err(e) = surface.resize(nw, nh) {
            warn!("Surface resize failed: {e}");
            return;
        }

        let mut buf = match surface.buffer_mut() {
            Ok(b) => b,
            Err(e) => {
                warn!("Failed to get surface buffer: {e}");
                return;
            }
        };

        // Fill with black first.
        buf.fill(0xFF_00_00_00u32); // ARGB black with full alpha

        if let Some(frame) = &self.latest_frame {
            blit_frame_to_buffer(frame, &mut buf, win_w, win_h);
        }

        if let Err(e) = buf.present() {
            warn!("Failed to present surface buffer: {e}");
        }

        debug!("rendered frame to {}×{} window", win_w, win_h);
    }
}

/// Scale and blit a BGRA32 VideoFrame into the softbuffer pixel buffer.
///
/// softbuffer expects 0xAARRGGBB (native-endian u32) values.
fn blit_frame_to_buffer(
    frame: &VideoFrame,
    buf: &mut softbuffer::Buffer<'_, Arc<Window>, Arc<Window>>,
    win_w: u32,
    win_h: u32,
) {
    let fw = frame.width as usize;
    let fh = frame.height as usize;
    let ww = win_w as usize;
    let wh = win_h as usize;

    if fw == 0 || fh == 0 {
        return;
    }

    let src = frame.data.as_ref();

    for py in 0..wh {
        // Nearest-neighbour mapping from window row → frame row
        let fy = (py * fh) / wh;
        let fy = fy.min(fh - 1);

        for px in 0..ww {
            let fx = (px * fw) / ww;
            let fx = fx.min(fw - 1);

            let src_off = (fy * fw + fx) * 4;
            let (b, g, r) = match frame.format {
                crate::types::PixelFormat::BGRA32 => {
                    (src[src_off], src[src_off + 1], src[src_off + 2])
                }
                crate::types::PixelFormat::RGBA32 => {
                    (src[src_off + 2], src[src_off + 1], src[src_off])
                }
                crate::types::PixelFormat::I420 => (0, 0, 0),
            };

            // softbuffer format: 0x00_RR_GG_BB stored as u32
            buf[py * ww + px] = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
        }
    }
}

// ── Public helper ─────────────────────────────────────────────────────────────

/// Create an `EventLoop` and return it together with a `EventLoopProxy` that
/// the pipeline can use to send `PreviewEvent`s from other threads.
pub fn create_event_loop() -> (EventLoop<PreviewEvent>, EventLoopProxy<PreviewEvent>) {
    let event_loop = EventLoop::<PreviewEvent>::with_user_event()
        .build()
        .expect("Failed to create winit event loop");
    let proxy = event_loop.create_proxy();
    (event_loop, proxy)
}
