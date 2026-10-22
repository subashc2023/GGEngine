//! The windows of their own that the dock's torn-off groups live in: egui's immediate
//! viewports. egui runs each one's pass inside the main window's, through a callback, so the
//! callback opens the windows as egui asks for them, and the main window's frame renders them
//! all.

use std::cell::RefCell;
use std::iter;
use std::rc::Rc;
use std::sync::Arc;

use egui::epaint::ClippedShape;
use egui::{
    ClippedPrimitive, ImmediateViewport, OrderedViewportIdMap, Pos2, RawInput, Rect, TexturesDelta,
    ViewportBuilder, ViewportClass, ViewportCommand, ViewportEvent, ViewportId, ViewportIdMap,
    ViewportIdPair, ViewportInfo, ViewportOutput, pos2,
};
use egui_winit::EventResponse;
use gg_core::{Error, Result};
use gg_renderer::Renderer;
use tracing::{debug, warn};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::chrome::Chrome;

scoped_tls::scoped_thread_local! {
    /// The event loop while a frame runs, which egui's callback opens windows with.
    static EVENT_LOOP: ActiveEventLoop
}

/// Runs `frame` where egui's callback can open windows.
pub(crate) fn with_event_loop<R>(event_loop: &ActiveEventLoop, frame: impl FnOnce() -> R) -> R {
    EVENT_LOOP.set(event_loop, frame)
}

/// The windows of their own, which open and close as egui shows their viewports.
pub(crate) struct Viewports {
    shared: Rc<RefCell<Shared>>,
}

/// What the main window's frame shares with egui's callback. Neither holds a borrow while egui
/// runs a pass, since the callback runs inside the main window's.
struct Shared {
    /// The main window, which owns the others, and what egui knows of it.
    main: Arc<Window>,
    main_info: ViewportInfo,
    children: Vec<Child>,
    /// What the windows' passes drew this frame, in the order they ran.
    passes: Vec<Pass>,
    /// Whether the warning about deferred viewports was logged.
    warned: bool,
    /// Who draws the windows' frames.
    chrome: Chrome,
}

struct Child {
    id: ViewportId,
    window: Arc<Window>,
    input: WindowInput,
    info: ViewportInfo,
    /// What egui last asked of the window, which it compares what it asks next with.
    builder: ViewportBuilder,
    /// Whether the renderer presents to it yet.
    presented: bool,
}

struct Pass {
    viewport: ViewportId,
    shapes: Vec<ClippedShape>,
    pixels_per_point: f32,
    textures: TexturesDelta,
}

/// What a window's pass drew, for the renderer.
pub(crate) struct Drawn {
    key: u64,
    primitives: Vec<ClippedPrimitive>,
    textures: TexturesDelta,
    pixels_per_point: f32,
}

impl Drawn {
    /// The window's key in the renderer, and its UI.
    pub(crate) fn ui(&mut self) -> (u64, gg_renderer::Ui<'_>) {
        let ui = gg_renderer::Ui {
            primitives: &self.primitives,
            textures: &mut self.textures,
            pixels_per_point: self.pixels_per_point,
        };
        (self.key, ui)
    }
}

impl Viewports {
    /// Lets egui open windows of their own, which `main` owns, framed as `chrome` says. On
    /// Wayland, where windows can't be placed, egui keeps its viewports inside the main window
    /// instead.
    pub(crate) fn new(
        egui: &egui::Context,
        event_loop: &ActiveEventLoop,
        main: Arc<Window>,
        chrome: Chrome,
    ) -> Self {
        let mut main_info = ViewportInfo::default();
        egui_winit::update_viewport_info(&mut main_info, egui, &main, true);
        let shared = Rc::new(RefCell::new(Shared {
            main,
            main_info,
            children: Vec::new(),
            passes: Vec::new(),
            warned: false,
            chrome,
        }));
        let display = event_loop.display_handle().map(|display| display.as_raw());
        egui.set_embed_viewports(matches!(display, Ok(RawDisplayHandle::Wayland(_))));
        // The callback lasts as long as the thread, and the windows only as long as this.
        let weak = Rc::downgrade(&shared);
        egui::Context::set_immediate_viewport_renderer(move |egui, viewport| {
            let shared = weak
                .upgrade()
                .expect("egui runs viewports only in the main window's frames");
            run(&shared, egui, viewport);
        });
        Self { shared }
    }

