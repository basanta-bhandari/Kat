use crossterm::{
    cursor::MoveTo,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use forgetui::{
    formats,
    graphics::{self, GraphicsMode},
    math::Vec3,
    pixel_renderer::{self, PixelStyle},
    print, projects, renderer,
    scene::{PrimitiveKind, Scene, ShadingMode, tube_with_dimensions},
    update,
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    error::Error,
    fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

const HELP: &str = "3D-PRINT CAD — MILLIMETRES\n  A Add   G Move   R Rotate   S Scale   E Export   ? Help\n\nMOUSE TOOLS\n  Click selects; clicking never starts a command prompt.\n  Select: left-drag orbit · right/middle-drag pan\n  Move:   left-drag screen plane · right-drag Z height\n  Rotate: left-drag X/Z · right-drag Y/Z\n  Scale:  left-drag uniform · right-drag Z only\n  Right-click without dragging opens object actions.\n  Wheel zooms; top menus and side tools are clickable.\n\nPRINT COMMANDS\n  size 20 30 5       exact X/Y/Z size in mm\n  size z 12          exact size on one axis\n  place               put selection on build plate\n  center              centre selection and place it\n  check                printable-solid validation\n  repair               weld/remove broken mesh data\n  bed 220 220 250      set build volume in mm\n  slice                export 3MF and open a slicer\n\nFILES\n  save job.forge       native editable project\n  open part.stl        STL/3MF import\n  export part.3mf      3MF or STL for printing\n  export part.step     optional FreeCAD bridge\n\nGRAPHICS  auto · kitty · sixel · ascii\nTHEMES    midnight · amber · matrix · light · mono\n\nEsc or ? closes this panel.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeChoice {
    Midnight,
    Amber,
    Matrix,
    Light,
    Mono,
}

impl ThemeChoice {
    fn name(self) -> &'static str {
        match self {
            Self::Midnight => "midnight",
            Self::Amber => "amber",
            Self::Matrix => "matrix",
            Self::Light => "light",
            Self::Mono => "mono",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Midnight => Self::Amber,
            Self::Amber => Self::Matrix,
            Self::Matrix => Self::Light,
            Self::Light => Self::Mono,
            Self::Mono => Self::Midnight,
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "midnight" | "dark" | "blue" => Some(Self::Midnight),
            "amber" | "orange" => Some(Self::Amber),
            "matrix" | "green" => Some(Self::Matrix),
            "light" => Some(Self::Light),
            "mono" | "gray" | "grey" => Some(Self::Mono),
            _ => None,
        }
    }

    fn colors(self) -> Theme {
        match self {
            Self::Midnight => Theme::new(
                Color::Rgb(10, 15, 23),
                Color::Rgb(20, 29, 40),
                Color::Rgb(222, 230, 238),
                Color::Rgb(94, 220, 220),
                Color::Rgb(120, 135, 150),
            ),
            Self::Amber => Theme::new(
                Color::Rgb(20, 14, 8),
                Color::Rgb(43, 30, 14),
                Color::Rgb(255, 225, 170),
                Color::Rgb(255, 174, 66),
                Color::Rgb(180, 130, 75),
            ),
            Self::Matrix => Theme::new(
                Color::Black,
                Color::Rgb(5, 28, 14),
                Color::Rgb(166, 255, 180),
                Color::Rgb(45, 255, 105),
                Color::Rgb(50, 145, 75),
            ),
            Self::Light => Theme::new(
                Color::Rgb(250, 251, 252),
                Color::Rgb(234, 238, 242),
                Color::Rgb(37, 47, 58),
                Color::Rgb(18, 105, 173),
                Color::Rgb(145, 157, 168),
            ),
            Self::Mono => Theme::new(
                Color::Black,
                Color::Rgb(28, 28, 28),
                Color::White,
                Color::White,
                Color::DarkGray,
            ),
        }
    }
}

#[derive(Clone, Copy)]
struct Theme {
    background: Color,
    panel: Color,
    foreground: Color,
    accent: Color,
    muted: Color,
}

impl Theme {
    const fn new(
        background: Color,
        panel: Color,
        foreground: Color,
        accent: Color,
        muted: Color,
    ) -> Self {
        Self {
            background,
            panel,
            foreground,
            accent,
            muted,
        }
    }
}

#[derive(Default)]
struct UiRegions {
    outliner: Rect,
    viewport: Rect,
    toolbar: Rect,
    navbar: Rect,
    context: Rect,
    save_parent: Rect,
    save_list: Rect,
    save_name: Rect,
    save_formats: Rect,
    save_button: Rect,
    save_cancel: Rect,
    project_list: Rect,
}

#[derive(Clone, Copy)]
struct DragState {
    button: MouseButton,
    column: u16,
    row: u16,
    moved: bool,
    checkpointed: bool,
    hit_object: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    Help,
    Add,
    Command,
    Context,
    SaveAs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SaveFormat {
    Forge,
    ThreeMf,
    Stl,
    Step,
}

impl SaveFormat {
    const ALL: [Self; 4] = [Self::Forge, Self::ThreeMf, Self::Stl, Self::Step];

    fn label(self) -> &'static str {
        match self {
            Self::Forge => "Project (.forge)",
            Self::ThreeMf => "3MF (.3mf)",
            Self::Stl => "STL (.stl)",
            Self::Step => "STEP (.step)",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Forge => "forge",
            Self::ThreeMf => "3mf",
            Self::Stl => "stl",
            Self::Step => "step",
        }
    }

