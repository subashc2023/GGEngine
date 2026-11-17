//! The windows of the dock's torn-off groups: egui's immediate viewports. A callback opens them
//! as egui asks and runs each pass inside the main window's; its frame renders them all.

use std::cell::RefCell;
use std::iter;
use std::rc::Rc;
use std::sync::Arc;

use egui::epaint::ClippedShape;
use egui::{
    ClippedPrimitive, ImmediateViewport, OrderedViewportIdMap, Pos2, RawInput, Rect,
    ViewportBuilder, ViewportClass, ViewportCommand, ViewportEvent, ViewportId, ViewportIdMap,
    ViewportIdPair, ViewportIdSet, ViewportInfo, ViewportOutput, pos2,
};
use egui_winit::EventResponse;
use gg_core::{Error, Result};
use gg_renderer::Renderer;
use tracing::{debug, warn};
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::chrome::Chrome;
use crate::render_thread::TextureChanges;

scoped_tls::scoped_thread_local! {
    static EVENT_LOOP: ActiveEventLoop
}

/// Runs `frame` with the event loop set, for egui's callback to open windows.
pub(crate) fn with_event_loop<R>(event_loop: &ActiveEventLoop, frame: impl FnOnce() -> R) -> R {
    EVENT_LOOP.set(event_loop, frame)
}

/// The windows that open and close as egui shows their viewports.
pub(crate) struct Viewports {
    shared: Rc<RefCell<Shared>>,
}

/// What the main window's frame shares with egui's callback. Neither holds a borrow while egui
/// runs a pass: the callback runs inside the main window's.
struct Shared {
    /// The main window, which owns the others, and what egui knows of it.
    main: Arc<Window>,
    main_info: ViewportInfo,
    children: Vec<Child>,
    /// This frame's passes, in run order.
    passes: Vec<Pass>,
    /// Deferred viewports already warned about; they don't open.
    warned: ViewportIdSet,
    chrome: Chrome,
}

struct Child {
    id: ViewportId,
    window: Arc<Window>,
    input: WindowInput,
    info: ViewportInfo,
    /// What egui last asked, to diff against its next ask.
    builder: ViewportBuilder,
    /// Whether the renderer presents to it yet.
    presented: bool,
    /// The size taken while the renderer was away, told to it once back.
    resized: Option<PhysicalSize<u32>>,
}

struct Pass {
    viewport: ViewportId,
    shapes: Vec<ClippedShape>,
    pixels_per_point: f32,
    textures: TextureChanges,
}

pub(crate) struct Drawn {
    key: u64,
    primitives: Vec<ClippedPrimitive>,
    textures: TextureChanges,
    pixels_per_point: f32,
}

impl Drawn {
    /// The window's renderer key, and its UI.
    pub(crate) fn ui(&mut self) -> (u64, gg_renderer::Ui<'_>) {
        let ui = gg_renderer::Ui {
            primitives: &self.primitives,
            textures: &mut self.textures.0,
            pixels_per_point: self.pixels_per_point,
        };
        (self.key, ui)
    }
}

impl Viewports {
    /// Lets egui open windows that `main` owns, framed by `chrome`. On Wayland, where windows
    /// can't be placed, egui keeps viewports inside the main window.
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
            warned: ViewportIdSet::default(),
            chrome,
        }));
        let display = event_loop.display_handle().map(|display| display.as_raw());
        egui.set_embed_viewports(matches!(display, Ok(RawDisplayHandle::Wayland(_))));
        // The callback outlives this; the windows don't.
        let weak = Rc::downgrade(&shared);
        egui::Context::set_immediate_viewport_renderer(move |egui, viewport| {
            let shared = weak
                .upgrade()
                .expect("egui runs viewports only in the main window's frames");
            run(&shared, egui, viewport);
        });
        Self { shared }
    }

    /// The main window's input for a frame, with every window's place.
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

    /// After the main window's pass, with the renderer back: applies egui's window changes,
    /// closes windows it no longer shows, tells the renderer their new sizes and has it present
    /// to new windows. `main` is the main window's input. Returns the windows' drawing in pass
    /// order, and whether egui asked to close the main window; the layers allow that, as with
    /// the OS's close button.
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
        for (&id, output) in &outputs {
            // A pass after the main window's would leave egui's state pointing at the other
            // window.
            if output.class == ViewportClass::Deferred && shared.warned.insert(id) {
                let title = title(&output.builder);
                warn!(
                    code = "engine.viewport_deferred",
                    window = title,
                    "egui's deferred viewport {title:?} doesn't open; show it with \
                     `show_viewport_immediate`"
                );
            }
        }
        // The main window's builder stays as opened; only its commands apply.
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
            // This frame's passes saw them.
            child.info.events.clear();
            // The dock never changes what only a new window changes.
            let (mut commands, _) = child.builder.patch(output.builder);
            commands.extend(output.commands);
            apply(
                egui,
                &mut child.info,
                &mut child.input,
                &child.window,
                commands,
            );
            // A new window already has the size it took.
            let resized = child.resized.take();
            if !child.presented {
                let size = child.window.inner_size();
                let window = child.window.clone();
                let title = child.window.title();
                if let Err(error) =
                    renderer.open_window(key(child.id), window, &title, size.width, size.height)
                {
                    embed(egui, &error, &title);
                    return false;
                }
                child.presented = true;
            } else if let Some(size) = resized {
                renderer.resize_window(key(child.id), size.width, size.height);
            }
            true
        });
        // Tessellated now: the main window's pass could grow the font atlas.
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

    /// Hands egui a window's event; the layers never see them. A new size waits for
    /// [`Viewports::end_frame`]: the renderer is away rendering between frames.
    pub(crate) fn window_event(&self, id: WindowId, event: &WindowEvent) {
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
            WindowEvent::Resized(size) => child.resized = Some(*size),
            // The dock floats the window's group over the main window instead.
            WindowEvent::CloseRequested => child.info.events.push(ViewportEvent::Close),
            _ => {}
        }
        // Nothing else gets what egui doesn't keep.
        let _ = child.input.on_window_event(&child.window, event);
    }
}