    /// The main window's input for a frame, which tells egui where every window is.
    pub(crate) fn input(&self, main: &mut WindowInput) -> RawInput {
        let mut shared = self.shared.borrow_mut();
        let shared = &mut *shared;
        let egui = main.state.egui_ctx();
        egui_winit::update_viewport_info(&mut shared.main_info, egui, &shared.main, false);
        for child in &mut shared.children {
            egui_winit::update_viewport_info(&mut child.info, egui, &child.window, false);
        }
        let mut input = main.state.take_egui_input(&shared.main);
        input.viewports = shared.infos();
        input
    }

    /// After the main window's pass: makes the changes egui asks of the windows, closes those it
    /// no longer shows, and has the renderer present to the new ones. `main` is the main
    /// window's input. Returns what the windows' passes drew, in the order they ran, and whether
    /// egui asked to close the main window, which, as with the OS's close button, is the
    /// layers' to allow.
    pub(crate) fn end_frame(
        &self,
        egui: &egui::Context,
        main: &mut WindowInput,
        mut outputs: OrderedViewportIdMap<ViewportOutput>,
        renderer: &mut Renderer,
    ) -> (Vec<Drawn>, bool) {
        let _zone = gg_core::zone!();
        let mut shared = self.shared.borrow_mut();
        let shared = &mut *shared;
        if !shared.warned
            && outputs
                .values()
                .any(|output| output.class == ViewportClass::Deferred)
        {
            // A pass run after the main window's would leave egui's state pointing at the other
            // window.
            warn!(
                code = "engine.viewport_deferred",
                "egui's deferred viewports don't open; show them with `show_viewport_immediate`"
            );
            shared.warned = true;
        }
        // What the main window opened with stays as it was: only its commands apply.
        let mut close = false;
        if let Some(output) = outputs.remove(&ViewportId::ROOT) {
            let mut commands = output.commands;
            close = commands.contains(&ViewportCommand::Close);
            commands.retain(|command| *command != ViewportCommand::Close);
            apply(egui, &mut shared.main_info, main, &shared.main, commands);
        }
        shared.children.retain_mut(|child| {
            let Some(output) = outputs.remove(&child.id) else {
                debug!(title = child.window.title(), "Panel window closed");
                renderer.close_window(key(child.id));
                return false;
            };
            // This frame's passes saw its events.
            child.info.events.clear();
            // What changes only with a new window, the dock never changes.
            let (mut commands, _) = child.builder.patch(output.builder);
            commands.extend(output.commands);
            apply(
                egui,
                &mut child.info,
                &mut child.input,
                &child.window,
                commands,
            );
            if !child.presented {
                let size = child.window.inner_size();
                let window = child.window.clone();
                if let Err(error) =
                    renderer.open_window(key(child.id), window, size.width, size.height)
                {
                    embed(egui, &error);
                    return false;
                }
                child.presented = true;
            }
            true
        });
        // Tessellated only now, since the rest of the main window's pass could grow the font
        // atlas the shapes' text is in.
        let drawn = (shared.passes.drain(..))
            .map(|pass| Drawn {
                key: key(pass.viewport),
                primitives: egui.tessellate(pass.shapes, pass.pixels_per_point),
                textures: pass.textures,
                pixels_per_point: pass.pixels_per_point,
            })
            .collect();
        (drawn, close)
    }

    /// Hands an event of one of the windows of their own to egui. Their events never reach the
    /// layers.
    pub(crate) fn window_event(&self, id: WindowId, event: &WindowEvent, renderer: &mut Renderer) {
        let mut shared = self.shared.borrow_mut();
        let Some(child) = shared
            .children
            .iter_mut()
            .find(|child| child.window.id() == id)
        else {
            return;
        };
        match event {
            // They render with the main window's frames.
            WindowEvent::RedrawRequested => return,
            WindowEvent::Resized(size) => {
                renderer.resize_window(key(child.id), size.width, size.height);
            }
            // The dock floats the window's group over the main window instead.
            WindowEvent::CloseRequested => child.info.events.push(ViewportEvent::Close),
            _ => {}
        }
        // What egui doesn't keep goes nowhere else.
        let _ = child.input.on_window_event(&child.window, event);
    }
}

impl Shared {
    /// What egui knows of every window, which each pass's input carries.
    fn infos(&self) -> ViewportIdMap<ViewportInfo> {
        let children = (self.children.iter()).map(|child| (child.id, child.info.clone()));
        iter::once((ViewportId::ROOT, self.main_info.clone()))
            .chain(children)
            .collect()
    }
}