    fn shift(self, direction: isize) -> Self {
        let index = Self::ALL
            .iter()
            .position(|format| *format == self)
            .unwrap_or(0);
        Self::ALL[(index as isize + direction).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SaveFocus {
    Browse,
    Name,
    Format,
    Actions,
}

impl SaveFocus {
    fn next(self) -> Self {
        match self {
            Self::Browse => Self::Name,
            Self::Name => Self::Format,
            Self::Format => Self::Actions,
            Self::Actions => Self::Browse,
        }
    }
}

#[derive(Debug, Clone)]
struct BrowserEntry {
    name: String,
    path: PathBuf,
    directory: bool,
}

#[derive(Debug, Clone)]
struct SaveDialog {
    directory: PathBuf,
    entries: Vec<BrowserEntry>,
    selected: usize,
    filename: String,
    filename_pristine: bool,
    format: SaveFormat,
    focus: SaveFocus,
    save_action: bool,
    overwrite: bool,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolKind {
    Select,
    Move,
    Rotate,
    Scale,
    Measure,
    Frame,
    Check,
}

impl ToolKind {
    const ALL: [Self; 7] = [
        Self::Select,
        Self::Move,
        Self::Rotate,
        Self::Scale,
        Self::Measure,
        Self::Frame,
        Self::Check,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Select => "▶ Select",
            Self::Move => "↔ Move",
            Self::Rotate => "⟳ Rotate",
            Self::Scale => "⤢ Scale",
            Self::Measure => "⌖ Measure",
            Self::Frame => "◎ Frame",
            Self::Check => "✓ Check",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    Object,
    File,
    Edit,
    Add,
    View,
    Print,
    Update,
}

#[derive(Debug, Clone, Copy)]
struct ContextMenu {
    kind: MenuKind,
    x: u16,
    y: u16,
    selected: usize,
}

struct App {
    scene: Scene,
    path: Option<PathBuf>,
    history: Vec<Scene>,
    future: Vec<Scene>,
    overlay: Overlay,
    command: String,
    status: String,
    dirty: bool,
    quit: bool,
    theme: ThemeChoice,
    snap: f32,
    regions: UiRegions,
    pick_frame: Option<renderer::RenderedFrame>,
    drag: Option<DragState>,
    graphics: GraphicsMode,
    kitty_image_id: u32,
    active_tool: ToolKind,
    context_menu: Option<ContextMenu>,
    measure_from: Option<Vec3>,
    save_dialog: Option<SaveDialog>,
    startup: bool,
    project_directory: PathBuf,
    project_entries: Vec<projects::ProjectEntry>,
    project_selected: usize,
    project_filter: String,
    project_filtering: bool,
    update_receiver: Option<Receiver<Result<String, String>>>,
}

impl App {
    fn new(scene: Scene, path: Option<PathBuf>) -> Self {
        Self {
            scene,
            path,
            history: Vec::new(),
            future: Vec::new(),
            overlay: Overlay::None,
            command: String::new(),
            status: "Ready — press ? for help, : for commands".into(),
            dirty: false,
            quit: false,
            theme: ThemeChoice::Light,
            snap: 0.25,
            regions: UiRegions::default(),
            pick_frame: None,
            drag: None,
            graphics: std::env::var("FORGETUI_GRAPHICS")
                .ok()
                .and_then(|value| GraphicsMode::parse(&value.to_ascii_lowercase()))
                .unwrap_or(GraphicsMode::Auto),
            kitty_image_id: 0,
            active_tool: ToolKind::Select,
            context_menu: None,
            measure_from: None,
            save_dialog: None,
            startup: false,
            project_directory: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            project_entries: Vec::new(),
            project_selected: 0,
            project_filter: String::new(),
            project_filtering: false,
            update_receiver: None,
        }
    }

    fn refresh_projects(&mut self) {
        match projects::browse(&self.project_directory) {
            Ok(entries) => {
                let filter = self.project_filter.to_lowercase();
                self.project_entries = entries
                    .into_iter()
                    .filter(|entry| entry.name.to_lowercase().contains(&filter))
                    .collect();
                self.project_selected = self
                    .project_selected
                    .min(self.project_entries.len().saturating_sub(1));
            }
            Err(error) => self.status = format!("Cannot browse: {error}"),
        }
    }

    fn start_update(&mut self, install: bool) {
        if self.update_receiver.is_some() {
            self.status = "An update operation is already running".into();
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.update_receiver = Some(receiver);
        self.status = if install {
            "Downloading and building update…"
        } else {
            "Checking main over SSH…"
        }
        .into();
        std::thread::spawn(move || {
            let result = if install {
                update::install()
            } else {
                update::check()
            };
            let _ = sender.send(result);
        });
    }

    fn poll_update(&mut self) -> bool {
        let Some(receiver) = self.update_receiver.as_ref() else {
            return false;
        };
        match receiver.try_recv() {
            Ok(result) => {
                self.status = result.unwrap_or_else(|error| format!("Error: {error}"));
                self.update_receiver = None;
                true
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.status = "Error: update worker stopped".into();
                self.update_receiver = None;
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
        }
    }

    fn activate_project(&mut self) {
        let Some(entry) = self.project_entries.get(self.project_selected).cloned() else {
            return;
        };
        if entry.is_directory {
            self.project_directory = entry.path;
            self.project_selected = 0;
            self.project_filter.clear();
            self.refresh_projects();
        } else {
            self.open(entry.path);
            if self.status.starts_with("Opened ") {
                self.startup = false;
            }
        }
    }

    fn handle_project_key(&mut self, key: KeyEvent) {
        if self.project_filtering {
            match key.code {
                KeyCode::Enter => self.project_filtering = false,
                KeyCode::Esc => {
                    self.project_filter.clear();
                    self.project_filtering = false;
                }
                KeyCode::Backspace => {
                    self.project_filter.pop();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.project_filter.push(character)
                }
                _ => {}
            }
            self.project_selected = 0;
            self.refresh_projects();
            return;
        }
        match key.code {
            KeyCode::Char('n') => {
                self.scene = Scene::default();
                self.path = None;
                self.startup = false;
            }
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('/') => self.project_filtering = true,
            KeyCode::Up => self.project_selected = self.project_selected.saturating_sub(1),
            KeyCode::Down => {
                self.project_selected =
                    (self.project_selected + 1).min(self.project_entries.len().saturating_sub(1))
            }
            KeyCode::Enter | KeyCode::Right => self.activate_project(),
            KeyCode::Backspace | KeyCode::Left => {
                if let Some(parent) = self.project_directory.parent() {
                    self.project_directory = parent.to_path_buf();
                    self.project_selected = 0;
                    self.project_filter.clear();
                    self.refresh_projects();
                }
            }
            _ => {}
        }
    }

    fn checkpoint(&mut self) {
        if self.history.len() == 100 {
            self.history.remove(0);
        }
        self.history.push(self.scene.clone());
        self.future.clear();
        self.dirty = true;
    }

    fn undo(&mut self) {
        if let Some(previous) = self.history.pop() {
            self.future
                .push(std::mem::replace(&mut self.scene, previous));
            self.dirty = true;
            self.status = "Undo".into();
        } else {
            self.status = "Nothing to undo".into();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.future.pop() {
            self.history.push(std::mem::replace(&mut self.scene, next));
            self.dirty = true;
            self.status = "Redo".into();
        } else {
            self.status = "Nothing to redo".into();
        }
    }

    fn add(&mut self, kind: PrimitiveKind) {
        self.checkpoint();
        self.scene.add_primitive(kind);
        self.status = format!("Added {}", kind.label());
        self.overlay = Overlay::None;
    }

    fn remove(&mut self) {
        if self.scene.selected().is_some() {
            self.checkpoint();
            let removed = self.scene.remove_selected().expect("selection checked");
            self.status = format!("Removed {}", removed.name);
        }
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        match formats::save(&self.scene, &path) {
            Ok(()) => {
                if matches!(path.extension().and_then(|e| e.to_str()), Some("forge")) {
                    self.path = Some(path.clone());
                    self.dirty = false;
                }
                self.status = format!("Saved {}", path.display());
                true
            }
            Err(error) => {
                self.status = format!("Error: {error}");
                false
            }
        }
    }

    fn open_save_dialog(&mut self, format: SaveFormat) {
        let directory = self
            .path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let filename = if format == SaveFormat::Forge {
            self.path
                .as_deref()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{}.forge", safe_file_stem(&self.scene.name)))
        } else {
            format!(
                "{}.{}",
                safe_file_stem(&self.scene.name),
                format.extension()
            )
        };
        let mut dialog = SaveDialog {
            directory,
            entries: Vec::new(),
            selected: 0,
            filename,
            filename_pristine: true,
            format,
            focus: SaveFocus::Name,
            save_action: true,
            overwrite: false,
            message: "Tab changes section · Enter opens folder/saves · Esc cancels".into(),
        };
        refresh_save_entries(&mut dialog);
        self.save_dialog = Some(dialog);
        self.overlay = Overlay::SaveAs;
    }

    fn change_save_format(&mut self, direction: isize) {
        let Some(dialog) = self.save_dialog.as_mut() else {
            return;
        };
        dialog.format = dialog.format.shift(direction);
        let stem = Path::new(&dialog.filename)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("model");
        dialog.filename = format!("{stem}.{}", dialog.format.extension());
        dialog.overwrite = false;
    }

    fn save_dialog_parent(&mut self) {
        let Some(dialog) = self.save_dialog.as_mut() else {
            return;
        };
        if let Some(parent) = dialog.directory.parent() {
            dialog.directory = parent.to_path_buf();
            dialog.selected = 0;
            dialog.overwrite = false;
            refresh_save_entries(dialog);
        }
    }

    fn activate_save_entry(&mut self) {
        let Some(dialog) = self.save_dialog.as_mut() else {
            return;
        };
        let Some(entry) = dialog.entries.get(dialog.selected).cloned() else {
            return;
        };
        if entry.directory {
            dialog.directory = entry.path;
            dialog.selected = 0;
            dialog.overwrite = false;
            refresh_save_entries(dialog);
        } else {
            dialog.filename = entry.name;
            dialog.filename_pristine = true;
            dialog.focus = SaveFocus::Name;
            dialog.overwrite = false;
        }
    }

    fn confirm_save_dialog(&mut self) {
        let Some(dialog) = self.save_dialog.as_ref() else {
            return;
        };
        let filename = dialog.filename.trim();
        if filename.is_empty() {
            if let Some(dialog) = self.save_dialog.as_mut() {
                dialog.message = "Enter a filename first".into();
            }
            return;
        }
        let mut path = dialog.directory.join(filename);
        path.set_extension(dialog.format.extension());
        if path.exists() && !dialog.overwrite {
            if let Some(dialog) = self.save_dialog.as_mut() {
                dialog.overwrite = true;
                dialog.message =
                    format!("{} exists — choose Save again to overwrite", path.display());
            }
            return;
        }
        if self.save_to(path) {
            self.save_dialog = None;
            self.overlay = Overlay::None;
        } else if let Some(dialog) = self.save_dialog.as_mut() {
            dialog.message = self.status.clone();
        }
    }

    fn open(&mut self, path: PathBuf) {
        match formats::load(&path) {
            Ok(scene) => {
                self.history.clear();
                self.future.clear();
                self.scene = scene;
                self.path = matches!(path.extension().and_then(|e| e.to_str()), Some("forge"))
                    .then_some(path.clone());
                self.dirty = false;
                self.status = format!("Opened {}", path.display());
            }
            Err(error) => self.status = format!("Error: {error}"),
        }
    }

    fn execute_command(&mut self) {
        let input = self.command.trim().to_string();
        self.command.clear();
        self.overlay = Overlay::None;
        if input.is_empty() {
            return;
        }
        let mut parts = input.split_whitespace();
        let command = parts.next().unwrap_or_default().to_ascii_lowercase();
        let args: Vec<&str> = parts.collect();
        let result = match command.as_str() {
            "add" | "a" => self.command_add(&args),
            "move" | "grab" | "g" => self.command_vector(&args, VectorOperation::Move),
            "rotate" | "r" => self.command_vector(&args, VectorOperation::Rotate),
            "scale" | "s" => self.command_scale(&args),
            "size" | "dimensions" | "dim" => self.command_size(&args),
            "array" | "pattern" => self.command_array(&args),
            "duplicate" | "dup" | "d" => {
                self.checkpoint();
                self.scene.duplicate_selected();
                Ok("Duplicated selection".into())
            }
            "reset" => {
                self.checkpoint();
                if let Some(object) = self.scene.selected_mut() {
                    object.transform = Default::default();
                }
                Ok("Reset transform".into())
            }
            "mirror" => self.command_mirror(&args),
            "snap" => self.command_snap(&args),
            "theme" => self.command_theme(&args),
            "graphics" | "renderer" => self.command_graphics(&args),
            "view" => self.command_view(&args),
            "check" | "validate" => Ok(print::validate(&self.scene).summary()),
            "repair" => self.command_repair(),
            "place" => self.command_place(false),
            "center" => self.command_place(true),
            "bed" => self.command_bed(&args),
            "slice" | "slicer" => {
                formats::open_in_slicer(&self.scene).map_err(|error| error.to_string())
            }
            "rename" => {
                let name = input
                    .split_once(char::is_whitespace)
                    .map(|(_, rest)| rest.trim())
                    .unwrap_or_default();
                if name.is_empty() {
                    Err("usage: rename NAME".into())
                } else {
                    self.checkpoint();
                    if let Some(object) = self.scene.selected_mut() {
                        object.name = name.into();
                    }
                    Ok(format!("Renamed selection to {name}"))
                }
            }
            "delete" | "remove" => {
                self.remove();
                return;
            }
            "hide" | "show" => {
                self.checkpoint();
                if let Some(object) = self.scene.selected_mut() {
                    object.visible = command == "show";
                }
                Ok(format!(
                    "Selection is now {}",
                    if command == "show" {
                        "visible"
                    } else {
                        "hidden"
                    }
                ))
            }
            "mode" => self.command_mode(&args),
            "frame" => {
                if let Some(object) = self.scene.selected() {
                    self.scene.camera.target = object.transform.position;
                }
                Ok("Framed selection".into())
            }
            "save" => {
                if let Some(path) = argument_path(&input).or_else(|| self.path.clone()) {
                    self.save_to(path);
                } else {
                    self.open_save_dialog(SaveFormat::Forge);
                }
                return;
            }
            "export" | "e" => match argument_path(&input) {
                Some(path) => {
                    self.save_to(path);
                    return;
                }
                None => {
                    self.open_save_dialog(SaveFormat::ThreeMf);
                    return;
                }
            },
            "open" | "o" => match argument_path(&input) {
                Some(path) => {
                    self.open(path);
                    return;
                }
                None => Err("usage: open FILE".into()),
            },
            "new" => {
                self.checkpoint();
                self.scene = Scene::default();
                self.path = None;
                Ok("New scene".into())
            }
            "undo" => {
                self.undo();
                return;
            }
            "redo" => {
                self.redo();
                return;
            }
            "help" => {
                self.overlay = Overlay::Help;
                return;
            }
            "quit" | "q" => {
                self.quit = true;
                return;
            }
            _ => Err(format!("unknown command '{command}' (press ? for help)")),
        };
        self.status = match result {
            Ok(message) => message,
            Err(error) => format!("Error: {error}"),
        };
    }

    fn command_add(&mut self, args: &[&str]) -> Result<String, String> {
        let kind = match args.first().copied().unwrap_or_default() {
            "cube" | "box" => PrimitiveKind::Cube,
            "sphere" | "uvsphere" => PrimitiveKind::Sphere,
            "cylinder" => PrimitiveKind::Cylinder,
            "cone" => PrimitiveKind::Cone,
            "torus" | "donut" => PrimitiveKind::Torus,
            "wedge" | "ramp" => PrimitiveKind::Wedge,
            "plane" => PrimitiveKind::Plane,
            "tube" | "pipe" => PrimitiveKind::Tube,
            _ => return Err("try: add cube|sphere|cylinder|cone|torus|wedge|plane|tube".into()),
        };
        let custom_tube = if kind == PrimitiveKind::Tube && args.len() > 1 {
            let [_, outer, inner, height] = args else {
                return Err("usage: add tube OUTER_MM INNER_MM HEIGHT_MM".into());
            };
            let mesh = tube_with_dimensions(
                parse_positive(outer)?,
                parse_positive(inner)?,
                parse_positive(height)?,
            )
            .ok_or(
                "tube dimensions must be finite, positive, and inner diameter smaller than outer",
            )?;
            Some(mesh)
        } else {
            if args.len() > 1 {
                return Err(
                    "additional dimensions are supported for: add tube OUTER INNER HEIGHT".into(),
                );
            }
            None
        };
        self.checkpoint();
        self.scene.add_primitive(kind);
        if let Some(mesh) = custom_tube {
            let object = self.scene.selected_mut().expect("new object is selected");
            object.mesh = mesh;
            print::place_on_bed(object);
        }
        Ok(format!("Added {}", kind.label()))
    }

    fn command_vector(
        &mut self,
        args: &[&str],
        operation: VectorOperation,
    ) -> Result<String, String> {
        if args.len() != 2 {
            return Err(format!("usage: {} x|y|z NUMBER", operation.name()));
        }
        let axis = args[0];
        let amount: f32 = args[1]
            .parse()
            .map_err(|_| "value must be a number".to_string())?;
        if !matches!(axis, "x" | "y" | "z") {
            return Err("axis must be x, y, or z".into());
        }
        self.checkpoint();
        let Some(object) = self.scene.selected_mut() else {
            return Err("nothing selected".into());
        };
        let vector = match operation {
            VectorOperation::Move => &mut object.transform.position,
            VectorOperation::Rotate => &mut object.transform.rotation,
        };
        match axis {
            "x" => vector.x += amount,
            "y" => vector.y += amount,
            "z" => vector.z += amount,
            _ => unreachable!(),
        }
        Ok(format!(
            "{} {} by {amount}",
            operation.name(),
            axis.to_uppercase()
        ))
    }

    fn command_scale(&mut self, args: &[&str]) -> Result<String, String> {
        let (axis, factor) = match args {
            [factor] => (None, factor.parse::<f32>()),
            [axis @ ("x" | "y" | "z"), factor] => (Some(*axis), factor.parse::<f32>()),
            _ => return Err("usage: scale NUMBER | scale x|y|z NUMBER".into()),
        };
        let factor = factor.map_err(|_| "scale must be a number".to_string())?;
        if factor.abs() < 0.0001 {
            return Err("scale cannot be zero".into());
        }
        self.checkpoint();
        let Some(object) = self.scene.selected_mut() else {
            return Err("nothing selected".into());
        };
        match axis {
            None => object.transform.scale = object.transform.scale * factor,
            Some("x") => object.transform.scale.x *= factor,
            Some("y") => object.transform.scale.y *= factor,
            Some("z") => object.transform.scale.z *= factor,
            _ => unreachable!(),
        }
        Ok(format!("Scaled by {factor}"))
    }

    fn command_array(&mut self, args: &[&str]) -> Result<String, String> {
        let [axis, count, spacing] = args else {
            return Err("usage: array x|y|z COUNT SPACING_MM".into());
        };
        let count: usize = count.parse().map_err(|_| "count must be a whole number")?;
        if !(2..=100).contains(&count) {
            return Err("count must be between 2 and 100".into());
        }
        let spacing: f32 = spacing.parse().map_err(|_| "spacing must be a number")?;
        if !spacing.is_finite() || spacing == 0.0 {
            return Err("spacing must be finite and nonzero".into());
        }
        let offset = match *axis {
            "x" => Vec3::new(spacing, 0.0, 0.0),
            "y" => Vec3::new(0.0, spacing, 0.0),
            "z" => Vec3::new(0.0, 0.0, spacing),
            _ => return Err("axis must be x, y, or z".into()),
        };
        if self.scene.selected().is_none() {
            return Err("nothing selected".into());
        }
        self.checkpoint();
        self.scene.linear_array(count, offset);
        Ok(format!(
            "Created {count} objects, {spacing} mm apart on {}",
            axis.to_uppercase()
        ))
    }

    fn command_size(&mut self, args: &[&str]) -> Result<String, String> {
        let selected = self.scene.selected().ok_or("nothing selected")?;
        let rotation = selected.transform.rotation;
        if rotation.x.rem_euclid(360.0).abs() > 0.001
            || rotation.y.rem_euclid(360.0).abs() > 0.001
            || rotation.z.rem_euclid(360.0).abs() > 0.001
        {
            return Err("set dimensions before rotating; exact world-axis sizing of rotated objects is not yet supported".into());
        }
        let current = selected.world_dimensions();
        let target = match args {
            [x, y, z] => Some(Vec3::new(
                parse_positive(x)?,
                parse_positive(y)?,
                parse_positive(z)?,
            )),
            _ => None,
        };
        self.checkpoint();
        let object = self.scene.selected_mut().ok_or("nothing selected")?;
        if let Some(target) = target {
            if current.x <= 0.0001 || current.y <= 0.0001 || current.z <= 0.0001 {
                return Err("all current dimensions must be non-zero".into());
            }
            object.transform.scale.x *= target.x / current.x;
            object.transform.scale.y *= target.y / current.y;
            object.transform.scale.z *= target.z / current.z;
            return Ok(format!(
                "Set size to {} × {} × {} mm",
                target.x, target.y, target.z
            ));
        }
        let [axis, raw] = args else {
            return Err("usage: size X Y Z | size x|y|z MM".into());
        };
        let value = parse_positive(raw)?;
        let (dimension, scale) = match *axis {
            "x" => (current.x, &mut object.transform.scale.x),
            "y" => (current.y, &mut object.transform.scale.y),
            "z" => (current.z, &mut object.transform.scale.z),
            _ => return Err("axis must be x, y, or z".into()),
        };
        if dimension <= 0.0001 {
            return Err("current dimension is zero".into());
        }
        *scale *= value / dimension;
        Ok(format!("Set {} size to {value} mm", axis.to_uppercase()))
    }

    fn command_bed(&mut self, args: &[&str]) -> Result<String, String> {
        let [x, y, z] = args else {
            return Err("usage: bed X Y Z (millimetres)".into());
        };
        let volume = Vec3::new(parse_positive(x)?, parse_positive(y)?, parse_positive(z)?);
        self.checkpoint();
        self.scene.build_volume = volume;
        Ok(format!(
            "Build volume: {} × {} × {} mm",
            volume.x, volume.y, volume.z
        ))
    }

    fn command_repair(&mut self) -> Result<String, String> {
        if self.scene.selected().is_none() {
            return Err("nothing selected".into());
        }
        self.checkpoint();
        let (vertices, triangles) =
            print::repair_mesh(&mut self.scene.selected_mut().expect("checked").mesh);
        Ok(format!(
            "Basic repair: removed {vertices} vertices and {triangles} triangles"
        ))
    }

    fn command_place(&mut self, center: bool) -> Result<String, String> {
        if self.scene.selected().is_none() {
            return Err("nothing selected".into());
        }
        self.checkpoint();
        let object = self.scene.selected_mut().expect("checked");
        if center {
            print::center_on_bed(object);
        } else {
            print::place_on_bed(object);
        }
        Ok(if center {
            "Centred selection on build plate"
        } else {
            "Placed selection on build plate"
        }
        .into())
    }

    fn command_mode(&mut self, args: &[&str]) -> Result<String, String> {
        self.scene.shading = match args.first().copied().unwrap_or_default() {
            "solid" => ShadingMode::Solid,
            "wire" | "wireframe" => ShadingMode::Wireframe,
            "xray" | "x-ray" => ShadingMode::XRay,
            _ => return Err("usage: mode solid|wire|xray".into()),
        };
        Ok(format!("{} shading", self.scene.shading.label()))
    }

    fn command_mirror(&mut self, args: &[&str]) -> Result<String, String> {
        let axis = args.first().copied().unwrap_or_default();
        if !matches!(axis, "x" | "y" | "z") {
            return Err("try: mirror x".into());
        }
        self.checkpoint();
        let scale = &mut self
            .scene
            .selected_mut()
            .ok_or("nothing selected")?
            .transform
            .scale;
        match axis {
            "x" => scale.x *= -1.0,
            "y" => scale.y *= -1.0,
            "z" => scale.z *= -1.0,
            _ => unreachable!(),
        }
        Ok(format!("Mirrored on {}", axis.to_uppercase()))
    }

    fn command_snap(&mut self, args: &[&str]) -> Result<String, String> {
        let value = args.first().copied().unwrap_or_default();
        self.snap = if value == "off" {
            0.0
        } else {
            value
                .parse::<f32>()
                .map_err(|_| "try: snap .25 or snap off".to_string())?
                .abs()
        };
        Ok(if self.snap == 0.0 {
            "Snapping off".into()
        } else {
            format!("Snap step: {}", self.snap)
        })
    }

    fn command_theme(&mut self, args: &[&str]) -> Result<String, String> {
        self.theme = ThemeChoice::parse(args.first().copied().unwrap_or_default())
            .ok_or("themes: midnight, amber, matrix, light, mono")?;
        Ok(format!("Theme: {}", self.theme.name()))
    }

    fn command_graphics(&mut self, args: &[&str]) -> Result<String, String> {
        self.graphics = GraphicsMode::parse(args.first().copied().unwrap_or_default())
            .ok_or("graphics modes: auto, kitty, sixel, ascii")?;
        Ok(format!(
            "Graphics: {} (active {})",
            self.graphics.label(),
            self.graphics.detect().label()
        ))
    }

    fn command_view(&mut self, args: &[&str]) -> Result<String, String> {
        let view = args.first().copied().unwrap_or_default();
        match view {
            "front" => {
                self.scene.camera.yaw = 0.0;
                self.scene.camera.pitch = 0.0;
            }
            "side" | "right" => {
                self.scene.camera.yaw = 90.0;
                self.scene.camera.pitch = 0.0;
            }
            "top" => {
                self.scene.camera.yaw = 0.0;
                self.scene.camera.pitch = 88.0;
            }
            "iso" => {
                self.scene.camera.yaw = -35.0;
                self.scene.camera.pitch = 24.0;
            }
            _ => return Err("views: front, side, top, iso".into()),
        }
        Ok(format!("{view} view"))
    }

    fn open_menu(&mut self, kind: MenuKind, x: u16, y: u16) {
        self.context_menu = Some(ContextMenu {
            kind,
            x,
            y,
            selected: 0,
        });
        self.overlay = Overlay::Context;
    }

    fn close_menu(&mut self) {
        self.context_menu = None;
        self.overlay = Overlay::None;
    }

    fn frame_selection(&mut self) {
        if let Some((position, name)) = self
            .scene
            .selected()
            .map(|object| (object.transform.position, object.name.clone()))
        {
            self.scene.camera.target = position;
            self.status = format!("Framed {name}");
        }
    }

    fn choose_tool(&mut self, tool: ToolKind) {
        self.active_tool = tool;
        match tool {
            ToolKind::Frame => self.frame_selection(),
            ToolKind::Check => self.status = print::validate(&self.scene).summary(),
            ToolKind::Select => {
                self.status = "Select · left-drag orbit · right/middle-drag pan".into()
            }
            ToolKind::Move => {
                self.status = "Move · left-drag object in view · right-drag its Z height".into()
            }
            ToolKind::Rotate => self.status = "Rotate · left-drag X/Z · right-drag Y/Z".into(),
            ToolKind::Scale => self.status = "Scale · left-drag uniform · right-drag Z".into(),
            ToolKind::Measure => self.status = "Measure · click two objects".into(),
        }
    }

    fn execute_menu_item(&mut self, index: usize) {
        let Some(menu) = self.context_menu else {
            return;
        };
        self.close_menu();
        match menu.kind {
            MenuKind::Object => match index {
                0 => self.prompt("move ", "Move: enter axis and distance, e.g. x 10"),
                1 => self.prompt("rotate ", "Rotate: enter axis and degrees, e.g. y 45"),
                2 => self.prompt("size ", "Exact size: enter X Y Z in mm"),
                3 => {
                    self.checkpoint();
                    self.scene.duplicate_selected();
                    self.status = "Duplicated selection".into();
                }
                4 => {
                    self.status = self
                        .command_place(false)
                        .unwrap_or_else(|error| format!("Error: {error}"))
                }
                5 => {
                    self.status = self
                        .command_place(true)
                        .unwrap_or_else(|error| format!("Error: {error}"))
                }
                6 => {
                    self.checkpoint();
                    if let Some(object) = self.scene.selected_mut() {
                        object.visible = !object.visible;
                    }
                }
                7 => self.status = print::validate(&self.scene).summary(),
                8 => {
                    self.status = self
                        .command_repair()
                        .unwrap_or_else(|error| format!("Error: {error}"))
                }
                9 => self.frame_selection(),
                10 => {
                    self.checkpoint();
                    if let Some(object) = self.scene.selected_mut() {
                        object.transform = Default::default();
                        print::place_on_bed(object);
                    }
                }
                11 => self.remove(),
                _ => {}
            },
            MenuKind::File => match index {
                0 => {
                    self.checkpoint();
                    self.scene = Scene::default();
                    self.path = None;
                }
                1 => self.prompt("open ", "Open file"),
                2 => {
                    if let Some(path) = self.path.clone() {
                        self.save_to(path);
                    } else {
                        self.open_save_dialog(SaveFormat::Forge);
                    }
                }
                3 => self.open_save_dialog(SaveFormat::Forge),
                4 => self.open_save_dialog(SaveFormat::ThreeMf),
                5 => {
                    self.status = formats::open_in_slicer(&self.scene)
                        .unwrap_or_else(|error| format!("Error: {error}"))
                }
                6 => self.quit = true,
                _ => {}
            },
            MenuKind::Edit => match index {
                0 => self.undo(),
                1 => self.redo(),
                2 => {
                    self.checkpoint();
                    self.scene.duplicate_selected();
                }
                3 => self.remove(),
                _ => {}
            },
            MenuKind::Add => {
                let kind = match index {
                    0 => Some(PrimitiveKind::Cube),
                    1 => Some(PrimitiveKind::Sphere),
                    2 => Some(PrimitiveKind::Cylinder),
                    3 => Some(PrimitiveKind::Cone),
                    4 => Some(PrimitiveKind::Torus),
                    5 => Some(PrimitiveKind::Wedge),
                    6 => Some(PrimitiveKind::Plane),
                    7 => Some(PrimitiveKind::Tube),
                    _ => None,
                };
                if let Some(kind) = kind {
                    self.add(kind);
                }
            }
            MenuKind::View => match index {
                0 => {
                    self.scene.camera.yaw = 0.0;
                    self.scene.camera.pitch = 0.0;
                }
                1 => {
                    self.scene.camera.yaw = 90.0;
                    self.scene.camera.pitch = 0.0;
                }
                2 => {
                    self.scene.camera.yaw = 0.0;
                    self.scene.camera.pitch = 88.0;
                }
                3 => {
                    self.scene.camera.yaw = -35.0;
                    self.scene.camera.pitch = 24.0;
                }
                4 => self.frame_selection(),
                5 => self.scene.camera.perspective = !self.scene.camera.perspective,
                6 => self.scene.shading = ShadingMode::Solid,
                7 => self.scene.shading = ShadingMode::Wireframe,
                8 => self.scene.shading = ShadingMode::XRay,
                9 => self.graphics = self.graphics.next(),
                10 => self.theme = self.theme.next(),
                _ => {}
            },
            MenuKind::Print => match index {
                0 => self.status = print::validate(&self.scene).summary(),
                1 => {
                    self.status = self
                        .command_repair()
                        .unwrap_or_else(|error| format!("Error: {error}"))
                }
                2 => {
                    self.status = self
                        .command_place(false)
                        .unwrap_or_else(|error| format!("Error: {error}"))
                }
                3 => {
                    self.status = self
                        .command_place(true)
                        .unwrap_or_else(|error| format!("Error: {error}"))
                }
                4 => self.prompt("bed ", "Build volume: X Y Z in mm"),
                5 => self.open_save_dialog(SaveFormat::Stl),
                6 => self.open_save_dialog(SaveFormat::ThreeMf),
                7 => {
                    self.status = formats::open_in_slicer(&self.scene)
                        .unwrap_or_else(|error| format!("Error: {error}"))
                }
                _ => {}
            },
            MenuKind::Update => match index {
                0 => self.start_update(false),
                1 => self.start_update(true),
                _ => {}
            },
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if self.startup {
            self.handle_project_key(key);
            return;
        }
        match self.overlay {
            Overlay::Command => self.handle_command_key(key),
            Overlay::Help => {
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
                ) {
                    self.overlay = Overlay::None;
                }
            }
            Overlay::Add => self.handle_add_key(key),
            Overlay::Context => self.handle_context_key(key),
            Overlay::SaveAs => self.handle_save_key(key),
            Overlay::None => self.handle_normal_key(key),
        }
    }

    fn handle_save_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.save_dialog = None;
            self.overlay = Overlay::None;
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            self.confirm_save_dialog();
            return;
        }
        let Some(focus) = self.save_dialog.as_ref().map(|dialog| dialog.focus) else {
            return;
        };
        if key.code == KeyCode::Tab {
            if let Some(dialog) = self.save_dialog.as_mut() {
                dialog.focus = dialog.focus.next();
            }
            return;
        }
        match focus {
            SaveFocus::Browse => match key.code {
                KeyCode::Up => {
                    if let Some(dialog) = self.save_dialog.as_mut() {
                        dialog.selected = dialog.selected.saturating_sub(1);
                    }
                }
                KeyCode::Down => {
                    if let Some(dialog) = self.save_dialog.as_mut() {
                        dialog.selected =
                            (dialog.selected + 1).min(dialog.entries.len().saturating_sub(1));
                    }
                }
                KeyCode::Enter | KeyCode::Right => self.activate_save_entry(),
                KeyCode::Left | KeyCode::Backspace => self.save_dialog_parent(),
                _ => {}
            },
            SaveFocus::Name => match key.code {
                KeyCode::Enter => self.confirm_save_dialog(),
                KeyCode::Backspace => {
                    if let Some(dialog) = self.save_dialog.as_mut() {
                        if dialog.filename_pristine {
                            dialog.filename.clear();
                            dialog.filename_pristine = false;
                        } else {
                            dialog.filename.pop();
                        }
                        dialog.overwrite = false;
                    }
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if !matches!(character, '/' | '\0') {
                        if let Some(dialog) = self.save_dialog.as_mut() {
                            if dialog.filename_pristine {
                                dialog.filename.clear();
                                dialog.filename_pristine = false;
                            }
                            dialog.filename.push(character);
                            dialog.overwrite = false;
                        }
                    }
                }
                _ => {}
            },
            SaveFocus::Format => match key.code {
                KeyCode::Left | KeyCode::Up => self.change_save_format(-1),
                KeyCode::Right | KeyCode::Down => self.change_save_format(1),
                KeyCode::Enter => {
                    if let Some(dialog) = self.save_dialog.as_mut() {
                        dialog.focus = SaveFocus::Name;
                    }
                }
                _ => {}
            },
            SaveFocus::Actions => match key.code {
                KeyCode::Left | KeyCode::Right => {
                    if let Some(dialog) = self.save_dialog.as_mut() {
                        dialog.save_action = !dialog.save_action;
                    }
                }
                KeyCode::Enter => {
                    if self
                        .save_dialog
                        .as_ref()
                        .is_some_and(|dialog| dialog.save_action)
                    {
                        self.confirm_save_dialog();
                    } else {
                        self.save_dialog = None;
                        self.overlay = Overlay::None;
                    }
                }
                _ => {}
            },
        }
    }

    fn handle_context_key(&mut self, key: KeyEvent) {
        let Some(mut menu) = self.context_menu else {
            self.close_menu();
            return;
        };
        let count = menu_items(menu.kind).len();
        match key.code {
            KeyCode::Esc => self.close_menu(),
            KeyCode::Up => {
                menu.selected = menu.selected.saturating_sub(1);
                self.context_menu = Some(menu);
            }
            KeyCode::Down => {
                menu.selected = (menu.selected + 1).min(count.saturating_sub(1));
                self.context_menu = Some(menu);
            }
            KeyCode::Enter => self.execute_menu_item(menu.selected),
            _ => {}
        }
    }

    fn handle_command_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                self.command.clear();
            }
            KeyCode::Enter => self.execute_command(),
            KeyCode::Backspace => {
                self.command.pop();
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.command.push(character)
            }
            _ => {}
        }
    }

    fn handle_add_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('c') => self.add(PrimitiveKind::Cube),
            KeyCode::Char('s') => self.add(PrimitiveKind::Sphere),
            KeyCode::Char('y') => self.add(PrimitiveKind::Cylinder),
            KeyCode::Char('o') => self.add(PrimitiveKind::Cone),
            KeyCode::Char('t') => self.add(PrimitiveKind::Torus),
            KeyCode::Char('w') => self.add(PrimitiveKind::Wedge),
            KeyCode::Char('p') => self.add(PrimitiveKind::Plane),
            KeyCode::Char('u') => self.add(PrimitiveKind::Tube),
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::None,
            _ => {}
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    if key.modifiers.contains(KeyModifiers::SHIFT) {
                        self.open_save_dialog(SaveFormat::Forge);
                    } else if let Some(path) = self.path.clone() {
                        self.save_to(path);
                    } else {
                        self.open_save_dialog(SaveFormat::Forge);
                    }
                }
                KeyCode::Char('r') => self.redo(),
                _ => {}
            }
            return;
        }
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            let step = if self.snap > 0.0 { self.snap } else { 0.25 };
            let delta = match key.code {
                KeyCode::Left => Some(Vec3::new(-step, 0.0, 0.0)),
                KeyCode::Right => Some(Vec3::new(step, 0.0, 0.0)),
                KeyCode::Up => Some(Vec3::new(0.0, step, 0.0)),
                KeyCode::Down => Some(Vec3::new(0.0, -step, 0.0)),
                KeyCode::PageUp => Some(Vec3::new(0.0, 0.0, step)),
                KeyCode::PageDown => Some(Vec3::new(0.0, 0.0, -step)),
                _ => None,
            };
            if let Some(delta) = delta {
                self.checkpoint();
                if let Some(object) = self.scene.selected_mut() {
                    object.transform.position += delta;
                }
                self.status = format!("Nudged by {step}");
                return;
            }
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            KeyCode::Char(':') => {
                self.overlay = Overlay::Command;
                self.command.clear();
            }
            KeyCode::Char('a') => self.overlay = Overlay::Add,
            KeyCode::Char('g') => self.prompt("move ", "Move: type axis and distance, e.g. x 10"),
            KeyCode::Char('r') => {
                self.prompt("rotate ", "Rotate: type axis and degrees, e.g. y 45")
            }
            KeyCode::Char('s') => self.prompt("scale ", "Scale: type factor, e.g. 2 or z .5"),
            KeyCode::Char('e') => self.open_save_dialog(SaveFormat::ThreeMf),
            KeyCode::Char('o') => self.prompt(
                "open ",
                "Open: type a .forge, .stl, .3mf, or .step filename",
            ),
            KeyCode::Char('d') => {
                self.checkpoint();
                self.scene.duplicate_selected();
                self.status = "Duplicated selection".into();
            }
            KeyCode::Char('t') => {
                self.theme = self.theme.next();
                self.status = format!("Theme: {}", self.theme.name());
            }
            KeyCode::Char('v') => {
                self.graphics = self.graphics.next();
                self.status = format!("Graphics: {}", self.graphics.label());
            }
            KeyCode::Char('u') => self.undo(),
            KeyCode::Delete | KeyCode::Backspace => self.remove(),
            KeyCode::Char('j') => self.scene.select_next(1),
            KeyCode::Char('k') => self.scene.select_next(-1),
            KeyCode::Left | KeyCode::Char('h') => self.scene.camera.yaw -= 5.0,
            KeyCode::Right | KeyCode::Char('l') => self.scene.camera.yaw += 5.0,
            KeyCode::Up => self.scene.camera.pitch = (self.scene.camera.pitch + 4.0).min(88.0),
            KeyCode::Down => self.scene.camera.pitch = (self.scene.camera.pitch - 4.0).max(-88.0),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.scene.camera.distance = (self.scene.camera.distance * 0.88).max(0.5)
            }
            KeyCode::Char('-') => {
                self.scene.camera.distance = (self.scene.camera.distance * 1.14).min(1000.0)
            }
            KeyCode::Char('p') | KeyCode::Char('5') => {
                self.scene.camera.perspective = !self.scene.camera.perspective
            }
            KeyCode::Char('1') => {
                self.scene.camera.yaw = 0.0;
                self.scene.camera.pitch = 0.0;
            }
            KeyCode::Char('3') => {
                self.scene.camera.yaw = 90.0;
                self.scene.camera.pitch = 0.0;
            }
            KeyCode::Char('7') => {
                self.scene.camera.yaw = 0.0;
                self.scene.camera.pitch = 88.0;
            }
            KeyCode::Char('0') => {
                self.scene.camera.yaw = -35.0;
                self.scene.camera.pitch = 24.0;
            }
            KeyCode::Char(' ') => {
                self.scene.shading = match self.scene.shading {
                    ShadingMode::Solid => ShadingMode::Wireframe,
                    ShadingMode::Wireframe => ShadingMode::XRay,
                    ShadingMode::XRay => ShadingMode::Solid,
                }
            }
            KeyCode::Char('f') => {
                if let Some(object) = self.scene.selected() {
                    self.scene.camera.target = object.transform.position;
                }
            }
            _ => {}
        }
    }

    fn prompt(&mut self, prefix: &str, guidance: &str) {
        self.overlay = Overlay::Command;
        self.command = prefix.into();
        self.status = guidance.into();
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.startup {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                if mouse.row == 1 && mouse.column < 10 {
                    self.scene = Scene::default();
                    self.path = None;
                    self.startup = false;
                } else if contains(self.regions.project_list, mouse.column, mouse.row) {
                    let visible = usize::from(self.regions.project_list.height.max(1));
                    let offset = self
                        .project_selected
                        .saturating_sub(visible.saturating_sub(1));
                    let index = offset + usize::from(mouse.row - self.regions.project_list.y);
                    if index < self.project_entries.len() {
                        self.project_selected = index;
                        self.activate_project();
                    }
                }
            } else if mouse.kind == MouseEventKind::ScrollDown {
                self.project_selected =
                    (self.project_selected + 1).min(self.project_entries.len().saturating_sub(1));
            } else if mouse.kind == MouseEventKind::ScrollUp {
                self.project_selected = self.project_selected.saturating_sub(1);
            }
            return;
        }
        if self.overlay == Overlay::SaveAs {
            self.handle_save_mouse(mouse);
            return;
        }
        if self.overlay == Overlay::Context {
            self.handle_context_mouse(mouse);
            return;
        }
        if self.overlay != Overlay::None {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scene.camera.distance = (self.scene.camera.distance * 0.88).max(0.5)
            }
            MouseEventKind::ScrollDown => {
                self.scene.camera.distance = (self.scene.camera.distance * 1.14).min(1000.0)
            }
            MouseEventKind::Down(button) => {
                if button == MouseButton::Left
                    && contains(self.regions.navbar, mouse.column, mouse.row)
                {
                    if let Some((kind, x)) = navbar_menu_at(self.regions.navbar, mouse.column) {
                        self.open_menu(kind, x, self.regions.navbar.bottom());
                    } else if navbar_help_at(self.regions.navbar, mouse.column) {
                        self.overlay = Overlay::Help;
                    }
                } else if button == MouseButton::Left
                    && contains(self.regions.toolbar, mouse.column, mouse.row)
                {
                    let index = mouse.row.saturating_sub(self.regions.toolbar.y + 1) as usize;
                    if let Some(tool) = ToolKind::ALL.get(index).copied() {
                        self.choose_tool(tool);
                    }
                } else if contains(self.regions.outliner, mouse.column, mouse.row) {
                    let index = mouse.row.saturating_sub(self.regions.outliner.y + 1) as usize;
                    if index < self.scene.objects.len() {
                        self.scene.selected = index;
                    }
                    if button == MouseButton::Right {
                        self.open_menu(MenuKind::Object, mouse.column, mouse.row);
                    }
                } else if contains(self.regions.viewport, mouse.column, mouse.row) {
                    let x = mouse.column.saturating_sub(self.regions.viewport.x) as usize;
                    let y = mouse.row.saturating_sub(self.regions.viewport.y) as usize;
                    let picked = self
                        .pick_frame
                        .as_ref()
                        .and_then(|frame| frame.object_at(x, y));
                    if let Some(index) = picked {
                        self.scene.selected = index;
                    }
                    self.drag = Some(DragState {
                        button,
                        column: mouse.column,
                        row: mouse.row,
                        moved: false,
                        checkpointed: false,
                        hit_object: picked.is_some(),
                    });
                }
            }
            MouseEventKind::Drag(button) => {
                if let Some(mut last) = self.drag.filter(|drag| drag.button == button) {
                    let dx = mouse.column as i32 - last.column as i32;
                    let dy = mouse.row as i32 - last.row as i32;
                    if dx == 0 && dy == 0 {
                        return;
                    }
                    last.moved = true;
                    let editing = self.scene.selected().is_some()
                        && matches!(
                            self.active_tool,
                            ToolKind::Move | ToolKind::Rotate | ToolKind::Scale
                        )
                        && button != MouseButton::Middle;
                    if editing {
                        if !last.checkpointed {
                            self.checkpoint();
                            last.checkpointed = true;
                        }
                        self.drag_transform(self.active_tool, button, dx as f32, dy as f32);
                    } else if button == MouseButton::Left {
                        self.scene.camera.yaw += dx as f32 * 1.5;
                        self.scene.camera.pitch =
                            (self.scene.camera.pitch - dy as f32 * 2.0).clamp(-88.0, 88.0);
                        self.status = "Orbit view".into();
                    } else {
                        self.pan_camera(dx as f32, dy as f32);
                    }
                    last.column = mouse.column;
                    last.row = mouse.row;
                    self.drag = Some(last);
                }
            }
            MouseEventKind::Up(button) => {
                let Some(drag) = self.drag.take().filter(|drag| drag.button == button) else {
                    return;
                };
                if !drag.moved {
                    match button {
                        MouseButton::Right => {
                            self.open_menu(MenuKind::Object, mouse.column, mouse.row)
                        }
                        MouseButton::Left if drag.hit_object => match self.active_tool {
                            ToolKind::Measure => self.measure_selected(),
                            ToolKind::Frame => self.frame_selection(),
                            ToolKind::Check => self.status = print::validate(&self.scene).summary(),
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn pan_camera(&mut self, dx: f32, dy: f32) {
        let scale = self.scene.camera.distance * 0.015;
        let (right, up) = self.camera_screen_axes();
        self.scene.camera.target += right * (-dx * scale) + up * (dy * scale);
        self.status = "Pan view".into();
    }

    fn drag_transform(&mut self, tool: ToolKind, button: MouseButton, dx: f32, dy: f32) {
        let sensitivity =
            self.scene.camera.distance / f32::from(self.regions.viewport.height.max(12));
        let (right, up) = self.camera_screen_axes();
        let snap = self.snap;
        let Some(object) = self.scene.selected_mut() else {
            return;
        };
        match (tool, button) {
            (ToolKind::Move, MouseButton::Left) => {
                object.transform.position += right * (dx * sensitivity) - up * (dy * sensitivity);
                if snap > 0.0 {
                    object.transform.position.x = snap_to(object.transform.position.x, snap);
                    object.transform.position.y = snap_to(object.transform.position.y, snap);
                    object.transform.position.z = snap_to(object.transform.position.z, snap);
                }
                self.status =
                    format!("Move · left-drag screen plane · right-drag Z · {snap:.2} mm snap");
            }
            (ToolKind::Move, MouseButton::Right) => {
                object.transform.position.z += -dy * sensitivity;
                if snap > 0.0 {
                    object.transform.position.z = snap_to(object.transform.position.z, snap);
                }
                self.status = format!("Move Z: {:.2} mm", object.transform.position.z);
            }
            (ToolKind::Rotate, MouseButton::Left) => {
                object.transform.rotation.z += dx * 2.0;
                object.transform.rotation.x += dy * 2.0;
                self.status = "Rotate · left-drag X/Z · right-drag Y/Z".into();
            }
            (ToolKind::Rotate, MouseButton::Right) => {
                object.transform.rotation.y += dx * 2.0;
                object.transform.rotation.z -= dy * 2.0;
                self.status = "Rotate · right-drag Y/Z".into();
            }
            (ToolKind::Scale, MouseButton::Left) => {
                let factor = ((dx - dy) * 0.035).clamp(-2.0, 2.0).exp();
                object.transform.scale = object.transform.scale * factor;
                self.status = format!("Uniform scale: {:.3}", object.transform.scale.x.abs());
            }
            (ToolKind::Scale, MouseButton::Right) => {
                let factor = (-dy * 0.035).clamp(-2.0, 2.0).exp();
                object.transform.scale.z *= factor;
                self.status = format!("Z scale: {:.3}", object.transform.scale.z.abs());
            }
            _ => {}
        }
    }

    fn camera_screen_axes(&self) -> (Vec3, Vec3) {
        let yaw = self.scene.camera.yaw.to_radians();
        let pitch = self.scene.camera.pitch.to_radians();
        let forward = Vec3::new(
            -yaw.sin() * pitch.cos(),
            -yaw.cos() * pitch.cos(),
            -pitch.sin(),
        );
        let right = Vec3::new(yaw.cos(), -yaw.sin(), 0.0);
        (right, forward.cross(right).normalized())
    }

    fn measure_selected(&mut self) {
        let Some(point) = self
            .scene
            .selected()
            .map(|object| object.transform.position)
        else {
            return;
        };
        if let Some(start) = self.measure_from.take() {
            self.status = format!("Distance: {:.3} mm", (point - start).length());
        } else {
            self.measure_from = Some(point);
            self.status = "Measure: click the second object".into();
        }
    }

    fn handle_context_mouse(&mut self, mouse: MouseEvent) {
        let Some(mut menu) = self.context_menu else {
            self.close_menu();
            return;
        };
        if contains(self.regions.context, mouse.column, mouse.row) {
            let index = mouse.row.saturating_sub(self.regions.context.y + 1) as usize;
            if index < menu_items(menu.kind).len() {
                menu.selected = index;
                self.context_menu = Some(menu);
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    self.execute_menu_item(index);
                }
            }
        } else if matches!(mouse.kind, MouseEventKind::Down(_)) {
            self.close_menu();
        }
    }

    fn handle_save_mouse(&mut self, mouse: MouseEvent) {
        if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) && contains(self.regions.save_list, mouse.column, mouse.row)
        {
            if let Some(dialog) = self.save_dialog.as_mut() {
                if mouse.kind == MouseEventKind::ScrollUp {
                    dialog.selected = dialog.selected.saturating_sub(1);
                } else {
                    dialog.selected =
                        (dialog.selected + 1).min(dialog.entries.len().saturating_sub(1));
                }
            }
            return;
        }
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        if contains(self.regions.save_parent, mouse.column, mouse.row) {
            self.save_dialog_parent();
        } else if contains(self.regions.save_list, mouse.column, mouse.row) {
            let row = mouse.row.saturating_sub(self.regions.save_list.y) as usize;
            if let Some(dialog) = self.save_dialog.as_mut() {
                let visible = usize::from(self.regions.save_list.height.max(1));
                let offset = dialog.selected.saturating_sub(visible.saturating_sub(1));
                let index = offset + row;
                if index < dialog.entries.len() {
                    dialog.selected = index;
                    dialog.focus = SaveFocus::Browse;
                }
            }
            self.activate_save_entry();
        } else if contains(self.regions.save_name, mouse.column, mouse.row) {
            if let Some(dialog) = self.save_dialog.as_mut() {
                dialog.focus = SaveFocus::Name;
            }
        } else if contains(self.regions.save_formats, mouse.column, mouse.row) {
            let relative = mouse.column.saturating_sub(self.regions.save_formats.x);
            let segment = (self.regions.save_formats.width / 4).max(1);
            let index = usize::from((relative / segment).min(3));
            if let Some(dialog) = self.save_dialog.as_mut() {
                dialog.format = SaveFormat::ALL[index];
                let stem = Path::new(&dialog.filename)
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("model");
                dialog.filename = format!("{stem}.{}", dialog.format.extension());
                dialog.filename_pristine = true;
                dialog.focus = SaveFocus::Format;
                dialog.overwrite = false;
            }
        } else if contains(self.regions.save_button, mouse.column, mouse.row) {
            self.confirm_save_dialog();
        } else if contains(self.regions.save_cancel, mouse.column, mouse.row) {
            self.save_dialog = None;
            self.overlay = Overlay::None;
        }
    }
}

fn contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x && x < area.right() && y >= area.y && y < area.bottom()
}