impl Shared {
    /// What egui knows of every window, for each pass's input.
    fn infos(&self) -> ViewportIdMap<ViewportInfo> {
        let children = (self.children.iter()).map(|child| (child.id, child.info.clone()));
        iter::once((ViewportId::ROOT, self.main_info.clone()))
            .chain(children)
            .collect()
    }
}

/// egui's callback: runs a window's pass inside the main window's, opening the window if new.
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
                Err(error) => embed(egui, &error, title(&builder)),
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
                max_texture_side: Some(crate::main_loop::MAX_TEXTURE_SIDE),
                ..RawInput::default()
            },
        };
        input.viewports = shared.infos();
        // A window that couldn't open still gets its pass, as egui needs; nothing shows it.
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
        // egui discarded the first pass and ran another, which draws; texture changes add up.
        // The renderer applies the windows' changes before all the main window's, as eframe
        // does, though the passes took turns here: a texture changed in both (a growing font
        // atlas) can miss the windows' changes. Exact order needs egui to hand over each
        // main-window pass's changes apart.
        Some(pass) => {
            pass.shapes = output.shapes;
            pass.pixels_per_point = output.pixels_per_point;
            pass.textures.0.append(output.textures_delta);
        }
        None => shared.passes.push(Pass {
            viewport: ids.this,
            shapes: output.shapes,
            pixels_per_point: output.pixels_per_point,
            textures: TextureChanges(output.textures_delta),
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
                    format!(
                        "can't open window {:?} for a group of panels",
                        title(builder)
                    ),
                )
                .with_source(error)
            })?;
        // What egui sets once the window is on a monitor, at its scale.
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
            resized: None,
        })
    }
}

/// Applies egui's changes to `window`. A move or resize hands the pointer to the OS, which on
/// macOS, X11 and Wayland may never send back the button's release, so egui gets it as
/// egui-winit starts the move, only while focused. On Windows the release comes back too; egui
/// and the layers ignore it, having seen no press.
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
    // egui's cut, copy and paste requests go unanswered; the engine sends none.
    egui_winit::process_viewport_commands(egui, info, commands, window, &mut Vec::new());
}

/// On Windows the main window owns the others: they stay above it and hide when it's minimized.
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

/// After window `title` fails to open, keeps viewports inside the main window from the next
/// frame: the dock floats their groups over it.
fn embed(egui: &egui::Context, error: &Error, title: &str) {
    warn!(
        code = error.code(),
        window = title,
        "{error:#}; panels float over the main window instead"
    );
    egui.set_embed_viewports(true);
}

/// The title of the window `builder` opens: the dock's is its showing panel.
fn title(builder: &ViewportBuilder) -> &str {
    builder.title.as_deref().unwrap_or_default()
}

fn key(id: ViewportId) -> u64 {
    id.0.value()
}

/// egui's view of one window: egui-winit's state and the cursor.
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
                Some(crate::main_loop::MAX_TEXTURE_SIDE),
            ),
            viewport,
            cursor: Pos2::ZERO,
        }
    }

    pub(crate) fn cursor(&self) -> Pos2 {
        self.cursor
    }

    /// Hands `event` to egui. egui-winit drops a release when the cursor has left the window and
    /// not moved since, leaving egui's and the dock's drags held; so that release goes in at the
    /// cursor's last position.
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