/// egui's callback: runs the pass of a window of its own, inside the main window's, opening the
/// window first if it's new.
fn run(shared: &RefCell<Shared>, egui: &egui::Context, viewport: ImmediateViewport<'_>) {
    let ImmediateViewport {
        ids,
        builder,
        mut viewport_ui_cb,
    } = viewport;
    let input = {
        let mut shared = shared.borrow_mut();
        let shared = &mut *shared;
        if !shared.children.iter().any(|child| child.id == ids.this) {
            let opened = EVENT_LOOP.with(|event_loop| {
                Child::open(egui, event_loop, &shared.main, shared.chrome, ids, &builder)
            });
            match opened {
                Ok(child) => shared.children.push(child),
                Err(error) => embed(egui, &error),
            }
        }
        let child = shared
            .children
            .iter_mut()
            .find(|child| child.id == ids.this);
        let mut input = match child {
            Some(child) => child.input.state.take_egui_input(&child.window),
            None => RawInput {
                viewport_id: ids.this,
                screen_rect: (builder.inner_size).map(|size| Rect::from_min_size(Pos2::ZERO, size)),
                ..RawInput::default()
            },
        };
        input.viewports = shared.infos();
        // A window that couldn't open still has its pass, as egui needs, though nothing shows it.
        input.viewports.entry(ids.this).or_default();
        input
    };
    let output = egui.run_ui(input, |ui| viewport_ui_cb(ui));
    let mut shared = shared.borrow_mut();
    let shared = &mut *shared;
    if let Some(child) = shared
        .children
        .iter_mut()
        .find(|child| child.id == ids.this)
    {
        (child.input.state).handle_platform_output(&child.window, output.platform_output);
    }
    match shared
        .passes
        .iter_mut()
        .find(|pass| pass.viewport == ids.this)
    {
        // egui discarded the frame's first pass and ran another, which draws; the texture
        // changes add up. The renderer applies the windows' changes before all of the main
        // window's, as eframe does, though here the passes took turns: a texture changed in
        // both, such as a font atlas that grows, can end up missing the windows' changes.
        // Ordering them exactly needs egui to hand over each of the main window's passes'
        // changes apart.
        Some(pass) => {
            pass.shapes = output.shapes;
            pass.pixels_per_point = output.pixels_per_point;
            pass.textures.append(output.textures_delta);
        }
        None => shared.passes.push(Pass {
            viewport: ids.this,
            shapes: output.shapes,
            pixels_per_point: output.pixels_per_point,
            textures: output.textures_delta,
        }),
    }
}

impl Child {
    fn open(
        egui: &egui::Context,
        event_loop: &ActiveEventLoop,
        main: &Window,
        chrome: Chrome,
        ids: ViewportIdPair,
        builder: &ViewportBuilder,
    ) -> Result<Self> {
        let attributes = egui_winit::create_winit_window_attributes(egui, builder.clone());
        let window = event_loop
            .create_window(owned_by(attributes, main))
            .map_err(|error| {
                Error::new(
                    "engine.viewport",
                    "can't open a window for a group of panels",
                )
                .with_source(error)
            })?;
        // What egui sets once the window is on a monitor, at that monitor's scale.
        egui_winit::apply_viewport_builder_to_window(egui, &window, builder);
        os_frame(chrome, &window)?;
        let mut info = ViewportInfo {
            parent: Some(ids.parent),
            ..ViewportInfo::default()
        };
        egui_winit::update_viewport_info(&mut info, egui, &window, true);
        let size = window.inner_size();
        debug!(
            title = window.title(),
            width = size.width,
            height = size.height,
            "Panel window opened"
        );
        Ok(Self {
            id: ids.this,
            input: WindowInput::new(egui, ids.this, &window),
            window: Arc::new(window),
            info,
            builder: builder.clone(),
            presented: false,
        })
    }
}