fn menu_items(kind: MenuKind) -> &'static [&'static str] {
    match kind {
        MenuKind::Object => &[
            "Move",
            "Rotate",
            "Set dimensions…",
            "Duplicate",
            "Place on bed",
            "Center on bed",
            "Hide / Show",
            "Validate model",
            "Repair mesh",
            "Frame selection",
            "Reset transform",
            "Delete",
        ],
        MenuKind::File => &[
            "New",
            "Open…",
            "Save",
            "Save As…",
            "Export Print File…",
            "Open in slicer",
            "Quit",
        ],
        MenuKind::Edit => &["Undo", "Redo", "Duplicate", "Delete"],
        MenuKind::Add => &[
            "Cube",
            "Sphere",
            "Cylinder",
            "Cone",
            "Torus",
            "Wedge",
            "Large plane",
            "Tube / pipe",
        ],
        MenuKind::View => &[
            "Front",
            "Side",
            "Top",
            "Isometric",
            "Frame selection",
            "Perspective",
            "Solid",
            "Wireframe",
            "X-Ray",
            "Cycle graphics",
            "Cycle theme",
        ],
        MenuKind::Print => &[
            "Validate model",
            "Repair selected",
            "Place on bed",
            "Center on bed",
            "Build volume…",
            "Export STL…",
            "Export 3MF…",
            "Open in slicer",
        ],
        MenuKind::Update => &["Check for updates", "Install from GitHub"],
    }
}

const NAV_MENUS: [(&str, MenuKind); 7] = [
    ("File", MenuKind::File),
    ("Edit", MenuKind::Edit),
    ("Add", MenuKind::Add),
    ("Object", MenuKind::Object),
    ("View", MenuKind::View),
    ("Print", MenuKind::Print),
    ("Update", MenuKind::Update),
];

fn navbar_menu_at(area: Rect, column: u16) -> Option<(MenuKind, u16)> {
    let mut x = area.x;
    for (label, kind) in NAV_MENUS {
        let width = label.len() as u16 + 2;
        if column >= x && column < x + width {
            return Some((kind, x));
        }
        x += width;
    }
    None
}

fn navbar_help_at(area: Rect, column: u16) -> bool {
    let start = area.x
        + NAV_MENUS
            .iter()
            .map(|(label, _)| label.len() as u16 + 2)
            .sum::<u16>();
    column >= start && column < start + 6
}

#[derive(Clone, Copy)]
enum VectorOperation {
    Move,
    Rotate,
}

impl VectorOperation {
    fn name(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Rotate => "rotate",
        }
    }
}

fn argument_path(input: &str) -> Option<PathBuf> {
    input
        .split_once(char::is_whitespace)
        .map(|(_, path)| PathBuf::from(path.trim_matches(|c| c == ' ' || c == '\"' || c == '\'')))
        .filter(|path| !path.as_os_str().is_empty())
}