/// Makes the changes egui asks of `window`. Moving or resizing a window hands the pointer to the
/// OS, which on macOS, X11 and Wayland may never send the button's release back, so egui gets it
/// now, when egui-winit starts the move: only while the window has focus. On Windows the release
/// comes back as well, which egui and the layers ignore, having seen no press.
fn apply(
    egui: &egui::Context,
    info: &mut ViewportInfo,
    input: &mut WindowInput,
    window: &Window,
    commands: Vec<ViewportCommand>,
) {
    let moves = commands.iter().any(|command| match command {
        ViewportCommand::StartDrag => window.has_focus(),
        ViewportCommand::BeginResize(_) => true,
        _ => false,
    });
    if moves {
        input.release(egui::PointerButton::Primary);
    }
    // egui's requests to cut, copy and paste, which nothing in the engine sends, go unanswered.
    egui_winit::process_viewport_commands(egui, info, commands, window, &mut Vec::new());
}

/// On Windows, the main window owns the others: they stay above it, and hide while it's
/// minimized.
#[cfg(target_os = "windows")]
fn owned_by(attributes: WindowAttributes, main: &Window) -> WindowAttributes {
    use winit::platform::windows::WindowAttributesExtWindows;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match main.window_handle().map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => attributes.with_owner_window(handle.hwnd.get()),
        _ => attributes,
    }
}

#[cfg(not(target_os = "windows"))]
fn owned_by(attributes: WindowAttributes, _: &Window) -> WindowAttributes {
    attributes
}

/// On Windows, a window whose title bar the UI draws gets the rest of its frame from the OS.
#[cfg(target_os = "windows")]
fn os_frame(chrome: Chrome, window: &Window) -> Result<()> {
    if chrome == Chrome::InWindowsFrame {
        crate::windows_frame::Frame::install(window)?;
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn os_frame(_: Chrome, _: &Window) -> Result<()> {
    Ok(())
}

/// Keeps egui's viewports inside the main window from the next frame, after a window couldn't
/// open, so that the dock floats their groups over it.
fn embed(egui: &egui::Context, error: &Error) {
    warn!(
        code = error.code(),
        "{error:#}; panels float over the main window instead"
    );
    egui.set_embed_viewports(true);
}

/// The renderer's key for viewport `id`'s window.
fn key(id: ViewportId) -> u64 {
    id.0.value()
}

/// egui's view of one window: egui-winit's, which turns the window's events into egui's input,
/// and where the cursor last was in the window.
pub(crate) struct WindowInput {
    pub(crate) state: egui_winit::State,
    viewport: ViewportId,
    /// In points.
    cursor: Pos2,
}

impl WindowInput {
    pub(crate) fn new(egui: &egui::Context, viewport: ViewportId, window: &Window) -> Self {
        let scale = Some(window.scale_factor() as f32);
        Self {
            state: egui_winit::State::new(
                egui.clone(),
                viewport,
                window,
                scale,
                window.theme(),
                None,
            ),
            viewport,
            cursor: Pos2::ZERO,
        }
    }

    /// Hands `event` to egui. egui-winit drops a button's release when the cursor has left the
    /// window and not moved since, which would leave egui's drags, and the dock's, holding on,
    /// so that release goes in at the cursor's last position.
    pub(crate) fn on_window_event(
        &mut self,
        window: &Window,
        event: &WindowEvent,
    ) -> EventResponse {
        match *event {
            WindowEvent::CursorMoved { position, .. } => {
                let ppp = egui_winit::pixels_per_point(self.state.egui_ctx(), window);
                self.cursor = pos2(position.x as f32 / ppp, position.y as f32 / ppp);
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button,
                ..
            } if !self.state.is_pointer_in_window() => {
                if let Some(button) = pointer_button(button) {
                    self.release(button);
                }
            }
            _ => {}
        }
        self.state.on_window_event(window, event)
    }

    /// Lets go of `button` in egui, at the cursor's last position.
    fn release(&mut self, button: egui::PointerButton) {
        let modifiers = (self.state.egui_ctx()).input_for(self.viewport, |i| i.modifiers);
        self.state
            .egui_input_mut()
            .events
            .push(egui::Event::PointerButton {
                pos: self.cursor,
                button,
                pressed: false,
                modifiers,
            });
    }
}

/// egui's name for a mouse button, as egui-winit names them.
fn pointer_button(button: MouseButton) -> Option<egui::PointerButton> {
    match button {
        MouseButton::Left => Some(egui::PointerButton::Primary),
        MouseButton::Right => Some(egui::PointerButton::Secondary),
        MouseButton::Middle => Some(egui::PointerButton::Middle),
        MouseButton::Back => Some(egui::PointerButton::Extra1),
        MouseButton::Forward => Some(egui::PointerButton::Extra2),
        MouseButton::Other(_) => None,
    }
}