fn parse_positive(value: &str) -> Result<f32, String> {
    let value = value
        .parse::<f32>()
        .map_err(|_| "value must be a number".to_string())?;
    if !value.is_finite() || value <= 0.0 {
        Err("value must be greater than zero".into())
    } else {
        Ok(value)
    }
}

fn snap_to(value: f32, step: f32) -> f32 {
    (value / step).round() * step
}

fn safe_file_stem(name: &str) -> String {
    let stem: String = name
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let stem = stem.trim_matches('_');
    if stem.is_empty() {
        "model".into()
    } else {
        stem.into()
    }
}

fn refresh_save_entries(dialog: &mut SaveDialog) {
    let result = fs::read_dir(&dialog.directory).map(|items| {
        let mut entries: Vec<_> = items
            .filter_map(std::result::Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let directory = path.is_dir();
                let supported_file = path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|extension| {
                        matches!(
                            extension.to_ascii_lowercase().as_str(),
                            "forge" | "3mf" | "stl" | "step" | "stp"
                        )
                    });
                (directory || supported_file).then(|| BrowserEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    path,
                    directory,
                })
            })
            .collect();
        entries.sort_by_key(|entry| (!entry.directory, entry.name.to_ascii_lowercase()));
        entries
    });
    match result {
        Ok(entries) => {
            dialog.entries = entries;
            dialog.selected = dialog.selected.min(dialog.entries.len().saturating_sub(1));
        }
        Err(error) => {
            dialog.entries.clear();
            dialog.message = format!("Cannot browse {}: {error}", dialog.directory.display());
        }
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
    if app.startup {
        draw_projects(frame, app);
        return;
    }
    let area = frame.area();
    let theme = app.theme.colors();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background).fg(theme.foreground)),
        area,
    );
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(8),
            Constraint::Length(if app.overlay == Overlay::Command {
                3
            } else {
                2
            }),
        ])
        .split(area);
    app.regions.outliner = Rect::default();
    app.regions.toolbar = Rect::default();
    draw_header(frame, app, root[0]);

    if area.width >= 110 {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(21),
                Constraint::Length(15),
                Constraint::Min(30),
                Constraint::Length(28),
            ])
            .split(root[1]);
        app.regions.outliner = body[0];
        app.regions.toolbar = body[1];
        draw_outliner(frame, app, body[0]);
        draw_toolbar(frame, app, body[1]);
        draw_viewport(frame, app, body[2]);
        draw_inspector(frame, app, body[3]);
    } else if area.width >= 70 {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(22),
                Constraint::Length(15),
                Constraint::Min(25),
            ])
            .split(root[1]);
        app.regions.outliner = body[0];
        app.regions.toolbar = body[1];
        draw_outliner(frame, app, body[0]);
        draw_toolbar(frame, app, body[1]);
        draw_viewport(frame, app, body[2]);
    } else {
        draw_viewport(frame, app, root[1]);
    }
    draw_status(frame, app, root[2]);

    match app.overlay {
        Overlay::Help => draw_popup(frame, theme, " Help ", HELP, 72, 31),
        Overlay::Add => draw_popup(
            frame,
            theme,
            " Add primitive ",
            "[c] Cube       [s] Sphere\n[y] Cylinder   [o] Cone\n[t] Torus      [w] Wedge\n[p] Large plane [u] Tube\n\n[Esc] Cancel",
            34,
            12,
        ),
        Overlay::Context => draw_context_menu(frame, app),
        Overlay::SaveAs => draw_save_dialog(frame, app),
        _ => {}
    }
}

fn draw_projects(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let theme = app.theme.colors();
    frame.render_widget(
        Block::default().style(Style::default().fg(theme.foreground).bg(theme.background)),
        area,
    );
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(4),
        Constraint::Length(2),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(format!(
            "  ◈ FORGETUI    Projects\n  [N] New   [Enter] Open   [←] Parent   [/] Filter: {}{}",
            app.project_filter,
            if app.project_filtering { "_" } else { "" }
        ))
        .style(Style::default().fg(theme.foreground).bg(theme.panel)),
        rows[0],
    );
    let entries: Vec<ListItem> = app
        .project_entries
        .iter()
        .map(|entry| {
            ListItem::new(format!(
                "  {}  {}",
                if entry.is_directory { "▸" } else { "◇" },
                entry.name
            ))
        })
        .collect();
    app.regions.project_list = rows[1].inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let visible = usize::from(app.regions.project_list.height.max(1));
    let offset = app
        .project_selected
        .saturating_sub(visible.saturating_sub(1));
    let mut state = ListState::default()
        .with_offset(offset)
        .with_selected((!entries.is_empty()).then_some(app.project_selected));
    let title = format!(" Projects · {} ", app.project_directory.display());
    frame.render_stateful_widget(
        List::new(entries)
            .block(panel_block(&title, theme))
            .highlight_symbol("▸")
            .highlight_style(Style::default().fg(theme.background).bg(theme.accent)),
        rows[1],
        &mut state,
    );
    frame.render_widget(
        Paragraph::new(format!("  {}", app.status))
            .style(Style::default().fg(theme.muted).bg(theme.panel)),
        rows[2],
    );
}

fn panel_block<'a>(title: &'a str, theme: Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.muted))
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
}

fn draw_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme.colors();
    let dirty = if app.dirty { "*" } else { "" };
    let file = app
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "Untitled.forge".into());
    let left = format!(" ◈ ForgeTUI   ☰   {file}{dirty}");
    let right = format!(
        " {} · {} · {} · {:.0}×{:.0}×{:.0} mm ",
        app.scene.shading.label(),
        if app.scene.camera.perspective {
            "PERSP"
        } else {
            "ORTHO"
        },
        app.graphics.detect().label(),
        app.scene.build_volume.x,
        app.scene.build_volume.y,
        app.scene.build_volume.z,
    );
    let fill = area.width.saturating_sub((left.len() + right.len()) as u16) as usize;
    let title_area = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(
        Paragraph::new(format!("{left}{}{right}", " ".repeat(fill))).style(
            Style::default()
                .bg(theme.panel)
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        ),
        title_area,
    );
    let nav_area = Rect::new(area.x, area.y + 1, area.width, 1);
    app.regions.navbar = nav_area;
    let mut spans = Vec::new();
    for (label, _) in NAV_MENUS {
        spans.push(Span::styled(
            format!(" {label} "),
            Style::default().fg(theme.foreground).bg(theme.panel),
        ));
    }
    spans.push(Span::styled(
        " Help ",
        Style::default().fg(theme.accent).bg(theme.panel),
    ));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.panel)),
        nav_area,
    );
}

fn draw_toolbar(frame: &mut Frame, app: &App, area: Rect) {
    let theme = app.theme.colors();
    let items: Vec<ListItem> = ToolKind::ALL
        .iter()
        .map(|tool| ListItem::new(format!(" {}", tool.label())))
        .collect();
    let selected = ToolKind::ALL
        .iter()
        .position(|tool| *tool == app.active_tool);
    let mut state = ListState::default().with_selected(selected);
    let list = List::new(items)
        .block(panel_block(" Tools ", theme))
        .highlight_symbol("▸")
        .highlight_style(
            Style::default()
                .fg(theme.background)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::default().fg(theme.foreground).bg(theme.background));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_outliner(frame: &mut Frame, app: &App, area: Rect) {
    let theme = app.theme.colors();
    let items: Vec<ListItem> = app
        .scene
        .objects
        .iter()
        .map(|object| {
            let icon = if object.visible { "◉" } else { "○" };
            ListItem::new(format!(" {icon}  {}", object.name))
        })
        .collect();
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new("\n  No features yet\n\n  Press A or use\n  Add in the menu")
                .block(panel_block(" Features ", theme))
                .style(Style::default().fg(theme.muted).bg(theme.background)),
            area,
        );
        return;
    }
    let mut state =
        ListState::default().with_selected((!items.is_empty()).then_some(app.scene.selected));
    let list = List::new(items)
        .block(panel_block(" Features ", theme))
        .highlight_symbol("▸")
        .highlight_style(
            Style::default()
                .fg(theme.background)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::default().fg(theme.foreground).bg(theme.background));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_viewport(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme.colors();
    let inner_width = area.width.saturating_sub(2);
    let inner_height = area.height.saturating_sub(2);
    let rendered = renderer::render_scene(&app.scene, inner_width, inner_height);
    let rows = if app.graphics.detect() == GraphicsMode::Ascii {
        rendered.rows.clone()
    } else {
        vec![" ".repeat(inner_width as usize); inner_height as usize]
    };
    let text = Text::from(rows.into_iter().map(Line::from).collect::<Vec<_>>());
    let title = format!(
        " Model · {} triangles · {} ",
        rendered.triangles,
        app.active_tool
            .label()
            .trim_start_matches(|character: char| !character.is_alphabetic())
    );
    frame.render_widget(
        Paragraph::new(text)
            .block(panel_block(&title, theme))
            .style(Style::default().fg(theme.accent).bg(theme.background)),
        area,
    );
    app.regions.viewport = Rect::new(area.x + 1, area.y + 1, inner_width, inner_height);
    app.pick_frame = Some(rendered);
}

fn draw_inspector(frame: &mut Frame, app: &App, area: Rect) {
    let theme = app.theme.colors();
    let text = if let Some(object) = app.scene.selected() {
        let p = object.transform.position;
        let r = object.transform.rotation;
        let s = object.transform.scale;
        let d = object.world_dimensions();
        format!(
            "{}\n{}\n\nDIMENSIONS (mm)\n  X {:>9.3}\n  Y {:>9.3}\n  Z {:>9.3}\n\nTRANSFORM\n  Position (mm)\n    X {:>8.3}\n    Y {:>8.3}\n    Z {:>8.3}\n  Rotation\n    X {:>8.2}°\n    Y {:>8.2}°\n    Z {:>8.2}°\n  Scale\n    X {:>8.3}\n    Y {:>8.3}\n    Z {:>8.3}\n\nMESH\n  {:>6} vertices\n  {:>6} triangles",
            object.name,
            object.kind.label(),
            d.x,
            d.y,
            d.z,
            p.x,
            p.y,
            p.z,
            r.x,
            r.y,
            r.z,
            s.x,
            s.y,
            s.z,
            object.mesh.vertices.len(),
            object.mesh.triangles.len()
        )
    } else {
        "No selection\n\nPress a to add an object.".into()
    };
    frame.render_widget(
        Paragraph::new(text)
            .block(panel_block(" Properties ", theme))
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let theme = app.theme.colors();
    if app.overlay == Overlay::Command {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    ":",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(&app.command),
            ]))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(theme.muted)),
            )
            .style(Style::default().bg(theme.panel)),
            area,
        );
        let cursor = area.x + 1 + app.command.chars().count() as u16;
        frame.set_cursor_position((cursor.min(area.right().saturating_sub(1)), area.y + 1));
    } else {
        let hint = " A Add  G Move  RMB Menu  ? Help ";
        let fill = area
            .width
            .saturating_sub((app.status.len() + hint.len()) as u16) as usize;
        frame.render_widget(
            Paragraph::new(format!(" {}{}{hint}", app.status, " ".repeat(fill)))
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(theme.muted)),
                )
                .style(Style::default().bg(theme.panel))
                .style(Style::default().fg(if app.status.starts_with("Error:") {
                    Color::Red
                } else {
                    theme.muted
                })),
            area,
        );
    }
}

fn draw_popup(
    frame: &mut Frame,
    theme: Theme,
    title: &str,
    content: &str,
    width: u16,
    height: u16,
) {
    let area = centered_rect(
        width.min(frame.area().width.saturating_sub(2)),
        height.min(frame.area().height.saturating_sub(2)),
        frame.area(),
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(content)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(theme.muted))
                    .title(title)
                    .title_alignment(Alignment::Center),
            )
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.foreground).bg(theme.panel)),
        area,
    );
}

fn draw_save_dialog(frame: &mut Frame, app: &mut App) {
    let Some(dialog) = app.save_dialog.as_ref() else {
        return;
    };
    let theme = app.theme.colors();
    let area = centered_rect(
        88.min(frame.area().width.saturating_sub(2)),
        32.min(frame.area().height.saturating_sub(2)),
        frame.area(),
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        panel_block(" Save As ", theme).style(Style::default().bg(theme.panel)),
        area,
    );
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(8),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(inner);

    let location = format!("  ↑ Parent    {}", dialog.directory.display());
    frame.render_widget(
        Paragraph::new(location).style(Style::default().fg(theme.foreground).bg(theme.background)),
        rows[0],
    );
    app.regions.save_parent = Rect::new(rows[0].x, rows[0].y, 12.min(rows[0].width), 1);

    let entries: Vec<ListItem> = dialog
        .entries
        .iter()
        .map(|entry| {
            let icon = if entry.directory { "▸" } else { "•" };
            ListItem::new(format!(
                " {icon} {}{}",
                entry.name,
                if entry.directory { "/" } else { "" }
            ))
        })
        .collect();
    let visible = usize::from(rows[1].height.saturating_sub(2).max(1));
    let offset = dialog.selected.saturating_sub(visible.saturating_sub(1));
    let mut state = ListState::default()
        .with_offset(offset)
        .with_selected((!entries.is_empty()).then_some(dialog.selected));
    let browse_title = if dialog.focus == SaveFocus::Browse {
        " Browse ● "
    } else {
        " Browse "
    };
    frame.render_stateful_widget(
        List::new(entries)
            .block(panel_block(browse_title, theme))
            .highlight_symbol("▸")
            .highlight_style(
                Style::default()
                    .fg(theme.background)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
            .style(Style::default().fg(theme.foreground).bg(theme.panel)),
        rows[1],
        &mut state,
    );
    app.regions.save_list = rows[1].inner(Margin {
        horizontal: 1,
        vertical: 1,
    });

    let name_title = if dialog.focus == SaveFocus::Name {
        " Name ● "
    } else {
        " Name "
    };
    frame.render_widget(
        Paragraph::new(format!(" {}", dialog.filename))
            .block(panel_block(name_title, theme))
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        rows[2],
    );
    app.regions.save_name = rows[2];

    let format_title = if dialog.focus == SaveFocus::Format {
        " Format ● "
    } else {
        " Format "
    };
    let format_width = rows[3].width.saturating_sub(2) / 4;
    let mut spans = Vec::new();
    for format in SaveFormat::ALL {
        let label = format!(
            " {:^width$} ",
            format.label(),
            width = format_width.saturating_sub(2) as usize
        );
        spans.push(Span::styled(
            label,
            if format == dialog.format {
                Style::default()
                    .fg(theme.background)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground).bg(theme.panel)
            },
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(panel_block(format_title, theme)),
        rows[3],
    );
    app.regions.save_formats = rows[3].inner(Margin {
        horizontal: 1,
        vertical: 1,
    });

    let message_color = if dialog.overwrite || dialog.message.starts_with("Error:") {
        Color::Yellow
    } else {
        theme.muted
    };
    frame.render_widget(
        Paragraph::new(dialog.message.as_str())
            .alignment(Alignment::Center)
            .style(Style::default().fg(message_color).bg(theme.panel)),
        rows[4],
    );
    let buttons = "[ Save ]    [ Cancel ]";
    let button_x = rows[5].x + rows[5].width.saturating_sub(buttons.len() as u16) / 2;
    let save_style = if dialog.focus == SaveFocus::Actions && dialog.save_action {
        Style::default()
            .fg(theme.background)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.foreground).bg(theme.panel)
    };
    let cancel_style = if dialog.focus == SaveFocus::Actions && !dialog.save_action {
        Style::default()
            .fg(theme.background)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.foreground).bg(theme.panel)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("[ Save ]", save_style),
            Span::raw("    "),
            Span::styled("[ Cancel ]", cancel_style),
        ])),
        Rect::new(button_x, rows[5].y, buttons.len() as u16, 1),
    );
    app.regions.save_button = Rect::new(button_x, rows[5].y, 8, 1);
    app.regions.save_cancel = Rect::new(button_x + 12, rows[5].y, 10, 1);

    if dialog.focus == SaveFocus::Name {
        let cursor = rows[2].x + 2 + dialog.filename.chars().count() as u16;
        frame.set_cursor_position((cursor.min(rows[2].right().saturating_sub(2)), rows[2].y + 1));
    }
}

fn draw_context_menu(frame: &mut Frame, app: &mut App) {
    let Some(menu) = app.context_menu else { return };
    let theme = app.theme.colors();
    let labels = menu_items(menu.kind);
    let width = 24u16.min(frame.area().width);
    let height = (labels.len() as u16 + 2).min(frame.area().height);
    let x = menu.x.min(frame.area().right().saturating_sub(width));
    let y = menu.y.min(frame.area().bottom().saturating_sub(height));
    let area = Rect::new(x, y, width, height);
    app.regions.context = area;
    let items: Vec<ListItem> = labels
        .iter()
        .map(|label| ListItem::new(format!("  {label}")))
        .collect();
    let mut state = ListState::default().with_selected(Some(menu.selected));
    let title = match menu.kind {
        MenuKind::Object => " Object ",
        MenuKind::File => " File ",
        MenuKind::Edit => " Edit ",
        MenuKind::Add => " Add ",
        MenuKind::View => " View ",
        MenuKind::Print => " Print ",
        MenuKind::Update => " Update ",
    };
    frame.render_widget(Clear, area);
    frame.render_stateful_widget(
        List::new(items)
            .block(panel_block(title, theme))
            .highlight_symbol("▸")
            .highlight_style(
                Style::default()
                    .fg(theme.background)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
            .style(Style::default().fg(theme.foreground).bg(theme.panel)),
        area,
        &mut state,
    );
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn demo_scene() -> Scene {
    let mut scene = Scene::default();
    scene.objects[0].transform.position = Vec3::new(-28.0, 0.0, 10.0);
    scene.add_primitive(PrimitiveKind::Sphere);
    scene.objects[1].transform.position = Vec3::new(28.0, 0.0, 10.0);
    scene.add_primitive(PrimitiveKind::Cylinder);
    scene.objects[2].transform.position = Vec3::new(0.0, 36.0, 14.0);
    scene.objects[2].transform.scale = Vec3::new(0.7, 1.4, 0.7);
    scene
}

fn print_help() {
    println!(
        "ForgeTUI — lightweight terminal 3D-printing CAD\n\nUSAGE:\n  forgetui [FILE]\n  forgetui --demo\n  forgetui --render [FILE]\n  forgetui --formats\n\nFILE FORMATS:\n{}\n\nInside the TUI, press ? for all controls.",
        formats::format_matrix()
    );
}

fn color_rgb(color: Color) -> [u8; 3] {
    match color {
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Black => [0, 0, 0],
        Color::White => [255, 255, 255],
        Color::DarkGray => [90, 90, 90],
        Color::Gray => [160, 160, 160],
        Color::Red | Color::LightRed => [230, 70, 70],
        Color::Green | Color::LightGreen => [70, 220, 100],
        Color::Blue | Color::LightBlue => [70, 130, 230],
        Color::Yellow | Color::LightYellow => [235, 205, 70],
        Color::Cyan | Color::LightCyan => [70, 220, 220],
        Color::Magenta | Color::LightMagenta => [210, 80, 210],
        _ => [180, 180, 180],
    }
}

fn render_pixel_overlay(
    backend: &mut CrosstermBackend<io::Stdout>,
    app: &mut App,
) -> io::Result<()> {
    if app.startup {
        return Ok(());
    }
    let mode = app.graphics.detect();
    if matches!(
        app.overlay,
        Overlay::Help | Overlay::Add | Overlay::Context | Overlay::SaveAs
    ) {
        return Ok(());
    }
    if mode != GraphicsMode::Kitty && app.kitty_image_id != 0 {
        backend.write_all(&graphics::kitty_delete(app.kitty_image_id))?;
        app.kitty_image_id = 0;
    }
    if mode == GraphicsMode::Ascii || app.regions.viewport.width == 0 {
        return Ok(());
    }
    let window = crossterm::terminal::window_size().ok();
    let cell_width = window
        .as_ref()
        .filter(|size| size.width > 0 && size.columns > 0)
        .map_or(8, |size| u32::from(size.width / size.columns).max(1));
    let cell_height = window
        .as_ref()
        .filter(|size| size.height > 0 && size.rows > 0)
        .map_or(16, |size| u32::from(size.height / size.rows).max(1));
    let theme = app.theme.colors();
    let image = pixel_renderer::render(
        &app.scene,
        u32::from(app.regions.viewport.width) * cell_width,
        u32::from(app.regions.viewport.height) * cell_height,
        PixelStyle {
            background: color_rgb(theme.background),
            accent: color_rgb(theme.accent),
            muted: color_rgb(theme.muted),
        },
    );
    backend.write_all(b"\x1b7")?;
    execute!(
        backend,
        MoveTo(app.regions.viewport.x, app.regions.viewport.y)
    )?;
    let previous_kitty_id = app.kitty_image_id;
    let payload = match mode {
        GraphicsMode::Kitty => {
            let image_id = if previous_kitty_id == 0 {
                std::process::id().saturating_mul(4096).saturating_add(1)
            } else {
                previous_kitty_id.wrapping_add(1).max(1)
            };
            app.kitty_image_id = image_id;
            graphics::kitty_image(
                &image.rgb,
                image.width,
                image.height,
                app.regions.viewport.width,
                app.regions.viewport.height,
                image_id,
            )
        }
        GraphicsMode::Sixel => graphics::sixel_image(&image.rgb, image.width, image.height),
        _ => Vec::new(),
    };
    backend.write_all(&payload)?;
    if mode == GraphicsMode::Kitty && previous_kitty_id != 0 {
        backend.write_all(&graphics::kitty_delete(previous_kitty_id))?;
    }
    backend.write_all(b"\x1b8")?;
    backend.flush()
}

fn process_event(app: &mut App, event: Event) -> bool {
    match event {
        Event::Key(key) => {
            let changed = key.kind == KeyEventKind::Press;
            app.handle_key(key);
            changed
        }
        Event::Mouse(mouse) => {
            let changed = !matches!(mouse.kind, MouseEventKind::Moved);
            app.handle_mouse(mouse);
            changed
        }
        Event::Resize(_, _) => true,
        _ => false,
    }
}

fn run_tui(mut app: App) -> Result<(), Box<dyn Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = (|| -> Result<(), Box<dyn Error>> {
        let mut redraw = true;
        loop {
            if redraw {
                terminal.draw(|frame| draw(frame, &mut app))?;
                render_pixel_overlay(terminal.backend_mut(), &mut app)?;
                redraw = false;
            }
            if app.quit {
                break;
            }
            if app.update_receiver.is_some() {
                if event::poll(Duration::from_millis(150))? {
                    redraw |= process_event(&mut app, event::read()?);
                }
                redraw |= app.poll_update();
            } else {
                redraw |= process_event(&mut app, event::read()?);
            }
            for _ in 0..64 {
                if !event::poll(Duration::ZERO)? {
                    break;
                }
                redraw |= process_event(&mut app, event::read()?);
            }
        }
        Ok(())
    })();
    disable_raw_mode()?;
    if app.kitty_image_id != 0 {
        terminal
            .backend_mut()
            .write_all(&graphics::kitty_delete(app.kitty_image_id))?;
    }
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        print_help();
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "--formats") {
        println!("{}", formats::format_matrix());
        return Ok(());
    }
    let render_only = args.first().is_some_and(|arg| arg == "--render");
    let demo = args.first().is_some_and(|arg| arg == "--demo");
    let file = if render_only {
        args.get(1)
    } else if demo {
        None
    } else {
        args.first()
    }
    .map(PathBuf::from);
    let scene = if demo {
        demo_scene()
    } else if let Some(path) = &file {
        formats::load(path)?
    } else {
        Scene::default()
    };

    if render_only || !io::stdout().is_terminal() {
        let rendered = renderer::render_scene(&scene, 100, 36);
        for row in rendered.rows {
            println!("{row}");
        }
        return Ok(());
    }
    let native_path =
        file.filter(|path| matches!(path.extension().and_then(|e| e.to_str()), Some("forge")));
    let mut app = App::new(scene, native_path);
    if !demo && args.is_empty() {
        app.startup = true;
        app.refresh_projects();
    }
    run_tui(app)
}

#[cfg(test)]
mod mouse_tests {
    use super::*;

    fn app_with_pickable_cube() -> (App, u16, u16) {
        let scene = Scene::default();
        let rendered = renderer::render_scene(&scene, 60, 30);
        let point = (0..30)
            .find_map(|y| {
                (0..60)
                    .find(|&x| rendered.object_at(x, y).is_some())
                    .map(|x| (x as u16, y as u16))
            })
            .expect("cube should be pickable");
        let mut app = App::new(scene, None);
        app.regions.viewport = Rect::new(0, 0, 60, 30);
        app.pick_frame = Some(rendered);
        (app, point.0, point.1)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn move_tool_click_selects_without_opening_command_prompt() {
        let (mut app, x, y) = app_with_pickable_cube();
        app.active_tool = ToolKind::Move;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x, y));
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.command.is_empty());
    }

    #[test]
    fn move_tool_left_drag_moves_and_right_click_still_opens_menu() {
        let (mut app, x, y) = app_with_pickable_cube();
        app.active_tool = ToolKind::Move;
        let before = app.scene.selected().unwrap().transform.position;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), x + 3, y));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x + 3, y));
        assert_ne!(app.scene.selected().unwrap().transform.position, before);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), x, y));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Right), x, y));
        assert_eq!(app.overlay, Overlay::Context);
    }

    #[test]
    fn manipulation_tools_change_their_selected_transform() {
        let (mut move_app, x, y) = app_with_pickable_cube();
        move_app.active_tool = ToolKind::Move;
        let old_z = move_app.scene.selected().unwrap().transform.position.z;
        move_app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), x, y));
        move_app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Right),
            x,
            y.saturating_add(2),
        ));
        move_app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Right),
            x,
            y.saturating_add(2),
        ));
        assert_ne!(
            move_app.scene.selected().unwrap().transform.position.z,
            old_z
        );
        assert_eq!(move_app.overlay, Overlay::None);
        let first_z = move_app.scene.selected().unwrap().transform.position.z;
        move_app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), 0, 0));
        move_app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Right), 0, 2));
        move_app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Right), 0, 2));
        assert_ne!(
            move_app.scene.selected().unwrap().transform.position.z,
            first_z,
            "move must work again without re-picking the mesh"
        );

        let (mut rotate_app, x, y) = app_with_pickable_cube();
        rotate_app.active_tool = ToolKind::Rotate;
        rotate_app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
        rotate_app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), x + 2, y));
        rotate_app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x + 2, y));
        let first_rotation = rotate_app.scene.selected().unwrap().transform.rotation.z;
        rotate_app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        rotate_app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 2, 0));
        rotate_app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, 0));
        assert_ne!(
            rotate_app.scene.selected().unwrap().transform.rotation.z,
            first_rotation,
            "rotate must work repeatedly"
        );

        let (mut scale_app, x, y) = app_with_pickable_cube();
        scale_app.active_tool = ToolKind::Scale;
        scale_app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
        scale_app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), x + 2, y));
        scale_app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x + 2, y));
        let first_scale = scale_app.scene.selected().unwrap().transform.scale.x;
        scale_app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        scale_app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 2, 0));
        scale_app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, 0));
        assert_ne!(
            scale_app.scene.selected().unwrap().transform.scale.x,
            first_scale,
            "scale must work repeatedly"
        );
    }

    #[test]
    fn save_dialog_changes_format_and_saves_native_project() {
        let mut app = App::new(Scene::default(), None);
        app.open_save_dialog(SaveFormat::Forge);
        assert_eq!(app.overlay, Overlay::SaveAs);
        app.change_save_format(1);
        assert!(app.save_dialog.as_ref().unwrap().filename.ends_with(".3mf"));

        let filename = format!("forgetui-save-dialog-{}.forge", std::process::id());
        let path = std::env::temp_dir().join(&filename);
        if path.exists() {
            fs::remove_file(&path).unwrap();
        }
        let dialog = app.save_dialog.as_mut().unwrap();
        dialog.directory = std::env::temp_dir();
        dialog.filename = filename;
        dialog.format = SaveFormat::Forge;
        app.confirm_save_dialog();
        assert!(path.exists());
        assert_eq!(app.overlay, Overlay::None);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn save_dialog_requires_second_confirmation_to_overwrite() {
        let filename = format!("forgetui-overwrite-{}.forge", std::process::id());
        let path = std::env::temp_dir().join(&filename);
        fs::write(&path, "existing").unwrap();
        let mut app = App::new(Scene::default(), None);
        app.open_save_dialog(SaveFormat::Forge);
        let dialog = app.save_dialog.as_mut().unwrap();
        dialog.directory = std::env::temp_dir();
        dialog.filename = filename;
        app.confirm_save_dialog();
        assert_eq!(app.overlay, Overlay::SaveAs);
        assert!(app.save_dialog.as_ref().unwrap().overwrite);
        app.confirm_save_dialog();
        assert_eq!(app.overlay, Overlay::None);
        fs::remove_file(path).unwrap();
    }
}
