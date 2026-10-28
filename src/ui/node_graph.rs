//! Node graph overlay over the viewport.
//!
//! The graph itself — nodes, links, evaluation, the operation library — is the
//! `graph` crate. This module is its canvas: the palette, the Inputs area, the
//! node sequence of steps and the value picker. There are no wires: every input of
//! a step takes its value from an earlier step or from the Inputs area through
//! the picker, and the Inputs area is the only place values are typed by hand.
//! Nodes can be gathered into named, coloured, collapsible groups, and every
//! structural edit goes on the graph's own undo history.
//! Object nodes own drawing entities, and their rows are the Properties
//! panel's own rows for that entity (built and written back in
//! `app::node_graph`); operation rows come from the node's spec.

use codec::{EntityType, Handle};
use std::sync::Arc;

use graph::docs::{PortType, Type};
use graph::script::Script;
use graph::{Composite, Kind, Link, NodeId, Port, Spec};
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, Widget};
use iced::advanced::{overlay, renderer, Renderer as _, Shell};
use iced::widget::{
    button, center, checkbox, column, container, mouse_area, opaque, pick_list, row, scrollable,
    slider, stack, text, text_input, tooltip, Space,
};
use iced::{
    mouse, Background, Border, Color, Element, Length, Point, Rectangle, Shadow, Size, Theme,
    Transformation, Vector,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use serde_json::{json, Value};

use crate::app::Message;
use crate::t;

pub const NODE_W: f32 = 300.0;
const INPUTS_W: f32 = 290.0;
const LABEL_W: f32 = 96.0;
const PICKER_W: f32 = 440.0;
/// The value picker: inputs on the left, earlier steps on the right.
const PICKER_TWO_W: f32 = 780.0;
const ZOOM_STEP: f32 = 1.15;
const ZOOM_STEPS: std::ops::RangeInclusive<i32> = -8..=8;
/// The `NodeUi::open` key of a step's computed-values group; no section
/// title can clash with it.
const COMPUTED_SECTION: &str = "\u{1}computed";
/// Undo steps the graph keeps.
const HISTORY: usize = 100;
/// Text inputs the app focuses when the picker or a rename opens.
pub const PICK_SEARCH_ID: &str = "node-graph-pick-search";
pub const RENAME_ID: &str = "node-graph-rename";
pub const NODE_SEARCH_ID: &str = "node-graph-node-search";
/// The scrollables of the node sequence and of the Inputs area, so a
/// found node can be brought into view.
pub const SEQUENCE_SCROLL_ID: &str = "node-graph-sequence";
pub const INPUTS_SCROLL_ID: &str = "node-graph-inputs";
/// Width of a step in the compact view.
const COMPACT_W: f32 = 200.0;
/// Section key of a slider's range rows.
const RANGE_SECTION: &str = "\u{1}range";
/// The `format` of copied nodes on the clipboard.
const CLIP_FORMAT: &str = "ocg-nodes";

/// Operations that live in the Inputs area: the only nodes whose values are
/// typed by hand.
const INPUT_KINDS: &[&str] = &[
    "Number",
    "Number Slider",
    "Integer Slider",
    "Text",
    "Boolean",
    "Table",
];

/// Whether `kind` is an Inputs-area node rather than a step of the node sequence.
pub fn is_input(kind: &Kind) -> bool {
    matches!(kind, Kind::Op(spec) if INPUT_KINDS.contains(&spec.name))
}

/// Drawing objects a node can create.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    Line,
    Circle,
    Arc,
    Ellipse,
    Point,
    Polyline,
    Ray,
    XLine,
    Text,
    MText,
    /// Draws curve values the graph computes: an object node whose
    /// entities follow its inputs rather than its own properties.
    Draw,
    /// Fills closed curves with a hatch pattern.
    Hatch,
    /// Places references of a block.
    Block,
    /// Places aligned dimensions between two points.
    Dimension,
    /// Draws a table of rows as lines and texts.
    Table,
    /// Writes a text along a curve, following its direction.
    Label,
    /// Writes data onto drawn objects it does not own.
    Tag,
}

pub const OBJECTS: &[ObjectKind] = &[
    ObjectKind::Line,
    ObjectKind::Circle,
    ObjectKind::Arc,
    ObjectKind::Ellipse,
    ObjectKind::Point,
    ObjectKind::Polyline,
    ObjectKind::Ray,
    ObjectKind::XLine,
    ObjectKind::Text,
    ObjectKind::MText,
    ObjectKind::Draw,
    ObjectKind::Hatch,
    ObjectKind::Block,
    ObjectKind::Dimension,
    ObjectKind::Table,
    ObjectKind::Label,
    ObjectKind::Tag,
];

impl ObjectKind {
    /// Every kind, in palette order.
    pub const ALL: &'static [ObjectKind] = OBJECTS;

    /// Display name, also the graph's object type name.
    pub fn name(self) -> &'static str {
        match self {
            ObjectKind::Line => "Line",
            ObjectKind::Circle => "Circle",
            ObjectKind::Arc => "Arc",
            ObjectKind::Ellipse => "Ellipse",
            ObjectKind::Point => "Point",
            ObjectKind::Polyline => "Polyline",
            ObjectKind::Ray => "Ray",
            ObjectKind::XLine => "Construction Line",
            ObjectKind::Text => "Text",
            ObjectKind::MText => "Multiline Text",
            ObjectKind::Draw => "Draw Geometry",
            ObjectKind::Hatch => "Hatch",
            ObjectKind::Block => "Insert Block",
            ObjectKind::Dimension => "Aligned Dimension",
            ObjectKind::Table => "Draw Table",
            ObjectKind::Label => "Curve Label",
            ObjectKind::Tag => "Tag Objects",
        }
    }

    /// What the node does, for its tooltip.
    pub fn description(self) -> &'static str {
        match self {
            ObjectKind::Line => "Draws a line and keeps it in step with its inputs. Its rows are the line's properties.",
            ObjectKind::Circle => "Draws a circle and keeps it in step with its inputs. Its rows are the circle's properties.",
            ObjectKind::Arc => "Draws an arc and keeps it in step with its inputs. Its rows are the arc's properties.",
            ObjectKind::Ellipse => "Draws an ellipse and keeps it in step with its inputs. Its rows are the ellipse's properties.",
            ObjectKind::Point => "Draws a point object and keeps it in step with its inputs.",
            ObjectKind::Polyline => "Draws a polyline and keeps it in step with its inputs. Its rows are the polyline's properties.",
            ObjectKind::Ray => "Draws a ray, a line endless in one direction.",
            ObjectKind::XLine => "Draws a construction line, endless in both directions.",
            ObjectKind::Text => "Draws a single-line text: contents, position, height and rotation can all come from other nodes.",
            ObjectKind::MText => "Draws a multiline text and keeps it in step with its inputs.",
            ObjectKind::Draw => "Draws every curve it receives, on a layer and in a colour, and keeps the drawing in step with them.",
            ObjectKind::Hatch => "Fills closed curves with a hatch: one hatch per region, a list holding an outline and its holes. SOLID fills; any other name is a hatch pattern, at a scale and an angle in degrees.",
            ObjectKind::Block => "Places a block of the drawing at each point, turned by an angle in degrees and scaled: doors, windows, trees or posts along an axis. A block the drawing lacks places nothing.",
            ObjectKind::Dimension => "Places an aligned dimension between two points, its line an offset to their left; the text, when given, replaces the measurement.",
            ObjectKind::Table => "Draws rows of cells as a table of lines and texts at a point: a schedule of areas, stations or openings. Columns fit their texts unless a width is given.",
            ObjectKind::Label => "Writes a text on each curve, at a distance along it (the middle when unset) and an offset to its left, turned to follow the curve and kept readable.",
            ObjectKind::Tag => "Writes data onto drawn objects, such as kind=door; segment=A, without owning them, so Object Data and Layer Objects can sort them later.",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        OBJECTS.iter().copied().find(|kind| kind.name() == name)
    }

    /// The kind of a drawn entity, when a node can own it.
    pub fn of(entity: &EntityType) -> Option<Self> {
        OBJECTS
            .iter()
            .copied()
            .find(|kind| !kind.places() && kind.matches(entity))
    }

    /// Whether the node's entities come from its inputs (geometry, points,
    /// a block name…) rather than from properties of its own.
    pub fn places(self) -> bool {
        matches!(
            self,
            ObjectKind::Draw
                | ObjectKind::Hatch
                | ObjectKind::Block
                | ObjectKind::Dimension
                | ObjectKind::Table
                | ObjectKind::Label
                | ObjectKind::Tag
        )
    }

    /// Whether `entity` is this kind of object.
    pub fn matches(self, entity: &EntityType) -> bool {
        matches!(
            (self, entity),
            (ObjectKind::Line, EntityType::Line(_))
                | (ObjectKind::Circle, EntityType::Circle(_))
                | (ObjectKind::Arc, EntityType::Arc(_))
                | (ObjectKind::Ellipse, EntityType::Ellipse(_))
                | (ObjectKind::Point, EntityType::Point(_))
                | (ObjectKind::Polyline, EntityType::LwPolyline(_))
                | (ObjectKind::Ray, EntityType::Ray(_))
                | (ObjectKind::XLine, EntityType::XLine(_))
                | (ObjectKind::Text, EntityType::Text(_))
                | (ObjectKind::MText, EntityType::MText(_))
                | (
                    ObjectKind::Draw,
                    EntityType::Line(_)
                        | EntityType::Circle(_)
                        | EntityType::Arc(_)
                        | EntityType::LwPolyline(_)
                        | EntityType::Ray(_)
                        | EntityType::XLine(_)
                )
                | (ObjectKind::Hatch, EntityType::Hatch(_))
                | (ObjectKind::Block, EntityType::Insert(_))
                | (ObjectKind::Dimension, EntityType::Dimension(_))
                | (ObjectKind::Table, EntityType::Line(_) | EntityType::Text(_))
                | (ObjectKind::Label, EntityType::Text(_))
        )
    }

    fn command(self) -> &'static str {
        match self {
            ObjectKind::Line => "LINE",
            ObjectKind::Circle => "CIRCLE",
            ObjectKind::Arc => "ARC",
            ObjectKind::Ellipse => "ELLIPSE",
            ObjectKind::Point => "POINT",
            ObjectKind::Polyline => "PLINE",
            ObjectKind::Ray => "RAY",
            ObjectKind::XLine => "XLINE",
            ObjectKind::Text => "TEXT",
            ObjectKind::MText => "MTEXT",
            ObjectKind::Draw => "",
            ObjectKind::Hatch => "HATCH",
            ObjectKind::Block => "INSERT",
            ObjectKind::Dimension => "DIMALIGNED",
            ObjectKind::Table | ObjectKind::Label | ObjectKind::Tag => "",
        }
    }

    /// The entity a new node starts from.
    pub fn entity(self) -> EntityType {
        use codec::entities as e;
        use codec::types::{Vector2, Vector3};
        let origin = Vector3::new(0.0, 0.0, 0.0);
        let x_axis = Vector3::new(1.0, 0.0, 0.0);
        match self {
            ObjectKind::Line => {
                EntityType::Line(e::Line::from_coords(0.0, 0.0, 0.0, 10.0, 0.0, 0.0))
            }
            ObjectKind::Circle => EntityType::Circle(e::Circle::from_coords(0.0, 0.0, 0.0, 5.0)),
            ObjectKind::Arc => EntityType::Arc(e::Arc::from_coords(
                0.0,
                0.0,
                0.0,
                5.0,
                0.0,
                std::f64::consts::PI,
            )),
            ObjectKind::Ellipse => EntityType::Ellipse(e::Ellipse::from_center_axes(
                origin,
                Vector3::new(10.0, 0.0, 0.0),
                0.5,
            )),
            ObjectKind::Point => EntityType::Point(e::Point::from_coords(0.0, 0.0, 0.0)),
            ObjectKind::Polyline => EntityType::LwPolyline(e::LwPolyline::from_points(vec![
                Vector2::new(0.0, 0.0),
                Vector2::new(10.0, 0.0),
                Vector2::new(10.0, 10.0),
            ])),
            ObjectKind::Ray => EntityType::Ray(e::Ray::new(origin, x_axis)),
            ObjectKind::XLine => EntityType::XLine(e::XLine::new(origin, x_axis)),
            ObjectKind::Text => {
                EntityType::Text(e::Text::with_value("Text", origin).with_height(2.5))
            }
            ObjectKind::MText => EntityType::MText(Box::new(
                e::MText::with_value("MText", origin).with_height(2.5),
            )),
            // These nodes' entities come from their inputs.
            ObjectKind::Draw
            | ObjectKind::Hatch
            | ObjectKind::Block
            | ObjectKind::Dimension
            | ObjectKind::Table
            | ObjectKind::Label
            | ObjectKind::Tag => {
                EntityType::Line(e::Line::from_coords(0.0, 0.0, 0.0, 0.0, 0.0, 0.0))
            }
        }
    }
}

/// What a palette entry adds.
#[derive(Clone, Copy, Debug)]
pub enum PaletteItem {
    Object(ObjectKind),
    Op(&'static Spec),
    /// An entry of the composite library, by position.
    Composite(usize),
    /// A script node from the starting template.
    NewScript,
    /// An entry of the script library, by position.
    Script(usize),
}

impl PaletteItem {
    /// The entry's name as the palette shows it.
    fn title(self) -> String {
        match self {
            PaletteItem::Object(kind) => t!(kind.name()).into_owned(),
            PaletteItem::Op(spec) => t!(spec.name).into_owned(),
            PaletteItem::Composite(k) => composite_library()
                .get(k)
                .map(|composite| composite.name.clone())
                .unwrap_or_default(),
            PaletteItem::NewScript => t!("New script").into_owned(),
            PaletteItem::Script(k) => script_library()
                .get(k)
                .map(|script| script.name.clone())
                .unwrap_or_default(),
        }
    }

    /// The node kind it adds; `None` for a library entry that is gone.
    pub fn kind(self) -> Option<Kind> {
        Some(match self {
            PaletteItem::Object(kind) => Kind::Object(kind.name().to_owned()),
            PaletteItem::Op(spec) => Kind::Op(spec),
            PaletteItem::Composite(k) => Kind::Composite(composite_library().get(k)?.clone()),
            PaletteItem::NewScript => Kind::Script(Script::new(
                t!("Script").into_owned(),
                graph::script::TEMPLATE,
            )),
            PaletteItem::Script(k) => {
                let saved = script_library().get(k)?.clone();
                Kind::Script(Script::new(saved.name.clone(), saved.source.clone()))
            }
        })
    }
}

// ── Composite library ─────────────────────────────────────────────────────
// Composites the user saved, offered in the palette for every drawing:
// a JSON list in the config folder (local storage on the web).

const LIBRARY_FILE: &str = "node_composites.json";
#[cfg(target_arch = "wasm32")]
const LIBRARY_KEY: &str = "opencadstudio.node-composites";

fn library_store() -> &'static std::sync::Mutex<Vec<Arc<Composite>>> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<Vec<Arc<Composite>>>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| {
        #[cfg(not(target_arch = "wasm32"))]
        let body = crate::config::config_dir()
            .and_then(|dir| std::fs::read_to_string(dir.join(LIBRARY_FILE)).ok());
        #[cfg(target_arch = "wasm32")]
        let body = web_sys::window()
            .and_then(|window| window.local_storage().ok().flatten())
            .and_then(|storage| storage.get_item(LIBRARY_KEY).ok().flatten());
        let items = body
            .and_then(|body| serde_json::from_str::<Vec<Value>>(&body).ok())
            .unwrap_or_default()
            .iter()
            .filter_map(|item| Composite::from_json(item).ok().map(Arc::new))
            .collect();
        std::sync::Mutex::new(items)
    })
}

fn persist_library(items: &[Arc<Composite>]) {
    let body = Value::Array(items.iter().map(|item| item.to_json()).collect()).to_string();
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(dir) = crate::config::config_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(LIBRARY_FILE), body);
    }
    #[cfg(target_arch = "wasm32")]
    if let Some(storage) =
        web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    {
        let _ = storage.set_item(LIBRARY_KEY, &body);
    }
}

/// The saved composites, in the order the palette lists them.
pub fn composite_library() -> Vec<Arc<Composite>> {
    library_store()
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default()
}

/// The saved composite called `name`, ignoring case.
pub fn library_composite(name: &str) -> Option<Arc<Composite>> {
    let wanted = name.trim().to_lowercase();
    composite_library()
        .into_iter()
        .find(|composite| composite.name.to_lowercase() == wanted)
}

/// Saves `composite` to the library, replacing one of the same name.
pub fn save_to_library(composite: Arc<Composite>) {
    if let Ok(mut items) = library_store().lock() {
        let name = composite.name.to_lowercase();
        match items
            .iter()
            .position(|item| item.name.to_lowercase() == name)
        {
            Some(k) => items[k] = composite,
            None => items.push(composite),
        }
        persist_library(&items);
    }
}

/// The library as a file: the same JSON list it is stored as.
pub fn library_file() -> Vec<u8> {
    Value::Array(
        composite_library()
            .iter()
            .map(|item| item.to_json())
            .collect(),
    )
    .to_string()
    .into_bytes()
}

/// Adds the composites of a library file, replacing those of the same
/// names; how many it held, or why it is not a library.
pub fn import_library(bytes: &[u8]) -> Result<usize, String> {
    let items: Vec<Value> = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let composites: Vec<Composite> = items
        .iter()
        .map(Composite::from_json)
        .collect::<Result<_, _>>()?;
    let count = composites.len();
    for composite in composites {
        save_to_library(Arc::new(composite));
    }
    Ok(count)
}

/// Drops library entry `k`.
pub fn forget_library_entry(k: usize) {
    if let Ok(mut items) = library_store().lock() {
        if k < items.len() {
            items.remove(k);
            persist_library(&items);
        }
    }
}

// ── Script library and trusted scripts ───────────────────────────────────
// Scripts the user saved, offered in the palette like composites, and the
// fingerprints of the scripts of opened files the user chose to run.

const SCRIPTS_FILE: &str = "node_scripts.json";
const TRUSTED_FILE: &str = "node_trusted_scripts.json";

/// The text kept under `file` in the config folder (local storage on the
/// web).
fn read_store(file: &str) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    return crate::config::config_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join(file)).ok());
    #[cfg(target_arch = "wasm32")]
    return web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| {
            storage
                .get_item(&format!("opencadstudio.{file}"))
                .ok()
                .flatten()
        });
}

fn write_store(file: &str, body: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(dir) = crate::config::config_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(file), body);
    }
    #[cfg(target_arch = "wasm32")]
    if let Some(storage) =
        web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    {
        let _ = storage.set_item(&format!("opencadstudio.{file}"), body);
    }
}

fn script_store() -> &'static std::sync::Mutex<Vec<Arc<Script>>> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<Vec<Arc<Script>>>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| {
        let items = read_store(SCRIPTS_FILE)
            .and_then(|body| serde_json::from_str::<Vec<Value>>(&body).ok())
            .unwrap_or_default()
            .iter()
            .filter_map(|item| Script::from_json(item).ok())
            .collect();
        std::sync::Mutex::new(items)
    })
}

/// The saved scripts, in the order the palette lists them.
pub fn script_library() -> Vec<Arc<Script>> {
    script_store()
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default()
}

/// Saves `script` to the library, replacing one of the same name.
pub fn save_script(script: Arc<Script>) {
    if let Ok(mut items) = script_store().lock() {
        let name = script.name.to_lowercase();
        match items
            .iter()
            .position(|item| item.name.to_lowercase() == name)
        {
            Some(k) => items[k] = script,
            None => items.push(script),
        }
        let body = Value::Array(items.iter().map(|item| item.to_json()).collect()).to_string();
        write_store(SCRIPTS_FILE, &body);
    }
}

/// Drops script library entry `k`.
pub fn forget_script(k: usize) {
    if let Ok(mut items) = script_store().lock() {
        if k < items.len() {
            items.remove(k);
            let body = Value::Array(items.iter().map(|item| item.to_json()).collect()).to_string();
            write_store(SCRIPTS_FILE, &body);
        }
    }
}

fn trusted_store() -> &'static std::sync::Mutex<Vec<u64>> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<Vec<u64>>> = std::sync::OnceLock::new();
    STORE.get_or_init(|| {
        let items = read_store(TRUSTED_FILE)
            .and_then(|body| serde_json::from_str(&body).ok())
            .unwrap_or_default();
        std::sync::Mutex::new(items)
    })
}

/// Whether the user chose to run scripts with this fingerprint before.
pub fn scripts_trusted(fingerprint: u64) -> bool {
    trusted_store()
        .lock()
        .is_ok_and(|items| items.contains(&fingerprint))
}

/// Remembers that the user runs scripts with this fingerprint.
pub fn trust_scripts(fingerprint: u64) {
    if let Ok(mut items) = trusted_store().lock() {
        if !items.contains(&fingerprint) {
            items.push(fingerprint);
            write_store(
                TRUSTED_FILE,
                &serde_json::to_string(&*items).unwrap_or_default(),
            );
        }
    }
}

/// What a composite input accepts: what the inner input it stands for does.
pub(crate) fn composite_expect(composite: &Composite, name: &str) -> Expect {
    composite_type(composite, name, true).map_or(Expect::Any, Expect::of_type)
}

/// What input `field` of an operation accepts, as the library documents it.
pub(crate) fn spec_expect(spec: &Spec, field: &str) -> Expect {
    graph::docs::input_type(spec, field).map_or(Expect::Any, Expect::of_type)
}

/// The kind of value port `field` of a node of `kind` holds, an input when
/// `input`: an operation's from the library, a composite's from the step
/// inside it the port leads to. Object nodes' rows come from the drawing.
pub(crate) fn port_type(kind: &Kind, field: &str, input: bool) -> Option<PortType> {
    match kind {
        Kind::Op(spec) if input => graph::docs::input_type(spec, field),
        Kind::Op(spec) => graph::docs::output_type(spec, field),
        Kind::Composite(composite) => composite_type(composite, field, input),
        Kind::Script(script) => script_type(script, field, input),
        Kind::Object(_) => None,
    }
}

/// The kind a script's header declares for port `field`.
pub(crate) fn script_type(script: &Script, field: &str, input: bool) -> Option<PortType> {
    let ports = if input {
        &script.inputs
    } else {
        &script.outputs
    };
    ports
        .iter()
        .find(|port| port.name == field)
        .map(|port| port.kind)
}

pub(crate) fn composite_type(composite: &Composite, field: &str, input: bool) -> Option<PortType> {
    let ports = if input {
        &composite.inputs
    } else {
        &composite.outputs
    };
    let (_, port) = ports.iter().find(|(name, _)| name == field)?;
    port_type(&composite.graph.node(port.node)?.kind, &port.field, input)
}

/// What a node does, for its tooltip.
pub(crate) fn describe(kind: &Kind) -> String {
    match kind {
        Kind::Op(spec) => graph::docs::doc(spec)
            .map(|doc| t!(doc.description).into_owned())
            .unwrap_or_default(),
        Kind::Object(name) => ObjectKind::from_name(name)
            .map(|kind| t!(kind.description()).into_owned())
            .unwrap_or_default(),
        Kind::Script(_) => t!(
            "A script: a short program computes its outputs from its inputs. Edit it to change its ports or what it does."
        )
        .into_owned(),
        Kind::Composite(composite) => {
            let made_of = crate::tf!(
                "A composite of {} steps used as one node. Expand it to see or change them.",
                composite.graph.nodes.len()
            );
            match composite_description(composite) {
                "" => made_of.into_owned(),
                about => format!("{about}\n{made_of}"),
            }
        }
    }
}

/// The kind a row holds: the declared one, or for a row of any kind the one
/// its value has.
fn shown_type(kind: Option<PortType>, value: &Value) -> PortType {
    match kind {
        Some(kind) if kind.kind != Type::Any => kind,
        _ => {
            let list = kind.is_some_and(|kind| kind.list) || value.is_array();
            let item = match value {
                Value::Array(items) => items.iter().find(|item| !item.is_null()),
                Value::Null => None,
                other => Some(other),
            };
            PortType {
                kind: item.map_or(Type::Any, |item| Expect::of_value(item).port_kind()),
                list,
            }
        }
    }
}

/// The name a card shows for a kind of value.
fn type_label(kind: Type) -> &'static str {
    match kind {
        Type::Any => "any",
        Type::Number => "number",
        Type::Text => "text",
        Type::Bool => "true/false",
        Type::Point => "point",
        Type::Vector => "vector",
        Type::Curve => "curve",
        Type::Plane => "plane",
        Type::Geometry => "geometry",
        Type::Object => "object",
    }
}

fn type_color(kind: Type) -> Color {
    match kind {
        Type::Any => Color::from_rgb8(0x94, 0xA3, 0xB8),
        Type::Number => Color::from_rgb8(0x3B, 0x82, 0xF6),
        Type::Text => Color::from_rgb8(0x10, 0xB9, 0x81),
        Type::Bool => Color::from_rgb8(0xF5, 0x9E, 0x0B),
        Type::Point => Color::from_rgb8(0x8B, 0x5C, 0xF6),
        Type::Vector => Color::from_rgb8(0x06, 0xB6, 0xD4),
        Type::Curve => Color::from_rgb8(0xF9, 0x73, 0x16),
        Type::Plane => Color::from_rgb8(0xEC, 0x48, 0x99),
        Type::Geometry => Color::from_rgb8(0x84, 0xCC, 0x16),
        Type::Object => Color::from_rgb8(0xEF, 0x44, 0x44),
    }
}

/// A small tag naming the kind of value a row holds; a list shows in
/// brackets.
fn type_chip<'a>(kind: PortType) -> Element<'a, Message> {
    let color = type_color(kind.kind);
    let name = t!(type_label(kind.kind)).into_owned();
    let label = if kind.list { format!("[{name}]") } else { name };
    container(text(label).size(9))
        .padding([0, 4])
        .style(move |theme: &Theme| container::Style {
            background: Some(Background::Color(color.scale_alpha(0.18))),
            text_color: Some(theme.palette().background.base.text),
            border: Border {
                color: color.scale_alpha(0.7),
                width: 1.0,
                radius: 6.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// An operation's port kinds, `inputs → outputs`.
fn signature(spec: &Spec) -> Option<String> {
    let doc = graph::docs::doc(spec)?;
    let names = |ports: &[PortType]| {
        ports
            .iter()
            .map(|port| {
                let name = t!(type_label(port.kind)).into_owned();
                if port.list {
                    format!("[{name}]")
                } else {
                    name
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    Some(format!("{} → {}", names(&doc.inputs), names(&doc.outputs)))
}

/// A Table node's cells as text, the first row its column names; an empty
/// table starts with two named columns and one row.
fn table_cells(value: &Value) -> Vec<Vec<String>> {
    let mut cells: Vec<Vec<String>> = graph::value::table(value)
        .iter()
        .map(|row| row.iter().map(display).collect())
        .collect();
    if cells.is_empty() {
        cells = vec![
            vec!["A".to_owned(), "B".to_owned()],
            vec![String::new(), String::new()],
        ];
    }
    let width = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
    for row in &mut cells {
        row.resize(width, String::new());
    }
    cells
}

fn table_value(cells: &[Vec<String>]) -> Value {
    Value::Array(
        cells
            .iter()
            .map(|row| Value::Array(row.iter().cloned().map(Value::String).collect()))
            .collect(),
    )
}

/// A Table node's grid: a cell per value, buttons to add or drop a row or a
/// column, and a box rows copied from a spreadsheet can be pasted into.
fn table_editor<'a>(port: Port, value: &Value) -> Element<'a, Message> {
    let cells = table_cells(value);
    let mut grid = column![].spacing(2);
    for (r, row_cells) in cells.iter().enumerate() {
        let mut line = row![].spacing(2);
        for (c, cell) in row_cells.iter().enumerate() {
            let (cells, port) = (cells.clone(), port.clone());
            let input = text_input("", cell)
                .on_input(move |text| {
                    let mut cells = cells.clone();
                    cells[r][c] = text;
                    msg(GraphMsg::Set(port.clone(), table_value(&cells)))
                })
                .size(11)
                .padding([2, 4])
                .width(Length::Fill);
            // The first row names the columns.
            line = line.push(if r == 0 {
                Element::from(container(input).style(|theme: &Theme| container::Style {
                    background: Some(Background::Color(theme.palette().primary.weak.color)),
                    ..Default::default()
                }))
            } else {
                input.into()
            });
        }
        grid = grid.push(line);
    }
    let (rows, columns) = (cells.len(), cells[0].len());
    let mut added_row = cells.clone();
    added_row.push(vec![String::new(); columns]);
    let mut added_column = cells.clone();
    added_column.iter_mut().enumerate().for_each(|(r, row)| {
        row.push(if r == 0 {
            char::from(b'A' + (columns % 26) as u8).to_string()
        } else {
            String::new()
        })
    });
    let mut dropped_row = cells.clone();
    dropped_row.pop();
    let mut dropped_column = cells.clone();
    dropped_column.iter_mut().for_each(|row| {
        row.pop();
    });
    let pasted = port.clone();
    let paste = text_input(&t!("Paste rows from a spreadsheet…"), "")
        .on_input({
            let port = pasted.clone();
            move |text| msg(GraphMsg::TablePaste(port.clone(), text))
        })
        .on_paste(move |text| msg(GraphMsg::TablePaste(pasted.clone(), text)))
        .size(10)
        .padding([2, 4])
        .width(Length::Fill);
    column![
        grid,
        row![
            icon_button(
                crate::ui::icons::PLUS,
                "Add row",
                Some(GraphMsg::Set(port.clone(), table_value(&added_row)))
            ),
            icon_button(
                crate::ui::icons::MINUS,
                "Remove last row",
                (rows > 2).then(|| GraphMsg::Set(port.clone(), table_value(&dropped_row)))
            ),
            Space::new().width(8.0),
            icon_button(
                crate::ui::icons::PLUS,
                "Add column",
                (columns < 8).then(|| GraphMsg::Set(port.clone(), table_value(&added_column)))
            ),
            icon_button(
                crate::ui::icons::MINUS,
                "Remove last column",
                (columns > 1).then(|| GraphMsg::Set(port.clone(), table_value(&dropped_column)))
            ),
            paste,
        ]
        .spacing(2)
        .align_y(iced::Center),
    ]
    .spacing(4)
    .into()
}

/// Why a node gave no result, in the interface's language.
pub(crate) fn issue_text(issue: &graph::Issue) -> String {
    match issue {
        graph::Issue::Missing(input) => crate::tf!("Nothing feeds {}", input).into_owned(),
        graph::Issue::EmptyList(input) => crate::tf!("The list on {} is empty", input).into_owned(),
        graph::Issue::NoResult => t!("No result with these values").into_owned(),
        graph::Issue::Inner(step, issue) => format!("{}: {}", t!(step.as_str()), issue_text(issue)),
        graph::Issue::Formula(why) => crate::tf!("The formula does not read: {}", why).into_owned(),
        graph::Issue::Script(why) => crate::tf!("Script: {}", why).into_owned(),
    }
}

/// The amber of a node's issues.
const ISSUE_COLOR: Color = Color::from_rgb8(0xF5, 0x9E, 0x0B);

/// A list's first items, one per line, under its length; `None` for a
/// single value, which its row already shows whole.
fn list_preview(value: &Value) -> Option<String> {
    const SHOWN: usize = 12;
    let items = value.as_array()?;
    let mut lines = vec![crate::tf!("{} items", items.len()).into_owned()];
    lines.extend(
        items
            .iter()
            .take(SHOWN)
            .enumerate()
            .map(|(k, item)| format!("{k}: {}", display(item))),
    );
    if items.len() > SHOWN {
        lines.push("…".to_owned());
    }
    Some(lines.join("\n"))
}

/// `content` with its list's preview as a tooltip, when it holds a list.
fn with_list_preview<'a>(
    content: Element<'a, Message>,
    preview: Option<String>,
) -> Element<'a, Message> {
    match preview {
        Some(preview) => tooltip(
            content,
            container(text(preview).size(10).font(iced::Font::MONOSPACE))
                .padding([4, 8])
                .style(container::bordered_box),
            tooltip::Position::Bottom,
        )
        .gap(4)
        .into(),
        None => content,
    }
}

/// A tooltip box holding a paragraph.
fn tip_box<'a>(content: String) -> Element<'a, Message> {
    // Containers take no maximum width: a long paragraph gets a fixed one
    // to wrap in, a short one keeps its own.
    let width = if content.chars().count() > 48 {
        Length::Fixed(280.0)
    } else {
        Length::Shrink
    };
    container(text(content).size(10))
        .padding([4, 8])
        .width(width)
        .style(container::bordered_box)
        .into()
}

pub type GroupId = u32;

/// Canvas state of one node.
#[derive(Clone)]
pub struct NodeUi {
    /// The user's name for the node, unique within the graph, so the picker
    /// tells apart several nodes of the same type.
    pub name: String,
    /// Expanded section titles; a new object node opens only Geometry.
    pub open: HashSet<String>,
    /// Entities an object node owns, one per list element.
    pub handles: Vec<Handle>,
    pub group: Option<GroupId>,
}

/// A named run of steps in the node sequence, or of cards in the Inputs area.
#[derive(Clone, Debug)]
pub struct NodeGroup {
    pub id: GroupId,
    pub name: String,
    /// Index into the accent colours.
    pub color: usize,
    pub collapsed: bool,
    /// Whether it gathers inputs rather than steps.
    pub inputs: bool,
}

pub enum RowWidget {
    Field,
    Slider {
        min: f64,
        max: f64,
        step: f64,
    },
    Toggle,
    /// A grid of cells, the first row its column names.
    Table,
}

/// A group of rows; an empty title has no header and is always open.
pub struct NodeSection {
    pub title: String,
    pub rows: Vec<NodeRow>,
}

pub struct NodeRow {
    pub field: String,
    pub label: String,
    pub value: Value,
    pub input: bool,
    pub output: bool,
    pub widget: RowWidget,
    /// The kind of value the row holds, when known ahead of its value.
    pub kind: Option<PortType>,
}

/// What a rename is renaming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renaming {
    Node(NodeId),
    Group(GroupId),
}

/// An edit that would break links, held until the user confirms it.
#[derive(Clone, Debug)]
pub struct Confirm {
    /// Applied on confirmation.
    pub action: GraphMsg,
    /// The node being deleted or moved.
    pub node: NodeId,
    pub delete: bool,
    /// Links the edit removes.
    pub links: Vec<Link>,
}

/// What the composite editor is changing: the node, then the composite's
/// name and description, and for each input its current name, new name and
/// default value, and for each output its current and new names.
#[derive(Clone, Debug)]
pub struct CompositeDraft {
    pub node: NodeId,
    pub name: String,
    pub description: String,
    pub inputs: Vec<(String, String, String)>,
    pub outputs: Vec<(String, String)>,
}

/// A script node's program being edited: the node, its name, the text and
/// the result of the last check (its ports, or why it does not read).
pub struct ScriptDraft {
    pub node: NodeId,
    pub name: String,
    pub content: iced::widget::text_editor::Content,
    pub checked: Option<Result<String, String>>,
}

/// What a script's check reports: its ports, or why it does not read.
fn script_check(source: &str) -> Result<String, String> {
    let script = Script::new("", source);
    if let Some(why) = &script.error {
        return Err(why.clone());
    }
    let ports = |ports: &[graph::script::ScriptPort]| {
        ports
            .iter()
            .map(|port| {
                let kind = port.kind.kind.name();
                format!(
                    "{} ({kind}{})",
                    port.name,
                    if port.kind.list { "[]" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    Ok(crate::tf!(
        "Reads well. Inputs: {} → outputs: {}",
        ports(&script.inputs),
        ports(&script.outputs)
    )
    .into_owned())
}

/// A composite's description, kept in its host metadata.
pub(crate) fn composite_description(composite: &Composite) -> &str {
    composite.meta["description"].as_str().unwrap_or_default()
}

/// Everything an undo step restores.
#[derive(Clone)]
pub(crate) struct Snapshot {
    engine: graph::Graph,
    ui: HashMap<NodeId, NodeUi>,
    groups: Vec<NodeGroup>,
}

impl Snapshot {
    /// The name node `id` had.
    pub(crate) fn name(&self, id: NodeId) -> String {
        self.ui
            .get(&id)
            .map(|ui| ui.name.clone())
            .unwrap_or_default()
    }
}

/// The kind of value an input expects, so the picker can dim the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Expect {
    Any,
    Number,
    Text,
    Bool,
    Point,
    Object,
    /// A curve value or a drawn object: anything a curve node reads.
    Curve,
    /// A plane value: a frame to place or orient geometry in.
    Plane,
}

impl Expect {
    pub(crate) fn of_value(value: &Value) -> Self {
        match value {
            Value::Number(_) => Expect::Number,
            Value::String(_) => Expect::Text,
            Value::Bool(_) => Expect::Bool,
            Value::Object(object) if object.contains_key("handle") => Expect::Object,
            Value::Object(object) if object.contains_key("curve") => Expect::Curve,
            Value::Object(object) if object.contains_key("plane") => Expect::Plane,
            Value::Object(_) if graph::value::point(value).is_some() => Expect::Point,
            _ => Expect::Any,
        }
    }

    /// Whether `value` can feed this input; a list feeds it element-wise.
    pub(crate) fn fits(self, value: &Value) -> bool {
        match value {
            Value::Null => true,
            Value::Array(items) => items.first().is_none_or(|item| self.fits(item)),
            _ => match self {
                Expect::Any => true,
                Expect::Number => {
                    value.is_number()
                        || value
                            .as_str()
                            .is_some_and(|text| text.trim().parse::<f64>().is_ok())
                }
                Expect::Text => value.is_string() || value.is_number() || value.is_boolean(),
                Expect::Bool => value.is_boolean(),
                Expect::Point => graph::value::point(value).is_some(),
                Expect::Object => value.get("handle").is_some(),
                Expect::Curve => {
                    value.get("handle").is_some() || graph::value::curve(value).is_some()
                }
                Expect::Plane => graph::value::plane(value).is_some(),
            },
        }
    }

    /// The kind's name in automation payloads.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Expect::Any => "any",
            Expect::Number => "number",
            Expect::Text => "text",
            Expect::Bool => "bool",
            Expect::Point => "point",
            Expect::Object => "object",
            Expect::Curve => "curve",
            Expect::Plane => "plane",
        }
    }

    /// The check for a documented port kind: a list port checks its items.
    pub(crate) fn of_type(kind: PortType) -> Self {
        match kind.kind {
            Type::Any | Type::Geometry => Expect::Any,
            Type::Number => Expect::Number,
            Type::Text => Expect::Text,
            Type::Bool => Expect::Bool,
            Type::Point | Type::Vector => Expect::Point,
            Type::Curve => Expect::Curve,
            Type::Plane => Expect::Plane,
            Type::Object => Expect::Object,
        }
    }

    /// The documented kind this check stands for.
    pub(crate) fn port_kind(self) -> Type {
        match self {
            Expect::Any => Type::Any,
            Expect::Number => Type::Number,
            Expect::Text => Type::Text,
            Expect::Bool => Type::Bool,
            Expect::Point => Type::Point,
            Expect::Object => Type::Object,
            Expect::Curve => Type::Curve,
            Expect::Plane => Type::Plane,
        }
    }

    /// Whether a value of kind `actual` can feed an input of this kind;
    /// an unknown kind on either side fits.
    pub(crate) fn accepts(self, actual: Expect) -> bool {
        self == Expect::Any
            || actual == Expect::Any
            || self == actual
            || (self == Expect::Text && matches!(actual, Expect::Number | Expect::Bool))
            || matches!(
                (self, actual),
                (Expect::Curve, Expect::Object) | (Expect::Object, Expect::Curve)
            )
    }

    fn label(self) -> Option<&'static str> {
        match self {
            Expect::Any => None,
            Expect::Number => Some("Expects a number"),
            Expect::Text => Some("Expects text"),
            Expect::Bool => Some("Expects true or false"),
            Expect::Point => Some("Expects a point"),
            Expect::Object => Some("Expects an object"),
            Expect::Curve => Some("Expects a curve"),
            Expect::Plane => Some("Expects a plane"),
        }
    }

    /// The input node a new value for this input starts as.
    fn input_kind(self) -> Option<&'static str> {
        match self {
            Expect::Any | Expect::Number => Some("Number"),
            Expect::Text => Some("Text"),
            Expect::Bool => Some("Boolean"),
            Expect::Point | Expect::Object | Expect::Curve | Expect::Plane => None,
        }
    }
}

#[derive(Default)]
pub struct Graph {
    /// Node order is the node sequence order of the steps; Inputs-area nodes keep
    /// their own relative order among them.
    pub engine: graph::Graph,
    pub ui: HashMap<NodeId, NodeUi>,
    pub groups: Vec<NodeGroup>,
    pub drafts: HashMap<Port, String>,
    /// The step input whose source the value picker is choosing.
    pub picking: Option<Port>,
    /// Filter typed into the picker's search box.
    pick_search: String,
    /// The picker shows only values of the kind the input takes.
    pub pick_compatible: bool,
    /// The card under the pointer: its geometry is highlighted in the
    /// drawing, and a dragged card drops onto it.
    pub hovered: Option<NodeId>,
    /// The card being dragged to a new place.
    pub dragging: Option<NodeId>,
    /// A composite node's name, description and ports being edited.
    pub editing: Option<CompositeDraft>,
    /// A script node's program being edited.
    pub scripting: Option<ScriptDraft>,
    /// What is being renamed and its draft name.
    pub renaming: Option<(Renaming, String)>,
    /// An edit waiting for the user to accept the links it breaks.
    pub confirm: Option<Confirm>,
    /// The last duplication, `(original, copy)`, for the app to give the copy
    /// its own entities.
    pub duplicated: Option<(NodeId, NodeId)>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The input a run of slider moves is recording into, so a drag is one
    /// undo step.
    recording: Option<Port>,
    /// Categories the user has flipped from their initial state: Objects
    /// starts open, the rest closed.
    toggled_categories: HashSet<&'static str>,
    /// Filter typed into the panel's search box.
    search: String,
    /// Node sequence zoom in steps; the scale is `ZOOM_STEP ^ zoom_steps`.
    zoom_steps: i32,
    /// Only the Inputs area shows, so the drawing stays visible while the
    /// inputs are tuned.
    pub steps_hidden: bool,
    /// Counts structural edits, so automation clients can tell when the
    /// graph they read has changed.
    pub revision: u64,
    /// Hides the viewport preview of the curves nodes compute.
    pub preview_hidden: bool,
    /// The drawing's edit revision the graph last checked its Layer Objects
    /// nodes against, and the fingerprint of what they read then.
    pub followed_revision: u64,
    pub watched: Option<u64>,
    /// Text typed into the node sequence's find box, and the match shown.
    node_search: String,
    pub found: Option<NodeId>,
    /// Steps show as small cards with their results only.
    pub compact: bool,
    /// The graph was opened with scripts the user has not chosen to run:
    /// it is not evaluated until they do.
    pub scripts_paused: bool,
}

#[derive(Debug, Clone)]
pub enum GraphMsg {
    Toggle,
    Category(&'static str),
    /// A palette entry was clicked: inputs join the Inputs area, everything
    /// else the end of the node sequence.
    Add(PaletteItem),
    /// Asks to delete a node; one other nodes read from asks first.
    NodeDelete(NodeId),
    /// Deletes a node without asking.
    DeleteNow(NodeId),
    /// Copies a node, its values and what feeds it, right after itself.
    Duplicate(NodeId),
    /// Moves a node one place earlier (-1) or later (+1) among its kind; one
    /// that would break links asks first.
    Move(NodeId, i32),
    MoveNow(NodeId, i32),
    Section(NodeId, String),
    RenameStart(NodeId),
    GroupRenameStart(GroupId),
    RenameInput(String),
    RenameCommit,
    /// Opens the value picker for a step input.
    Pick(Port),
    PickSearch(String),
    /// Shows only the values that fit the input, or all of them.
    PickCompatible(bool),
    /// The pointer entered a card (`Some`) or left it (`None`).
    Hover(Option<NodeId>),
    /// A card's header was pressed: dragging it starts.
    DragStart(NodeId),
    /// The pointer was released over this card: the dragged card moves to
    /// its place, asking first when that breaks links.
    DragDrop(NodeId),
    /// Moves a node to another node's place without asking.
    MoveToNow(NodeId, NodeId),
    /// The pointer left this card.
    HoverEnd(NodeId),
    /// Opens the editor of a composite node.
    CompositeEdit(NodeId),
    CompositeDraftName(String),
    CompositeDraftDescription(String),
    /// A new name for the input at this place.
    CompositeDraftInput(usize, String),
    /// A new default for the input at this place.
    CompositeDraftDefault(usize, String),
    CompositeDraftOutput(usize, String),
    CompositeEditSave,
    CompositeEditCancel,
    /// Opens the editor of a script node's program.
    ScriptEdit(NodeId),
    ScriptName(String),
    ScriptAction(iced::widget::text_editor::Action),
    /// Reads the program, reporting its ports or why it does not read.
    ScriptCheck,
    ScriptSave,
    ScriptCancel,
    /// Saves a script node to the script library.
    SaveScript(NodeId),
    ForgetScript(usize),
    /// Runs the scripts of the graph just opened, and remembers the choice.
    TrustScripts,
    /// Writes the composite library to a file the user picks.
    LibraryExport,
    /// Adds the composites of a library file the user picks.
    LibraryImport,
    LibraryImported(Option<Vec<u8>>),
    /// The library was written to this file, or failed to be.
    LibraryExported(Option<Result<String, String>>),
    /// The pointer was released away from every card.
    DragCancel,
    PickCancel,
    /// The picker chose this output for the input being picked.
    Choose(Port),
    /// Creates an input node of this kind holding this value and feeds the
    /// input being picked from it.
    PickNewInput(&'static str, Value),
    /// Clears what feeds a step input; it falls back to its own value.
    Unlink(Port),
    Input(Port, String),
    Commit(Port),
    /// A slider or toggle set an input value.
    Set(Port, Value),
    /// Text pasted into a table's paste box: rows of tab-, semicolon- or
    /// comma-separated cells replace the table.
    TablePaste(Port, String),
    /// Text typed into the find box; the first match is shown.
    NodeSearch(String),
    /// Shows the next (1) or previous (-1) match.
    NodeSearchStep(i32),
    ToggleCompact,
    /// Turns a Number into a slider, or a slider back into a Number.
    ToggleSlider(NodeId),
    /// Puts nodes, with the links among them, on the clipboard.
    CopyNodes(Vec<NodeId>),
    /// Reads the clipboard to paste the nodes on it.
    Paste,
    Pasted(Option<String>),
    /// Start an empty graph; the drawing keeps the old graph's objects.
    New,
    Save,
    /// `None` when the dialog was cancelled.
    Saved(Option<Result<String, String>>),
    Open,
    Loaded(Option<Vec<u8>>),
    Search(String),
    /// Zooms the node sequence by this many steps; 0 resets it.
    Zoom(i32),
    /// Shows or hides the node sequence, leaving only the Inputs area.
    ToggleSteps,
    /// Shows or hides the viewport preview of computed curves.
    TogglePreview,
    /// Adds an object node for each selected drawing entity, owning it as
    /// drawn.
    FromSelection,
    /// Starts a group holding this node.
    GroupNew(NodeId),
    GroupAssign(NodeId, Option<GroupId>),
    GroupCollapse(GroupId),
    /// Gives a group the next accent colour.
    GroupColor(GroupId),
    /// Dissolves a group; its nodes stay where they are.
    GroupDelete(GroupId),
    /// Turns a group's steps into one composite node.
    GroupToComposite(GroupId),
    /// Puts a composite's nodes back in its place.
    ExpandComposite(NodeId),
    /// Saves a composite node to the library, under the node's name.
    SaveComposite(NodeId),
    /// Drops a library entry.
    ForgetComposite(usize),
    Undo,
    Redo,
    ConfirmYes,
    ConfirmNo,
}

fn msg(m: GraphMsg) -> Message {
    Message::Graph(m)
}

/// Compact display of a port value.
pub fn display(value: &Value) -> String {
    let number = |x: f64| {
        let s = format!("{:.4}", x);
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    };
    match value {
        Value::Number(n) => n.as_f64().map_or_else(|| n.to_string(), number),
        Value::Array(items) => {
            let shown: Vec<String> = items.iter().take(4).map(display).collect();
            let more = if items.len() > 4 { ", …" } else { "" };
            format!("[{}] {}{more}", items.len(), shown.join(", "))
        }
        Value::Object(_) => match (graph::value::point(value), graph::value::curve(value)) {
            (Some(p), _) => format!("({}, {}, {})", number(p[0]), number(p[1]), number(p[2])),
            (None, Some(curve)) => {
                let kind = value["curve"].as_str().unwrap_or_default();
                match curve.curve.extent() {
                    kernel::geom2d::Extent::Bounded => {
                        format!("{kind} · L {}", number(curve.length()))
                    }
                    _ => kind.to_owned(),
                }
            }
            (None, None) => match graph::value::plane(value) {
                Some(plane) => {
                    let p = plane.origin;
                    format!(
                        "plane ({}, {}, {})",
                        number(p[0]),
                        number(p[1]),
                        number(p[2])
                    )
                }
                None => value.to_string(),
            },
        },
        other => graph::value::text(other),
    }
}

/// The type name shown under a node's own name.
pub(crate) fn kind_name(kind: &Kind) -> String {
    match kind {
        Kind::Op(spec) => t!(spec.name).into_owned(),
        Kind::Object(name) => t!(name.as_str()).into_owned(),
        Kind::Composite(composite) => composite.name.clone(),
        Kind::Script(script) => script.name.clone(),
    }
}

fn weak_text(theme: &Theme) -> text::Style {
    let palette = theme.palette();
    text::Style {
        color: Some(palette.background.base.text.scale_alpha(0.6)),
    }
}

fn card_style(theme: &Theme) -> container::Style {
    let palette = theme.palette();
    container::Style {
        background: Some(Background::Color(palette.background.base.color)),
        border: Border {
            color: palette.background.strong.color,
            width: 1.0,
            radius: 6.0.into(),
        },
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.15),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 6.0,
        },
        ..Default::default()
    }
}

fn header_style(theme: &Theme) -> container::Style {
    let palette = theme.palette();
    container::Style {
        background: Some(Background::Color(palette.primary.weak.color)),
        text_color: Some(palette.primary.weak.text),
        border: Border {
            radius: iced::border::top(6.0),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Accent colours telling nodes and groups apart: a node keeps its colour on
/// its card, in the value picker and beside every row that reads from it.
const NODE_COLORS: [Color; 8] = [
    Color::from_rgb8(0x3B, 0x82, 0xF6),
    Color::from_rgb8(0x10, 0xB9, 0x81),
    Color::from_rgb8(0xF5, 0x9E, 0x0B),
    Color::from_rgb8(0xEF, 0x44, 0x44),
    Color::from_rgb8(0x8B, 0x5C, 0xF6),
    Color::from_rgb8(0x06, 0xB6, 0xD4),
    Color::from_rgb8(0xEC, 0x48, 0x99),
    Color::from_rgb8(0x84, 0xCC, 0x16),
];

/// A node's accent colour, stable while it lives.
pub fn node_color(id: NodeId) -> Color {
    NODE_COLORS[(id.saturating_sub(1) as usize) % NODE_COLORS.len()]
}

fn group_color(group: &NodeGroup) -> Color {
    NODE_COLORS[group.color % NODE_COLORS.len()]
}

/// A round chip in `color`: the step number, or a plain dot for an input.
fn badge<'a>(color: Color, label: Option<String>) -> Element<'a, Message> {
    let size = if label.is_some() { 22.0 } else { 12.0 };
    container(text(label.unwrap_or_default()).size(11))
        .center_x(size)
        .center_y(size)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            text_color: Some(Color::WHITE),
            border: Border {
                radius: (size * 0.5).into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// Where a node comes from, as the picker tags its group.
fn kind_tag(kind: &Kind) -> String {
    match kind {
        _ if is_input(kind) => t!("Input").into_owned(),
        Kind::Op(spec) => t!(spec.category).into_owned(),
        Kind::Object(_) => t!("Object").into_owned(),
        Kind::Composite(_) => t!("Composite").into_owned(),
        Kind::Script(_) => t!("Script").into_owned(),
    }
}

/// A small icon button with a tooltip; `None` shows it disabled.
fn icon_button<'a>(
    icon: &'static [u8],
    tip: &str,
    message: Option<GraphMsg>,
) -> Element<'a, Message> {
    let icon = if message.is_some() {
        crate::ui::icons::themed(icon, 10.0)
    } else {
        crate::ui::icons::themed_disabled(icon, 10.0)
    };
    let button = button(icon)
        .on_press_maybe(message.map(msg))
        .style(button::text)
        .padding([5, 6]);
    tooltip(
        button,
        container(text(t!(tip).into_owned()).size(10))
            .padding([2, 6])
            .style(container::bordered_box),
        tooltip::Position::Top,
    )
    .gap(4)
    .into()
}

/// A choice in a card's group list.
#[derive(Clone, Debug, PartialEq)]
enum GroupChoice {
    None,
    Join(GroupId, String),
    New,
}

impl std::fmt::Display for GroupChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GroupChoice::None => f.write_str(&t!("No group")),
            GroupChoice::Join(_, name) => f.write_str(name),
            GroupChoice::New => f.write_str(&t!("New group…")),
        }
    }
}

impl Graph {
    fn zoom(&self) -> f32 {
        ZOOM_STEP.powi(self.zoom_steps)
    }

    /// The `.ocg` file: the graph, the canvas, and the fingerprint of the
    /// drawing its object nodes' entities live in. The graph's node order is
    /// the node sequence order.
    pub fn to_file(&self, drawing: &str) -> Vec<u8> {
        let nodes: serde_json::Map<String, Value> = self
            .ui
            .iter()
            .map(|(id, ui)| {
                let mut open: Vec<&String> = ui.open.iter().collect();
                open.sort();
                let handles: Vec<u64> = ui.handles.iter().map(|handle| handle.value()).collect();
                (
                    id.to_string(),
                    json!({ "name": ui.name, "open": open, "handles": handles, "group": ui.group }),
                )
            })
            .collect();
        let groups: Vec<Value> = self
            .groups
            .iter()
            .map(|group| {
                json!({
                    "id": group.id,
                    "name": group.name,
                    "color": group.color,
                    "collapsed": group.collapsed,
                    "inputs": group.inputs,
                })
            })
            .collect();
        let file = json!({
            "format": "ocg",
            "version": 2,
            "drawing": drawing,
            "graph": self.engine.to_json(),
            "canvas": {
                "zoom": self.zoom_steps,
                "steps_hidden": self.steps_hidden,
                "preview_hidden": self.preview_hidden,
                "compact": self.compact,
                "groups": groups,
                "nodes": nodes,
            },
        });
        serde_json::to_vec_pretty(&file).unwrap_or_default()
    }

    /// Reads an `.ocg` file, returning the graph and the drawing fingerprint
    /// it was saved against. Handles come back unchecked.
    pub fn from_file(bytes: &[u8]) -> Result<(Self, String), String> {
        let file: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        if file["format"] != "ocg" {
            return Err(t!("Not a node graph file").into_owned());
        }
        let mut graph = Graph {
            engine: graph::Graph::from_json(&file["graph"])?,
            ..Graph::default()
        };
        // Version 1 graphs were wired freely on a canvas, so their node order
        // means nothing; lay their steps out in dataflow order instead.
        if file["version"].as_u64().unwrap_or(1) < 2 {
            let order = graph.engine.order();
            graph.engine.nodes.sort_by_key(|node| {
                order
                    .iter()
                    .position(|id| *id == node.id)
                    .unwrap_or(usize::MAX)
            });
        }
        let canvas = &file["canvas"];
        graph.zoom_steps = canvas["zoom"].as_i64().map_or(0, |steps| {
            (steps as i32).clamp(*ZOOM_STEPS.start(), *ZOOM_STEPS.end())
        });
        graph.steps_hidden = canvas["steps_hidden"].as_bool().unwrap_or(false);
        graph.preview_hidden = canvas["preview_hidden"].as_bool().unwrap_or(false);
        graph.compact = canvas["compact"].as_bool().unwrap_or(false);
        for group in canvas["groups"].as_array().into_iter().flatten() {
            let (Some(id), Some(name)) = (group["id"].as_u64(), group["name"].as_str()) else {
                continue;
            };
            graph.groups.push(NodeGroup {
                id: id as GroupId,
                name: name.to_owned(),
                color: group["color"].as_u64().unwrap_or(0) as usize,
                collapsed: group["collapsed"].as_bool().unwrap_or(false),
                inputs: group["inputs"].as_bool().unwrap_or(false),
            });
        }
        let ids: Vec<(NodeId, String, bool)> = graph
            .engine
            .nodes
            .iter()
            .map(|node| (node.id, kind_name(&node.kind), is_input(&node.kind)))
            .collect();
        for (id, kind, input) in ids {
            let saved = &canvas["nodes"][id.to_string()];
            let open: HashSet<String> = match saved["open"].as_array() {
                Some(titles) => titles
                    .iter()
                    .filter_map(|title| title.as_str().map(str::to_owned))
                    .collect(),
                None => std::iter::once(t!("Geometry").into_owned()).collect(),
            };
            let handles = saved["handles"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_u64)
                        .map(Handle::new)
                        .collect()
                })
                .unwrap_or_default();
            let name = match saved["name"].as_str().map(str::trim) {
                Some(name) if !name.is_empty() => graph.unique_name(name, None, false),
                _ => graph.unique_name(&kind, None, true),
            };
            // A group only holds nodes of its own area.
            let group = saved["group"]
                .as_u64()
                .map(|group| group as GroupId)
                .filter(|group| {
                    graph
                        .groups
                        .iter()
                        .any(|known| known.id == *group && known.inputs == input)
                });
            graph.ui.insert(
                id,
                NodeUi {
                    name,
                    open,
                    handles,
                    group,
                },
            );
        }
        graph.prune_links();
        graph.prune_groups();
        Ok((
            graph,
            file["drawing"].as_str().unwrap_or_default().to_owned(),
        ))
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        Snapshot {
            engine: self.engine.clone(),
            ui: self.ui.clone(),
            groups: self.groups.clone(),
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.engine = snapshot.engine;
        self.ui = snapshot.ui;
        self.groups = snapshot.groups;
        self.picking = None;
        self.renaming = None;
        self.confirm = None;
        self.drafts.clear();
        self.recording = None;
    }

    /// Saves the graph as an undo step before an edit.
    pub fn record(&mut self) {
        let snapshot = self.snapshot();
        self.push_undo(snapshot);
    }

    /// Makes `snapshot`, taken before an edit, that edit's undo step.
    pub(crate) fn push_undo(&mut self, snapshot: Snapshot) {
        self.undo.push(snapshot);
        if self.undo.len() > HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.recording = None;
        self.revision += 1;
    }

    /// Puts back the nodes, links and groups of `snapshot`, keeping the
    /// canvas state (history, picker, zoom) as it is.
    pub(crate) fn restore_content(&mut self, snapshot: Snapshot) {
        self.engine = snapshot.engine;
        self.ui = snapshot.ui;
        self.groups = snapshot.groups;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Every entity some node owns.
    pub fn owned_handles(&self) -> Vec<Handle> {
        self.ui
            .values()
            .flat_map(|ui| ui.handles.iter().copied())
            .collect()
    }

    /// Empties the graph as one undo step.
    pub fn clear(&mut self) {
        self.record();
        self.engine = graph::Graph::default();
        self.ui.clear();
        self.groups.clear();
        self.picking = None;
        self.renaming = None;
        self.confirm = None;
        self.drafts.clear();
    }

    pub(crate) fn taken(&self, name: &str, except: Option<NodeId>) -> bool {
        let name = name.to_lowercase();
        self.ui
            .iter()
            .any(|(id, ui)| Some(*id) != except && ui.name.to_lowercase() == name)
    }

    /// `base` itself when free (and not `numbered`), else `base 2`, `base 3`…
    /// A numbered default starts at `base 1`.
    pub(crate) fn unique_name(&self, base: &str, except: Option<NodeId>, numbered: bool) -> String {
        if !numbered && !self.taken(base, except) {
            return base.to_owned();
        }
        (if numbered { 1 } else { 2 }..)
            .map(|n| format!("{base} {n}"))
            .find(|name| !self.taken(name, except))
            .unwrap_or_default()
    }

    /// A node's own name.
    pub fn name(&self, id: NodeId) -> String {
        self.ui
            .get(&id)
            .map(|ui| ui.name.clone())
            .unwrap_or_default()
    }

    pub(crate) fn group(&self, id: GroupId) -> Option<&NodeGroup> {
        self.groups.iter().find(|group| group.id == id)
    }

    pub(crate) fn node_group(&self, id: NodeId) -> Option<GroupId> {
        self.ui.get(&id).and_then(|ui| ui.group)
    }

    /// Adds a node: an input joins the Inputs area, anything else the end of
    /// the node sequence.
    pub fn add_node(&mut self, item: PaletteItem, handles: Vec<Handle>) -> NodeId {
        let kind = item
            .kind()
            .unwrap_or_else(|| Kind::Object(ObjectKind::Draw.name().to_owned()));
        let name = self.unique_name(&kind_name(&kind), None, true);
        self.add_named(kind, name, handles)
    }

    pub(crate) fn add_named(&mut self, kind: Kind, name: String, handles: Vec<Handle>) -> NodeId {
        let id = self.engine.add(kind);
        let mut open = HashSet::default();
        open.insert(t!("Geometry").into_owned());
        self.ui.insert(
            id,
            NodeUi {
                name,
                open,
                handles,
                group: None,
            },
        );
        id
    }

    /// Copies `id` right after itself: its kind, values, sections, group and
    /// every link into it. The copy owns no entities yet.
    pub(crate) fn duplicate(&mut self, id: NodeId) -> Option<NodeId> {
        let index = self.engine.nodes.iter().position(|node| node.id == id)?;
        let original = self.engine.nodes[index].clone();
        let ui = self.ui.get(&id)?.clone();
        let name = self.unique_name(&ui.name, None, false);
        let copy = self.add_named(original.kind, name, Vec::new());
        if let Some(node) = self.engine.nodes.pop() {
            self.engine.nodes.insert(index + 1, node);
        }
        if let Some(node) = self.engine.node_mut(copy) {
            node.params = original.params;
        }
        if let Some(copy_ui) = self.ui.get_mut(&copy) {
            copy_ui.open = ui.open;
            copy_ui.group = ui.group;
        }
        let incoming: Vec<Link> = self
            .engine
            .links
            .iter()
            .filter(|link| link.to.node == id)
            .cloned()
            .collect();
        for link in incoming {
            self.engine.connect(
                link.from,
                Port {
                    node: copy,
                    field: link.to.field,
                },
            );
        }
        Some(copy)
    }

    /// A fingerprint of every script's program, inside composites too;
    /// `None` without scripts.
    pub fn script_fingerprint(&self) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        fn sources<'g>(graph: &'g graph::Graph, into: &mut Vec<&'g str>) {
            for node in &graph.nodes {
                match &node.kind {
                    Kind::Script(script) => into.push(&script.source),
                    Kind::Composite(composite) => sources(&composite.graph, into),
                    _ => {}
                }
            }
        }
        let mut all = Vec::new();
        sources(&self.engine, &mut all);
        if all.is_empty() {
            return None;
        }
        all.sort_unstable();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        all.hash(&mut hasher);
        Some(hasher.finish())
    }

    /// Nodes whose name or type holds the find box's text, in node order.
    pub(crate) fn search_matches(&self) -> Vec<NodeId> {
        let query = self.node_search.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        self.engine
            .nodes
            .iter()
            .filter(|node| {
                self.name(node.id).to_lowercase().contains(&query)
                    || kind_name(&node.kind).to_lowercase().contains(&query)
            })
            .map(|node| node.id)
            .collect()
    }

    /// Shows match `step` places after the current one, opening its group.
    fn find(&mut self, step: i32) {
        let matches = self.search_matches();
        let current = self
            .found
            .and_then(|id| matches.iter().position(|found| *found == id));
        self.found = match (current, matches.len() as i32) {
            (_, 0) => None,
            (None, _) => matches.first().copied(),
            (Some(k), len) => Some(matches[(k as i32 + step).rem_euclid(len) as usize]),
        };
        if let Some(group) = self.found.and_then(|id| self.node_group(id)) {
            if let Some(group) = self.groups.iter_mut().find(|g| g.id == group) {
                group.collapsed = false;
            }
        }
        self.hovered = self.found;
    }

    /// Where the found node sits: whether it is an input, and how far along
    /// its area it is, from 0 (first) to 1 (last).
    pub fn found_place(&self) -> Option<(bool, f32)> {
        let id = self.found?;
        let input = is_input(&self.engine.node(id)?.kind);
        let area: Vec<NodeId> = self
            .engine
            .nodes
            .iter()
            .filter(|node| is_input(&node.kind) == input)
            .map(|node| node.id)
            .collect();
        let k = area.iter().position(|other| *other == id)?;
        Some((input, k as f32 / (area.len().max(2) - 1) as f32))
    }

    /// Nodes `ids` as clipboard text: the nodes, the links among them, their
    /// names and groups, and the links from other nodes by those nodes'
    /// names, so a paste into a graph with nodes of the same names relinks
    /// them.
    pub fn copy_text(&self, ids: &[NodeId]) -> String {
        let keep: HashSet<NodeId> = ids.iter().copied().collect();
        let mut part = graph::Graph::default();
        part.nodes = self
            .engine
            .nodes
            .iter()
            .filter(|node| keep.contains(&node.id))
            .cloned()
            .collect();
        let (inner, outer): (Vec<&Link>, Vec<&Link>) = self
            .engine
            .links
            .iter()
            .filter(|link| keep.contains(&link.to.node))
            .partition(|link| keep.contains(&link.from.node));
        part.links = inner.into_iter().cloned().collect();
        let names: serde_json::Map<String, Value> = ids
            .iter()
            .map(|id| (id.to_string(), json!(self.name(*id))))
            .collect();
        let groups: serde_json::Map<String, Value> = ids
            .iter()
            .filter_map(|id| {
                let group = self.group(self.node_group(*id)?)?;
                Some((
                    id.to_string(),
                    json!({ "id": group.id, "name": group.name, "color": group.color, "inputs": group.inputs }),
                ))
            })
            .collect();
        let outer: Vec<Value> = outer
            .into_iter()
            .map(|link| {
                json!({
                    "from": self.name(link.from.node),
                    "field": link.from.field,
                    "node": link.to.node,
                    "to": link.to.field,
                })
            })
            .collect();
        serde_json::to_string(&json!({
            "format": CLIP_FORMAT,
            "graph": part.to_json(),
            "names": names,
            "groups": groups,
            "outer": outer,
        }))
        .unwrap_or_default()
    }

    /// Adds the nodes in clipboard text written by
    /// [`copy_text`](Self::copy_text): inputs to the Inputs area, steps to
    /// the end of the node sequence, each in a new group of its own where
    /// they had one. Returns the new nodes.
    pub fn paste_text(&mut self, text: &str) -> Result<Vec<NodeId>, String> {
        let clip: Value = serde_json::from_str(text.trim())
            .map_err(|_| t!("The clipboard holds no nodes").into_owned())?;
        if clip["format"] != CLIP_FORMAT {
            return Err(t!("The clipboard holds no nodes").into_owned());
        }
        let part = graph::Graph::from_json(&clip["graph"])?;
        if part.nodes.is_empty() {
            return Err(t!("The clipboard holds no nodes").into_owned());
        }
        self.record();
        let mut map: HashMap<NodeId, NodeId> = HashMap::default();
        let mut groups: HashMap<u64, GroupId> = HashMap::default();
        for node in part.nodes {
            let base = clip["names"][node.id.to_string()]
                .as_str()
                .map_or_else(|| kind_name(&node.kind), str::to_owned);
            let name = self.unique_name(&base, None, false);
            let id = self.add_named(node.kind, name, Vec::new());
            if let Some(copy) = self.engine.node_mut(id) {
                copy.params = node.params;
            }
            let group = &clip["groups"][node.id.to_string()];
            if let Some(old) = group["id"].as_u64() {
                let new = match groups.get(&old) {
                    Some(new) => *new,
                    None => {
                        let new = self.groups.iter().map(|g| g.id).max().unwrap_or(0) + 1;
                        let name = group["name"].as_str().unwrap_or_default();
                        self.groups.push(NodeGroup {
                            id: new,
                            name: self.unique_group_name(name),
                            color: group["color"].as_u64().unwrap_or(0) as usize,
                            collapsed: false,
                            inputs: group["inputs"].as_bool().unwrap_or(false),
                        });
                        groups.insert(old, new);
                        new
                    }
                };
                if let Some(ui) = self.ui.get_mut(&id) {
                    ui.group = Some(new);
                }
            }
            map.insert(node.id, id);
        }
        for link in part.links {
            if let (Some(from), Some(to)) = (map.get(&link.from.node), map.get(&link.to.node)) {
                self.engine.connect(
                    Port {
                        node: *from,
                        field: link.from.field,
                    },
                    Port {
                        node: *to,
                        field: link.to.field,
                    },
                );
            }
        }
        let pasted: HashSet<NodeId> = map.values().copied().collect();
        for link in clip["outer"].as_array().into_iter().flatten() {
            let (Some(from), Some(field), Some(to), Some(port)) = (
                link["from"].as_str(),
                link["field"].as_str(),
                link["node"]
                    .as_u64()
                    .and_then(|id| map.get(&(id as NodeId))),
                link["to"].as_str(),
            ) else {
                continue;
            };
            let source = self.engine.nodes.iter().find(|node| {
                !pasted.contains(&node.id) && self.name(node.id).eq_ignore_ascii_case(from)
            });
            if let Some(source) = source.map(|node| node.id) {
                self.engine.connect(
                    Port {
                        node: source,
                        field: field.to_owned(),
                    },
                    Port {
                        node: *to,
                        field: port.to_owned(),
                    },
                );
            }
        }
        // Inputs gather with the inputs; the steps follow the last step.
        self.engine.nodes.sort_by_key(|node| !is_input(&node.kind));
        self.prune_links();
        self.revision += 1;
        let mut added: Vec<NodeId> = map.into_values().collect();
        added.sort_unstable();
        Ok(added)
    }

    /// `base`, or `base 2`, `base 3`… when a group already has that name.
    fn unique_group_name(&self, base: &str) -> String {
        let taken = |name: &str| {
            self.groups
                .iter()
                .any(|g| g.name.eq_ignore_ascii_case(name))
        };
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|name| !taken(name))
            .unwrap_or_default()
    }

    /// Turns a Number input into a Number Slider around its value, or a
    /// slider back into a Number; its links and name stay.
    fn toggle_slider(&mut self, id: NodeId) {
        let Some(node) = self.engine.node_mut(id) else {
            return;
        };
        let value = node
            .params
            .get("value")
            .and_then(graph::value::num)
            .unwrap_or(0.0);
        let (name, range) = match &node.kind {
            Kind::Op(spec) if spec.name == "Number" => {
                let span = value.abs().max(1.0) * 2.0;
                let (min, max) = if value >= 0.0 {
                    (0.0, span)
                } else {
                    (-span, 0.0)
                };
                ("Number Slider", Some((min, max, span / 100.0)))
            }
            Kind::Op(spec) if matches!(spec.name, "Number Slider" | "Integer Slider") => {
                ("Number", None)
            }
            _ => return,
        };
        let Some(spec) = graph::spec(name) else {
            return;
        };
        node.kind = Kind::Op(spec);
        node.params.retain(|field, _| field == "value");
        if let Some((min, max, step)) = range {
            for (field, x) in [("min", min), ("max", max), ("step", step)] {
                node.params
                    .insert(field.to_owned(), graph::value::number(x));
            }
        }
        self.revision += 1;
    }

    /// Position of every step in the node sequence.
    pub(crate) fn steps(&self) -> HashMap<NodeId, usize> {
        Self::ranks(self.engine.nodes.iter().map(|node| (node.id, &node.kind)))
    }

    pub(crate) fn ranks<'a>(
        nodes: impl Iterator<Item = (NodeId, &'a Kind)>,
    ) -> HashMap<NodeId, usize> {
        nodes
            .filter(|(_, kind)| !is_input(kind))
            .enumerate()
            .map(|(index, (id, _))| (id, index))
            .collect()
    }

    /// The links a node order would break: those that would read from a
    /// later step, or feed an input.
    pub(crate) fn broken_links(&self, steps: &HashMap<NodeId, usize>) -> Vec<Link> {
        let inputs: HashSet<NodeId> = self
            .engine
            .nodes
            .iter()
            .filter(|node| is_input(&node.kind))
            .map(|node| node.id)
            .collect();
        self.engine
            .links
            .iter()
            .filter(|link| {
                if inputs.contains(&link.to.node) {
                    return true;
                }
                match (steps.get(&link.from.node), steps.get(&link.to.node)) {
                    (Some(from), Some(to)) => from >= to,
                    _ => !inputs.contains(&link.from.node),
                }
            })
            .cloned()
            .collect()
    }

    /// Drops every link that no longer reads from an earlier step or from an
    /// input, so data only flows forward along the node sequence.
    pub(crate) fn prune_links(&mut self) {
        let broken = self.broken_links(&self.steps());
        self.engine.links.retain(|link| {
            !broken
                .iter()
                .any(|gone| gone.from == link.from && gone.to == link.to)
        });
    }

    /// Forgets groups nothing belongs to any more.
    pub(crate) fn prune_groups(&mut self) {
        let used: HashSet<GroupId> = self.ui.values().filter_map(|ui| ui.group).collect();
        self.groups.retain(|group| used.contains(&group.id));
    }

    /// Indices in `engine.nodes` of the nodes in `id`'s area.
    fn area(&self, id: NodeId) -> Vec<usize> {
        let input = self
            .engine
            .node(id)
            .is_some_and(|node| is_input(&node.kind));
        self.engine
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| is_input(&node.kind) == input)
            .map(|(index, _)| index)
            .collect()
    }

    /// What moving `id` one place does: crossing into a neighbouring group
    /// (or out of its own) first changes its group in place, so groups stay
    /// together; otherwise it swaps with that neighbour.
    fn plan_move(&self, id: NodeId, direction: i32) -> Option<Result<usize, Option<GroupId>>> {
        let area = self.area(id);
        let k = area
            .iter()
            .position(|index| self.engine.nodes[*index].id == id)?;
        let neighbour = *k
            .checked_add_signed(direction as isize)
            .and_then(|k| area.get(k))?;
        let other = self.node_group(self.engine.nodes[neighbour].id);
        if other != self.node_group(id) {
            return Some(Err(other));
        }
        Some(Ok(neighbour))
    }

    /// Links a move would break.
    fn move_breaks(&self, id: NodeId, direction: i32) -> Vec<Link> {
        let Some(Ok(neighbour)) = self.plan_move(id, direction) else {
            return Vec::new();
        };
        let Some(index) = self.engine.nodes.iter().position(|node| node.id == id) else {
            return Vec::new();
        };
        let mut order: Vec<(NodeId, &Kind)> = self
            .engine
            .nodes
            .iter()
            .map(|node| (node.id, &node.kind))
            .collect();
        order.swap(index, neighbour);
        self.broken_links(&Self::ranks(order.into_iter()))
    }

    fn move_node(&mut self, id: NodeId, direction: i32) {
        match self.plan_move(id, direction) {
            Some(Ok(neighbour)) => {
                if let Some(index) = self.engine.nodes.iter().position(|node| node.id == id) {
                    self.engine.nodes.swap(index, neighbour);
                }
                self.prune_links();
            }
            Some(Err(group)) => {
                if let Some(ui) = self.ui.get_mut(&id) {
                    ui.group = group;
                }
                self.prune_groups();
            }
            None => {}
        }
    }

    /// The editor's starting point for composite node `id`.
    fn script_draft(&self, id: NodeId) -> Option<ScriptDraft> {
        let Kind::Script(script) = &self.engine.node(id)?.kind else {
            return None;
        };
        Some(ScriptDraft {
            node: id,
            name: script.name.clone(),
            content: iced::widget::text_editor::Content::with_text(&script.source),
            checked: Some(script_check(&script.source)),
        })
    }

    /// Gives script node `id` a new program: values of inputs it keeps
    /// stay, new inputs start at their defaults, and links to ports it no
    /// longer has go.
    pub(crate) fn apply_script(&mut self, id: NodeId, name: String, source: String) {
        let name = if name.trim().is_empty() {
            t!("Script").into_owned()
        } else {
            name.trim().to_owned()
        };
        let script = Script::new(name, source);
        let Some(node) = self.engine.node_mut(id) else {
            return;
        };
        let mut params = script.defaults();
        for (field, value) in params.iter_mut() {
            if let Some(kept) = node.params.get(field).filter(|kept| !kept.is_null()) {
                *value = kept.clone();
            }
        }
        node.params = params;
        node.kind = Kind::Script(script.clone());
        self.engine.links.retain(|link| {
            (link.to.node != id || script.inputs.iter().any(|port| port.name == link.to.field))
                && (link.from.node != id
                    || script
                        .outputs
                        .iter()
                        .any(|port| port.name == link.from.field))
        });
        self.revision += 1;
    }

    fn composite_draft(&self, id: NodeId) -> Option<CompositeDraft> {
        let node = self.engine.node(id)?;
        let Kind::Composite(composite) = &node.kind else {
            return None;
        };
        Some(CompositeDraft {
            node: id,
            name: composite.name.clone(),
            description: composite_description(composite).to_owned(),
            inputs: composite
                .inputs
                .iter()
                .map(|(name, port)| {
                    let default = display(&composite.graph.input(port));
                    (name.clone(), name.clone(), default)
                })
                .collect(),
            outputs: composite
                .outputs
                .iter()
                .map(|(name, _)| (name.clone(), name.clone()))
                .collect(),
        })
    }

    /// Applies an edited composite to its node: the new name, description
    /// and defaults, and ports renamed together with the links on them.
    /// A blank or repeated port name keeps the old one.
    fn apply_composite_draft(&mut self, draft: CompositeDraft) {
        let id = draft.node;
        let Some(Kind::Composite(composite)) = self.engine.node(id).map(|node| node.kind.clone())
        else {
            return;
        };
        let mut composite = (*composite).clone();
        let renames = |ports: &mut Vec<(String, Port)>, wanted: Vec<(String, String)>| {
            let mut taken: HashSet<String> = HashSet::default();
            let mut changed = Vec::new();
            for ((name, _), (old, new)) in ports.iter_mut().zip(wanted) {
                let new = new.trim().to_owned();
                let name_now = if new.is_empty() || taken.contains(&new.to_lowercase()) {
                    old.clone()
                } else {
                    new
                };
                taken.insert(name_now.to_lowercase());
                if name_now != old {
                    changed.push((old, name_now.clone()));
                }
                *name = name_now;
            }
            changed
        };
        let input_renames = renames(
            &mut composite.inputs,
            draft
                .inputs
                .iter()
                .map(|(old, new, _)| (old.clone(), new.clone()))
                .collect(),
        );
        let output_renames = renames(
            &mut composite.outputs,
            draft
                .outputs
                .iter()
                .map(|(old, new)| (old.clone(), new.clone()))
                .collect(),
        );
        // Defaults are the inner values the exposed inputs start from.
        let mut defaults = Vec::new();
        for ((name, port), (_, _, text)) in composite.inputs.clone().iter().zip(&draft.inputs) {
            let current = display(&composite.graph.input(port));
            if *text == current {
                continue;
            }
            let value = text
                .trim()
                .parse::<f64>()
                .map_or_else(|_| Value::String(text.clone()), graph::value::number);
            if let Some(inner) = composite.graph.node_mut(port.node) {
                inner.params.insert(port.field.clone(), value.clone());
            }
            defaults.push((name.clone(), value));
        }
        if !draft.name.trim().is_empty() {
            composite.name = draft.name.trim().to_owned();
        }
        if !composite.meta.is_object() {
            composite.meta = json!({});
        }
        composite.meta["description"] = Value::String(draft.description.trim().to_owned());
        for link in &mut self.engine.links {
            if link.to.node == id {
                if let Some((_, new)) = input_renames.iter().find(|(old, _)| *old == link.to.field)
                {
                    link.to.field = new.clone();
                }
            }
            if link.from.node == id {
                if let Some((_, new)) = output_renames
                    .iter()
                    .find(|(old, _)| *old == link.from.field)
                {
                    link.from.field = new.clone();
                }
            }
        }
        if let Some(node) = self.engine.node_mut(id) {
            for (old, new) in &input_renames {
                if let Some(value) = node.params.remove(old) {
                    node.params.insert(new.clone(), value);
                }
            }
            for (name, value) in defaults {
                node.params.insert(name, value);
            }
            node.kind = Kind::Composite(Arc::new(composite));
        }
    }

    /// Whether `id` can be dropped on `target`: another node of its area.
    fn can_move_to(&self, id: NodeId, target: NodeId) -> bool {
        let input = |id: NodeId| self.engine.node(id).map(|node| is_input(&node.kind));
        id != target && input(id).is_some() && input(id) == input(target)
    }

    /// The node order with `id` moved to `target`'s place: after it when
    /// moving later, before it when moving earlier.
    fn order_moved_to(&self, id: NodeId, target: NodeId) -> Option<Vec<usize>> {
        let from = self.engine.nodes.iter().position(|node| node.id == id)?;
        let to = self
            .engine
            .nodes
            .iter()
            .position(|node| node.id == target)?;
        let mut order: Vec<usize> = (0..self.engine.nodes.len()).collect();
        let moved = order.remove(from);
        order.insert(to, moved);
        Some(order)
    }

    /// Links dropping `id` on `target` would break.
    fn move_to_breaks(&self, id: NodeId, target: NodeId) -> Vec<Link> {
        let Some(order) = self.order_moved_to(id, target) else {
            return Vec::new();
        };
        let nodes = &self.engine.nodes;
        self.broken_links(&Self::ranks(
            order.iter().map(|&k| (nodes[k].id, &nodes[k].kind)),
        ))
    }

    /// Moves `id` to `target`'s place, into `target`'s group.
    fn move_to(&mut self, id: NodeId, target: NodeId) {
        let Some(order) = self.order_moved_to(id, target) else {
            return;
        };
        let mut nodes: Vec<Option<graph::Node>> = std::mem::take(&mut self.engine.nodes)
            .into_iter()
            .map(Some)
            .collect();
        self.engine.nodes = order.iter().filter_map(|&k| nodes[k].take()).collect();
        let group = self.node_group(target);
        if let Some(ui) = self.ui.get_mut(&id) {
            ui.group = group;
        }
        self.prune_links();
        self.prune_groups();
    }

    /// Whether `id` can move that way at all.
    fn can_move(&self, id: NodeId, direction: i32) -> bool {
        self.plan_move(id, direction).is_some()
    }

    /// The links reading from `id`.
    pub(crate) fn dependents(&self, id: NodeId) -> Vec<Link> {
        self.engine
            .links
            .iter()
            .filter(|link| link.from.node == id)
            .cloned()
            .collect()
    }

    fn new_group(&mut self, id: NodeId) {
        let input = self
            .engine
            .node(id)
            .is_some_and(|node| is_input(&node.kind));
        let group = self.groups.iter().map(|group| group.id).max().unwrap_or(0) + 1;
        let name = (1..)
            .map(|n| format!("{} {n}", t!("Group")))
            .find(|name| !self.groups.iter().any(|group| &group.name == name))
            .unwrap_or_default();
        self.groups.push(NodeGroup {
            id: group,
            name,
            color: self.groups.len() + 2,
            collapsed: false,
            inputs: input,
        });
        if let Some(ui) = self.ui.get_mut(&id) {
            ui.group = Some(group);
        }
        self.prune_groups();
    }

    /// Turns the steps `members` into one composite node called `name`, where
    /// the last of them was; no undo step is recorded. Steps between the
    /// members that do not feed them move after the composite first.
    pub(crate) fn extract_members(
        &mut self,
        members: &[NodeId],
        name: &str,
    ) -> Result<NodeId, String> {
        let members: Vec<NodeId> = self
            .engine
            .nodes
            .iter()
            .filter(|node| members.contains(&node.id))
            .map(|node| node.id)
            .collect();
        if members.is_empty() {
            return Err(t!("Choose the steps to put in the composite").into_owned());
        }
        for id in &members {
            match self.engine.node(*id).map(|node| &node.kind) {
                Some(Kind::Object(_)) => {
                    return Err(crate::tf!(
                        "{} draws objects; a composite holds operations only",
                        self.name(*id)
                    )
                    .into_owned())
                }
                Some(kind) if is_input(kind) => {
                    return Err(
                        crate::tf!("{} is an input; inputs stay outside", self.name(*id))
                            .into_owned(),
                    )
                }
                _ => {}
            }
        }
        let position =
            |graph: &graph::Graph, id: NodeId| graph.nodes.iter().position(|node| node.id == id);
        let (Some(first), Some(last)) = (
            position(&self.engine, members[0]),
            position(&self.engine, members[members.len() - 1]),
        ) else {
            return Err(t!("Choose the steps to put in the composite").into_owned());
        };
        let movers: Vec<NodeId> = self.engine.nodes[first..=last]
            .iter()
            .map(|node| node.id)
            .filter(|id| !members.contains(id))
            .filter(|id| {
                !members
                    .iter()
                    .any(|member| self.engine.reaches(*id, *member))
            })
            .collect();
        let moved: Vec<graph::Node> = movers
            .iter()
            .filter_map(|id| {
                let at = position(&self.engine, *id)?;
                Some(self.engine.nodes.remove(at))
            })
            .collect();
        if let Some(at) = position(&self.engine, members[members.len() - 1]) {
            for (k, node) in moved.into_iter().enumerate() {
                self.engine.nodes.insert(at + 1 + k, node);
            }
        }
        let names: HashMap<NodeId, String> =
            members.iter().map(|id| (*id, self.name(*id))).collect();
        // Inner ids count up from one in the members' order.
        let meta = json!({
            "names": members
                .iter()
                .enumerate()
                .map(|(k, id)| ((k + 1).to_string(), json!(names[id])))
                .collect::<serde_json::Map<String, Value>>(),
        });
        let id = self.engine.extract(
            &members,
            name,
            &|port: &Port| format!("{} {}", names[&port.node], port.field),
            meta,
        )?;
        for member in &members {
            self.ui.remove(member);
            self.drafts.retain(|port, _| port.node != *member);
        }
        let node_name = self.unique_name(name, None, false);
        let mut open = HashSet::default();
        open.insert(t!("Geometry").into_owned());
        self.ui.insert(
            id,
            NodeUi {
                name: node_name,
                open,
                handles: Vec::new(),
                group: None,
            },
        );
        self.prune_groups();
        self.prune_links();
        Ok(id)
    }

    /// Makes a group's steps one composite node named after the group, as
    /// one undo step.
    pub fn make_composite(&mut self, group: GroupId) -> Result<NodeId, String> {
        let name = self
            .group(group)
            .map(|group| group.name.clone())
            .unwrap_or_else(|| t!("Composite").into_owned());
        let members: Vec<NodeId> = self
            .engine
            .nodes
            .iter()
            .filter(|node| self.node_group(node.id) == Some(group))
            .map(|node| node.id)
            .collect();
        let before = self.snapshot();
        match self.extract_members(&members, &name) {
            Ok(id) => {
                self.push_undo(before);
                Ok(id)
            }
            Err(error) => {
                self.restore_content(before);
                Err(error)
            }
        }
    }

    /// Puts composite `id`'s nodes back where it was, with the names they
    /// had, gathered in a group named after it; no undo step is recorded.
    pub(crate) fn expand_node(&mut self, id: NodeId) -> Result<Vec<NodeId>, String> {
        let Some(Kind::Composite(composite)) = self.engine.node(id).map(|node| node.kind.clone())
        else {
            return Err(crate::tf!("{} is not a composite", self.name(id)).into_owned());
        };
        let title = self.name(id);
        let ids = self.engine.expand(id)?;
        self.ui.remove(&id);
        self.drafts.retain(|port, _| port.node != id);
        let mut added: Vec<NodeId> = ids.values().copied().collect();
        added.sort_by_key(|new| self.engine.nodes.iter().position(|node| node.id == *new));
        for (inner, new) in &ids {
            let wanted = composite.meta["names"][inner.to_string()]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    self.engine
                        .node(*new)
                        .map(|node| kind_name(&node.kind))
                        .unwrap_or_default()
                });
            let name = self.unique_name(&wanted, None, false);
            let mut open = HashSet::default();
            open.insert(t!("Geometry").into_owned());
            self.ui.insert(
                *new,
                NodeUi {
                    name,
                    open,
                    handles: Vec::new(),
                    group: None,
                },
            );
        }
        if let Some(first) = added.first() {
            self.new_group(*first);
            if let Some(group) = self.node_group(*first) {
                if let Some(known) = self.groups.iter_mut().find(|known| known.id == group) {
                    known.name = title;
                }
                for new in &added {
                    if let Some(ui) = self.ui.get_mut(new) {
                        ui.group = Some(group);
                    }
                }
            }
        }
        self.prune_links();
        Ok(added)
    }

    /// Expands composite `id` as one undo step.
    pub fn expand_composite(&mut self, id: NodeId) -> Result<Vec<NodeId>, String> {
        let before = self.snapshot();
        match self.expand_node(id) {
            Ok(ids) => {
                self.push_undo(before);
                Ok(ids)
            }
            Err(error) => {
                self.restore_content(before);
                Err(error)
            }
        }
    }

    /// Rows of an operation node, read from its spec. An input sharing its
    /// name with an output is one row with both ports.
    fn op_sections(&self, id: NodeId, spec: &'static Spec) -> Vec<NodeSection> {
        let Some(node) = self.engine.node(id) else {
            return Vec::new();
        };
        let param = |name: &str| node.params.get(name).and_then(graph::value::num);
        let mut rows: Vec<NodeRow> = spec
            .inputs
            .iter()
            .map(|input| {
                let port = Port {
                    node: id,
                    field: input.name.to_owned(),
                };
                let widget = match (spec.name, input.name) {
                    ("Number Slider" | "Integer Slider", "value") => RowWidget::Slider {
                        min: param("min").unwrap_or(0.0),
                        max: param("max").unwrap_or(1.0),
                        step: param("step").unwrap_or(0.0),
                    },
                    ("Boolean", "value") => RowWidget::Toggle,
                    ("Table", "table") => RowWidget::Table,
                    ("Table", "header") => RowWidget::Toggle,
                    _ => RowWidget::Field,
                };
                let output = spec.outputs.contains(&input.name);
                NodeRow {
                    field: input.name.to_owned(),
                    label: input.name.to_owned(),
                    value: if output {
                        node.outputs.get(input.name).cloned().unwrap_or(Value::Null)
                    } else {
                        self.engine.input(&port)
                    },
                    input: true,
                    output,
                    widget,
                    kind: graph::docs::input_type(spec, input.name),
                }
            })
            .collect();
        // A table's columns take the names its first row gives them.
        let headers: Vec<String> = match spec.name {
            "Table"
                if graph::value::boolean(&self.engine.input(&Port {
                    node: id,
                    field: "header".to_owned(),
                }))
                .unwrap_or(true) =>
            {
                graph::value::table(&self.engine.input(&Port {
                    node: id,
                    field: "table".to_owned(),
                }))
                .first()
                .map(|row| row.iter().map(graph::value::text).collect())
                .unwrap_or_default()
            }
            _ => Vec::new(),
        };
        rows.extend(
            spec.outputs
                .iter()
                .filter(|name| !spec.inputs.iter().any(|input| input.name == **name))
                .map(|name| NodeRow {
                    field: (*name).to_owned(),
                    label: name
                        .strip_prefix("column ")
                        .and_then(|k| k.parse::<usize>().ok())
                        .and_then(|k| headers.get(k - 1))
                        .filter(|header| !header.trim().is_empty())
                        .cloned()
                        .unwrap_or_else(|| (*name).to_owned()),
                    value: node.outputs.get(*name).cloned().unwrap_or(Value::Null),
                    input: false,
                    output: true,
                    widget: RowWidget::Field,
                    kind: graph::docs::output_type(spec, name),
                }),
        );
        vec![NodeSection {
            title: String::new(),
            rows,
        }]
    }

    /// Every node's rows, parallel to `engine.nodes`. `objects` holds the
    /// object nodes' rows, which only the app can build.
    pub fn sections(
        &self,
        mut objects: HashMap<NodeId, Vec<NodeSection>>,
    ) -> Vec<Vec<NodeSection>> {
        self.engine
            .nodes
            .iter()
            .map(|node| match node.kind {
                Kind::Op(spec) => self.op_sections(node.id, spec),
                Kind::Object(_) => objects.remove(&node.id).unwrap_or_default(),
                Kind::Composite(ref composite) => self.composite_sections(node.id, composite),
                Kind::Script(ref script) => self.script_sections(node.id, script),
            })
            .collect()
    }

    /// A composite's rows: its exposed inputs, then its outputs.
    fn composite_sections(&self, id: NodeId, composite: &Composite) -> Vec<NodeSection> {
        let Some(node) = self.engine.node(id) else {
            return Vec::new();
        };
        let inputs = composite.inputs.iter().map(|(name, _)| NodeRow {
            field: name.clone(),
            label: name.clone(),
            value: self.engine.input(&Port {
                node: id,
                field: name.clone(),
            }),
            input: true,
            output: false,
            widget: RowWidget::Field,
            kind: port_type(&node.kind, name, true),
        });
        let outputs = composite.outputs.iter().map(|(name, _)| NodeRow {
            field: name.clone(),
            label: name.clone(),
            value: node.outputs.get(name).cloned().unwrap_or(Value::Null),
            input: false,
            output: true,
            widget: RowWidget::Field,
            kind: port_type(&node.kind, name, false),
        });
        vec![NodeSection {
            title: String::new(),
            rows: inputs.chain(outputs).collect(),
        }]
    }

    /// A script's rows: the inputs and outputs its header declares.
    fn script_sections(&self, id: NodeId, script: &Script) -> Vec<NodeSection> {
        let Some(node) = self.engine.node(id) else {
            return Vec::new();
        };
        let inputs = script.inputs.iter().map(|port| NodeRow {
            field: port.name.clone(),
            label: port.name.clone(),
            value: self.engine.input(&Port {
                node: id,
                field: port.name.clone(),
            }),
            input: true,
            output: false,
            widget: RowWidget::Field,
            kind: Some(port.kind),
        });
        let outputs = script.outputs.iter().map(|port| NodeRow {
            field: port.name.clone(),
            label: port.name.clone(),
            value: node.outputs.get(&port.name).cloned().unwrap_or(Value::Null),
            input: false,
            output: true,
            widget: RowWidget::Field,
            kind: Some(port.kind),
        });
        vec![NodeSection {
            title: String::new(),
            rows: inputs.chain(outputs).collect(),
        }]
    }

    fn is_open(&self, id: NodeId, section: &NodeSection) -> bool {
        section.title.is_empty()
            || self
                .ui
                .get(&id)
                .is_some_and(|ui| ui.open.contains(&section.title))
    }

    /// The row of `port` among `sections`.
    fn port_row<'s>(
        &self,
        sections: &'s [Vec<NodeSection>],
        port: &Port,
        output: bool,
    ) -> Option<&'s NodeRow> {
        let index = self
            .engine
            .nodes
            .iter()
            .position(|node| node.id == port.node)?;
        sections
            .get(index)?
            .iter()
            .flat_map(|section| &section.rows)
            .find(|row| row.field == port.field && if output { row.output } else { row.input })
    }

    /// The row label of `port` among `sections`, falling back to its field.
    fn port_label(&self, sections: &[Vec<NodeSection>], port: &Port, output: bool) -> String {
        self.port_row(sections, port, output).map_or_else(
            || port.field.clone(),
            |row| t!(row.label.as_str()).into_owned(),
        )
    }

    /// `Node › Row` for an output, as the picker and step rows name it.
    fn source_name(&self, sections: &[Vec<NodeSection>], port: &Port) -> String {
        format!(
            "{} › {}",
            self.name(port.node),
            self.port_label(sections, port, true)
        )
    }

    /// What `target` accepts: an operation's spec says so; an object row by
    /// the kind of value it holds now.
    pub(crate) fn expect(&self, target: &Port, value: &Value) -> Expect {
        match self.engine.node(target.node).map(|node| &node.kind) {
            Some(Kind::Op(spec)) => spec_expect(spec, &target.field),
            Some(Kind::Composite(composite)) => composite_expect(composite, &target.field),
            Some(Kind::Script(script)) => {
                script_type(script, &target.field, true).map_or(Expect::Any, Expect::of_type)
            }
            Some(Kind::Object(_)) => Expect::of_value(value),
            None => Expect::Any,
        }
    }

    pub fn view(&self, sections: Vec<Vec<NodeSection>>) -> Element<'_, Message> {
        let overlay = if self.steps_hidden {
            None
        } else if let Some(confirm) = &self.confirm {
            Some(self.confirm_dialog(confirm, &sections))
        } else if let Some(draft) = &self.editing {
            Some(self.composite_editor(draft))
        } else if let Some(draft) = &self.scripting {
            Some(self.script_editor(draft))
        } else {
            self.picking
                .as_ref()
                .map(|target| self.picker(target, &sections))
        };
        let sources: HashMap<Port, (NodeId, String)> = self
            .engine
            .links
            .iter()
            .map(|link| {
                (
                    link.to.clone(),
                    (link.from.node, self.source_name(&sections, &link.from)),
                )
            })
            .collect();

        let mut inputs: Vec<(&graph::Node, Vec<NodeSection>)> = Vec::new();
        let mut steps: Vec<(&graph::Node, Vec<NodeSection>)> = Vec::new();
        for (node, node_sections) in self.engine.nodes.iter().zip(sections) {
            if is_input(&node.kind) {
                inputs.push((node, node_sections));
            } else {
                steps.push((node, node_sections));
            }
        }
        let step_count = steps.len();

        let inputs_body: Element<'_, Message> = if inputs.is_empty() {
            text(t!(
                "Add Number, Slider, Text or Boolean nodes from the Input category. Their values are the only ones typed by hand."
            ))
            .size(11)
            .style(weak_text)
            .into()
        } else {
            let mut list = column![].spacing(8);
            for (group, members) in self.runs(inputs) {
                let cards: Vec<Element<'_, Message>> = members
                    .into_iter()
                    .map(|(node, node_sections)| self.input_card(node, node_sections))
                    .collect();
                list = list.push(match group {
                    Some(group) => {
                        let count = cards.len();
                        let body = column(cards).spacing(8).into();
                        self.group_frame(group, count, body)
                    }
                    None => column(cards).spacing(8).into(),
                });
            }
            list.into()
        };
        let (eye, eye_label) = if self.steps_hidden {
            (crate::ui::icons::EYE, "Show nodes")
        } else {
            (crate::ui::icons::EYE_OFF, "Hide nodes")
        };
        let toggle = button(
            row![
                crate::ui::icons::themed(eye, 12.0),
                text(t!(eye_label)).size(11)
            ]
            .spacing(4)
            .align_y(iced::Center),
        )
        .on_press(msg(GraphMsg::ToggleSteps))
        .style(button::secondary)
        .padding([3, 8]);
        let inputs_area = container(
            column![
                row![text(t!("Inputs")).size(13).width(Length::Fill), toggle].align_y(iced::Center),
                scrollable(container(inputs_body).padding(iced::Padding {
                    top: 0.0,
                    right: 10.0,
                    bottom: 0.0,
                    left: 0.0,
                }))
                .id(iced::widget::Id::new(INPUTS_SCROLL_ID))
                .height(Length::Fill),
            ]
            .spacing(8),
        )
        .padding(12)
        .width(INPUTS_W)
        .height(Length::Fill)
        .style(|theme: &Theme| {
            let palette = theme.palette();
            container::Style {
                background: Some(Background::Color(
                    palette.background.weak.color.scale_alpha(0.92),
                )),
                border: Border {
                    color: palette.background.strong.color,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            }
        });

        // Hidden steps leave the drawing uncovered and clickable beside the
        // Inputs area.
        if self.steps_hidden {
            return container(opaque(inputs_area))
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }

        let arrow = || {
            container(crate::ui::icons::themed(
                crate::ui::icons::ARROW_LONG_RIGHT,
                18.0,
            ))
            .padding([12, 6])
        };
        let sequence_body: Element<'_, Message> = if steps.is_empty() {
            container(
                text(t!(
                    "Add objects or operations from the Node Graph panel, or select drawn objects and press From drawing. Steps run from left to right, and each one takes its values from an earlier step or from the inputs."
                ))
                .size(11)
                .style(weak_text),
            )
            .width(360)
            .into()
        } else {
            let mut track = row![].spacing(0).align_y(iced::Top);
            let mut step = 0;
            for (index, (group, members)) in self.runs(steps).into_iter().enumerate() {
                if index > 0 {
                    track = track.push(arrow());
                }
                let mut lane = row![].spacing(0).align_y(iced::Top);
                let count = members.len();
                for (k, (node, node_sections)) in members.into_iter().enumerate() {
                    if k > 0 {
                        lane = lane.push(arrow());
                    }
                    step += 1;
                    lane =
                        lane.push(self.step_card(step, step_count, node, node_sections, &sources));
                }
                track = track.push(match group {
                    Some(group) => self.group_frame(group, count, lane.into()),
                    None => lane.into(),
                });
            }
            Scaled {
                content: track.into(),
                zoom: self.zoom(),
            }
            .into()
        };
        let zoom_button = |icon: &'static [u8], tip: &str, steps: i32, enabled: bool| {
            icon_button(icon, tip, enabled.then_some(GraphMsg::Zoom(steps)))
        };
        let matches = self.search_matches();
        let mut finder = row![
            crate::ui::icons::themed(crate::ui::icons::SEARCH, 11.0),
            text_input(&t!("Find node"), &self.node_search)
                .id(iced::widget::Id::new(NODE_SEARCH_ID))
                .on_input(|value| msg(GraphMsg::NodeSearch(value)))
                .on_submit(msg(GraphMsg::NodeSearchStep(1)))
                .size(11)
                .padding([3, 5])
                .width(130),
        ]
        .spacing(4)
        .align_y(iced::Center);
        if !self.node_search.trim().is_empty() {
            let position = self
                .found
                .and_then(|id| matches.iter().position(|found| *found == id));
            finder = finder.push(
                text(match position {
                    Some(k) => format!("{}/{}", k + 1, matches.len()),
                    None => t!("No match").into_owned(),
                })
                .size(10)
                .style(weak_text),
            );
            let several = matches.len() > 1;
            finder = finder.push(icon_button(
                crate::ui::icons::TRI_LEFT,
                "Previous match",
                several.then_some(GraphMsg::NodeSearchStep(-1)),
            ));
            finder = finder.push(icon_button(
                crate::ui::icons::TRI_RIGHT,
                "Next match (Enter)",
                several.then_some(GraphMsg::NodeSearchStep(1)),
            ));
        }
        let toolbar = row![
            finder,
            Space::new().width(10),
            tooltip(
                button(
                    row![
                        crate::ui::icons::themed(crate::ui::icons::EYEDROPPER, 11.0),
                        text(t!("From drawing")).size(11)
                    ]
                    .spacing(4)
                    .align_y(iced::Center),
                )
                .on_press(msg(GraphMsg::FromSelection))
                .style(button::secondary)
                .padding([3, 8]),
                container(text(t!("Add a node for each selected object in the drawing")).size(10))
                    .padding([2, 6])
                    .style(container::bordered_box),
                tooltip::Position::Bottom,
            )
            .gap(4),
            tooltip(
                button(text(t!("Paste nodes")).size(11))
                    .on_press(msg(GraphMsg::Paste))
                    .style(button::secondary)
                    .padding([3, 8]),
                container(
                    text(t!("Add the nodes copied with their copy button, from this graph or another drawing")).size(10)
                )
                .padding([2, 6])
                .style(container::bordered_box),
                tooltip::Position::Bottom,
            )
            .gap(4),
            Space::new().width(10),
            icon_button(
                if self.compact {
                    crate::ui::icons::GRID
                } else {
                    crate::ui::icons::LIST
                },
                if self.compact {
                    "Full cards"
                } else {
                    "Compact cards: results only"
                },
                Some(GraphMsg::ToggleCompact)
            ),
            icon_button(
                crate::ui::icons::PREVIEW,
                if self.preview_hidden {
                    "Show preview"
                } else {
                    "Hide preview"
                },
                Some(GraphMsg::TogglePreview)
            ),
            icon_button(
                crate::ui::icons::UNDO,
                "Undo graph change",
                self.can_undo().then_some(GraphMsg::Undo)
            ),
            icon_button(
                crate::ui::icons::REDO,
                "Redo graph change",
                self.can_redo().then_some(GraphMsg::Redo)
            ),
            Space::new().width(10),
            zoom_button(
                crate::ui::icons::MINUS,
                "Zoom out (Ctrl + wheel)",
                -1,
                self.zoom_steps > *ZOOM_STEPS.start()
            ),
            button(text(format!("{:.0}%", self.zoom() * 100.0)).size(11))
                .on_press(msg(GraphMsg::Zoom(0)))
                .style(button::text)
                .padding([3, 6]),
            zoom_button(
                crate::ui::icons::PLUS,
                "Zoom in (Ctrl + wheel)",
                1,
                self.zoom_steps < *ZOOM_STEPS.end()
            ),
        ]
        .spacing(2)
        .align_y(iced::Center);
        let paused: Element<'_, Message> = if self.scripts_paused {
            container(
                row![
                    crate::ui::icons::themed(crate::ui::icons::WARNING, 12.0),
                    text(t!(
                        "This graph holds scripts you have not run before. It stays as it was saved until you run them."
                    ))
                    .size(11)
                    .width(Length::Fill),
                    button(text(t!("Run scripts")).size(11))
                        .on_press(msg(GraphMsg::TrustScripts))
                        .style(button::primary)
                        .padding([3, 10]),
                ]
                .spacing(8)
                .align_y(iced::Center),
            )
            .padding([6, 8])
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(ISSUE_COLOR.scale_alpha(0.15))),
                border: Border {
                    color: ISSUE_COLOR,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            })
            .into()
        } else {
            Space::new().height(0).into()
        };
        let sequence = column![
            row![
                text(t!("Node Sequence")).size(13).width(Length::Fill),
                toolbar
            ]
            .align_y(iced::Center),
            paused,
            scrollable(container(sequence_body).padding(iced::Padding {
                top: 0.0,
                right: 12.0,
                bottom: 4.0,
                left: 0.0,
            }))
            .id(iced::widget::Id::new(SEQUENCE_SCROLL_ID))
            .direction(scrollable::Direction::Both {
                vertical: scrollable::Scrollbar::default(),
                horizontal: scrollable::Scrollbar::default(),
            })
            .width(Length::Fill)
            // As tall as the nodes, up to the space there is, so the
            // horizontal scrollbar sits right under them.
            .height(Length::Shrink),
        ]
        .spacing(8)
        .padding(12);

        let backdrop = container(row![inputs_area, sequence])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|theme: &Theme| container::Style {
                background: Some(Background::Color(
                    theme.palette().background.base.color.scale_alpha(0.6),
                )),
                ..Default::default()
            });
        // A drag released away from every card goes nowhere.
        let backdrop: Element<'_, Message> = if self.dragging.is_some() {
            mouse_area(backdrop)
                .on_release(msg(GraphMsg::DragCancel))
                .interaction(mouse::Interaction::Grabbing)
                .into()
        } else {
            backdrop.into()
        };
        let mut layers = stack![backdrop];
        if let Some(overlay) = overlay {
            layers = layers.push(overlay);
        }
        opaque(layers.width(Length::Fill).height(Length::Fill))
    }

    /// A card that highlights its geometry in the drawing while the pointer
    /// is over it, and takes a dragged card dropped on it, edged in the
    /// accent while one is.
    fn hover_target<'a>(&self, id: NodeId, card: Element<'a, Message>) -> Element<'a, Message> {
        let target = self.dragging.is_some_and(|dragged| dragged != id) && self.hovered == Some(id);
        // The node the find box shows is ringed; its other matches edged.
        let found = self.found == Some(id);
        let matched = !found && self.search_matches().contains(&id);
        let framed = container(card).padding(2).style(move |theme: &Theme| {
            let primary = theme.palette().primary;
            let (color, width) = if target || found {
                (primary.base.color, 2.0)
            } else if matched {
                (primary.base.color.scale_alpha(0.45), 1.0)
            } else {
                return container::Style::default();
            };
            container::Style {
                border: Border {
                    color,
                    width,
                    radius: 8.0.into(),
                },
                ..Default::default()
            }
        });
        let area = mouse_area(framed)
            .on_enter(msg(GraphMsg::Hover(Some(id))))
            .on_exit(msg(GraphMsg::HoverEnd(id)));
        if self.dragging.is_some() {
            area.on_release(msg(GraphMsg::DragDrop(id))).into()
        } else {
            area.into()
        }
    }

    /// Splits nodes into runs of consecutive members of one group (or of
    /// none).
    fn runs<'n>(
        &self,
        nodes: Vec<(&'n graph::Node, Vec<NodeSection>)>,
    ) -> Vec<(Option<&NodeGroup>, Vec<(&'n graph::Node, Vec<NodeSection>)>)> {
        let mut runs: Vec<(Option<GroupId>, Vec<(&'n graph::Node, Vec<NodeSection>)>)> = Vec::new();
        for (node, node_sections) in nodes {
            let group = self.node_group(node.id);
            match runs.last_mut() {
                Some((last, members)) if *last == group => members.push((node, node_sections)),
                _ => runs.push((group, vec![(node, node_sections)])),
            }
        }
        runs.into_iter()
            .map(|(group, members)| (group.and_then(|group| self.group(group)), members))
            .collect()
    }

    /// A group's frame: a tinted panel with its colour chip, name (click to
    /// rename), size, collapse, colour and ungroup controls over `body`.
    fn group_frame<'a>(
        &'a self,
        group: &NodeGroup,
        count: usize,
        body: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let color = group_color(group);
        let id = group.id;
        let name: Element<'_, Message> = match &self.renaming {
            Some((Renaming::Group(renaming), draft)) if *renaming == id => text_input("", draft)
                .id(iced::widget::Id::new(RENAME_ID))
                .on_input(|value| msg(GraphMsg::RenameInput(value)))
                .on_submit(msg(GraphMsg::RenameCommit))
                .size(12)
                .padding([2, 4])
                .width(180)
                .into(),
            _ => tooltip(
                button(
                    text(group.name.clone())
                        .size(12)
                        .style(move |_: &Theme| text::Style { color: Some(color) }),
                )
                .on_press(msg(GraphMsg::GroupRenameStart(id)))
                .style(button::text)
                .padding(0),
                container(text(t!("Click to rename").into_owned()).size(10))
                    .padding([2, 6])
                    .style(container::bordered_box),
                tooltip::Position::Top,
            )
            .gap(4)
            .into(),
        };
        let size = if group.inputs {
            crate::tf!("{} inputs", count)
        } else {
            crate::tf!("{} steps", count)
        };
        let swatch = tooltip(
            button(badge(color, None))
                .on_press(msg(GraphMsg::GroupColor(id)))
                .style(button::text)
                .padding(2),
            container(text(t!("Change colour").into_owned()).size(10))
                .padding([2, 6])
                .style(container::bordered_box),
            tooltip::Position::Top,
        )
        .gap(4);
        let header = row![
            button(crate::ui::icons::themed_arrow_toggle(!group.collapsed, 9.0))
                .on_press(msg(GraphMsg::GroupCollapse(id)))
                .style(button::text)
                .padding([4, 4]),
            swatch,
            name,
            text(size.into_owned()).size(10).style(weak_text),
            // A filling space on the node sequence would make the whole
            // track fill the viewport and stop it scrolling sideways.
            if group.inputs {
                Space::new().width(Length::Fill)
            } else {
                Space::new().width(12)
            },
            icon_button(
                crate::ui::icons::COPY,
                "Copy the group's nodes",
                Some(GraphMsg::CopyNodes(self.group_members(id)))
            ),
            icon_button(
                crate::ui::icons::NODE_GRAPH,
                "Make a composite node of this group",
                (!group.inputs).then_some(GraphMsg::GroupToComposite(id))
            ),
            icon_button(
                crate::ui::icons::CLOSE,
                "Ungroup",
                Some(GraphMsg::GroupDelete(id))
            ),
        ]
        .spacing(6)
        .align_y(iced::Center);
        let mut frame = column![header].spacing(6);
        if !group.collapsed {
            frame = frame.push(body);
        }
        let mut frame = container(frame)
            .padding(8)
            .style(move |_: &Theme| container::Style {
                background: Some(Background::Color(color.scale_alpha(0.10))),
                border: Border {
                    color: color.scale_alpha(0.7),
                    width: 1.5,
                    radius: 8.0.into(),
                },
                ..Default::default()
            });
        if group.inputs {
            frame = frame.width(Length::Fill);
        } else if group.collapsed {
            frame = frame.width(220);
        }
        frame.into()
    }

    /// A node's header: its colour chip (with the step number on the
    /// node sequence), its name (click to rename) over its type, then `extra`
    /// buttons, duplicate and delete.
    fn header(
        &self,
        node: &graph::Node,
        step: Option<usize>,
        extra: Vec<Element<'static, Message>>,
    ) -> Element<'_, Message> {
        let id = node.id;
        let count = self.ui.get(&id).map_or(0, |ui| ui.handles.len());
        let mut kind = kind_name(&node.kind);
        if count > 1 {
            kind = format!("{kind} ×{count}");
        }
        let name: Element<'_, Message> = match &self.renaming {
            Some((Renaming::Node(renaming), draft)) if *renaming == id => text_input("", draft)
                .id(iced::widget::Id::new(RENAME_ID))
                .on_input(|value| msg(GraphMsg::RenameInput(value)))
                .on_submit(msg(GraphMsg::RenameCommit))
                .size(12)
                .padding([2, 4])
                .into(),
            _ => tooltip(
                button(text(self.name(id)).size(12))
                    .on_press(msg(GraphMsg::RenameStart(id)))
                    .style(|theme: &Theme, status| {
                        let mut style = button::text(theme, status);
                        style.text_color = theme.palette().primary.weak.text;
                        style
                    })
                    .padding(0),
                container(text(t!("Click to rename").into_owned()).size(10))
                    .padding([2, 6])
                    .style(container::bordered_box),
                tooltip::Position::Top,
            )
            .gap(4)
            .into(),
        };
        let about = describe(&node.kind);
        let kind = text(kind).size(10).style(|theme: &Theme| text::Style {
            color: Some(theme.palette().primary.weak.text.scale_alpha(0.7)),
        });
        let kind: Element<'_, Message> = if about.is_empty() {
            kind.into()
        } else {
            tooltip(
                row![kind, crate::ui::icons::themed(crate::ui::icons::INFO, 9.0)]
                    .spacing(3)
                    .align_y(iced::Center),
                tip_box(about),
                tooltip::Position::Bottom,
            )
            .gap(4)
            .into()
        };
        let title = column![name, kind].spacing(1).width(Length::Fill);
        let mut header = row![
            badge(node_color(id), step.map(|step| step.to_string())),
            title
        ]
        .spacing(4)
        .align_y(iced::Center);
        for element in extra {
            header = header.push(element);
        }
        if matches!(node.kind, Kind::Script(_)) {
            header = header.push(icon_button(
                crate::ui::icons::MENU,
                "Edit script",
                Some(GraphMsg::ScriptEdit(id)),
            ));
            header = header.push(icon_button(
                crate::ui::icons::SAVE,
                "Save to the script library",
                Some(GraphMsg::SaveScript(id)),
            ));
        }
        if matches!(node.kind, Kind::Composite(_)) {
            header = header.push(icon_button(
                crate::ui::icons::MENU,
                "Edit composite",
                Some(GraphMsg::CompositeEdit(id)),
            ));
            header = header.push(icon_button(
                crate::ui::icons::SPLIT_H,
                "Expand into its nodes",
                Some(GraphMsg::ExpandComposite(id)),
            ));
            header = header.push(icon_button(
                crate::ui::icons::SAVE,
                "Save to the composite library",
                Some(GraphMsg::SaveComposite(id)),
            ));
        }
        header = header.push(icon_button(
            crate::ui::icons::COPY,
            "Duplicate node",
            Some(GraphMsg::Duplicate(id)),
        ));
        header = header.push(icon_button(
            crate::ui::icons::CLOSE,
            "Delete node",
            Some(GraphMsg::NodeDelete(id)),
        ));
        // The header is the handle a card is dragged by.
        mouse_area(
            container(header)
                .padding([5, 6])
                .width(Length::Fill)
                .style(header_style),
        )
        .on_press(msg(GraphMsg::DragStart(id)))
        .interaction(if self.dragging == Some(id) {
            mouse::Interaction::Grabbing
        } else {
            mouse::Interaction::Grab
        })
        .into()
    }

    /// The group list at the foot of a card.
    fn group_picker(&self, id: NodeId) -> Element<'_, Message> {
        let input = self
            .engine
            .node(id)
            .is_some_and(|node| is_input(&node.kind));
        let mut options = vec![GroupChoice::None];
        options.extend(
            self.groups
                .iter()
                .filter(|group| group.inputs == input)
                .map(|group| GroupChoice::Join(group.id, group.name.clone())),
        );
        options.push(GroupChoice::New);
        let selected = match self.node_group(id).and_then(|group| self.group(group)) {
            Some(group) => GroupChoice::Join(group.id, group.name.clone()),
            None => GroupChoice::None,
        };
        pick_list(Some(selected), options, GroupChoice::to_string)
            .on_select(move |choice| {
                msg(match choice {
                    GroupChoice::None => GraphMsg::GroupAssign(id, None),
                    GroupChoice::Join(group, _) => GraphMsg::GroupAssign(id, Some(group)),
                    GroupChoice::New => GraphMsg::GroupNew(id),
                })
            })
            .text_size(10)
            .padding([1, 6])
            .into()
    }

    /// A card's frame, edged in its node's colour.
    fn card<'a>(
        id: NodeId,
        warn: bool,
        content: impl Into<Element<'a, Message>>,
    ) -> container::Container<'a, Message> {
        // A node with issues is edged in amber instead of its colour.
        let color = if warn { ISSUE_COLOR } else { node_color(id) };
        container(content).style(move |theme: &Theme| {
            let mut style = card_style(theme);
            style.border.color = color.scale_alpha(0.8);
            style.border.width = 1.5;
            style
        })
    }

    /// An Inputs-area card: the node's values, all typed by hand.
    fn input_card(&self, node: &graph::Node, sections: Vec<NodeSection>) -> Element<'_, Message> {
        let id = node.id;
        let uses = self.dependents(id).len();
        let spec_name = match &node.kind {
            Kind::Op(spec) => spec.name,
            _ => "",
        };
        let slider = matches!(spec_name, "Number Slider" | "Integer Slider");
        let range_open = self
            .ui
            .get(&id)
            .is_some_and(|ui| ui.open.contains(RANGE_SECTION));
        let mut extra = Vec::new();
        if slider {
            extra.push(icon_button(
                crate::ui::icons::MORE,
                if range_open {
                    "Hide the range"
                } else {
                    "Edit the range: minimum, maximum and step"
                },
                Some(GraphMsg::Section(id, RANGE_SECTION.to_owned())),
            ));
        }
        if slider || spec_name == "Number" {
            extra.push(icon_button(
                crate::ui::icons::SWAP,
                if slider {
                    "Turn into a typed number"
                } else {
                    "Turn into a slider"
                },
                Some(GraphMsg::ToggleSlider(id)),
            ));
        }
        extra.push(icon_button(
            crate::ui::icons::TRI_UP,
            "Move up",
            self.can_move(id, -1).then_some(GraphMsg::Move(id, -1)),
        ));
        extra.push(icon_button(
            crate::ui::icons::TRI_DOWN,
            "Move down",
            self.can_move(id, 1).then_some(GraphMsg::Move(id, 1)),
        ));
        let mut body = column![].spacing(4).padding([6, 8]);
        let mut range = row![].spacing(4).align_y(iced::Center);
        for node_row in sections.into_iter().flat_map(|section| section.rows) {
            if !node_row.input {
                continue;
            }
            // A slider's range rows share one line, shown on request.
            if slider && matches!(node_row.field.as_str(), "min" | "max" | "step") {
                let port = Port {
                    node: id,
                    field: node_row.field.clone(),
                };
                range = range.push(
                    text(t!(node_row.label.as_str()).into_owned())
                        .size(10)
                        .style(weak_text),
                );
                range = range.push(self.field_input(port, &node_row.value));
                continue;
            }
            body = body.push(self.input_row(id, node_row));
        }
        if slider && range_open {
            body = body.push(range);
        }
        body = body.push(
            row![
                text(if uses == 0 {
                    t!("Not used yet").into_owned()
                } else {
                    crate::tf!("Used by {} inputs", uses).into_owned()
                })
                .size(10)
                .style(weak_text)
                .width(Length::Fill),
                icon_button(
                    crate::ui::icons::COPY,
                    "Copy node, to paste here or in another drawing",
                    Some(GraphMsg::CopyNodes(vec![id])),
                ),
                self.group_picker(id),
            ]
            .spacing(4)
            .align_y(iced::Center),
        );
        self.hover_target(
            id,
            Self::card(id, false, column![self.header(node, None, extra), body])
                .width(Length::Fill)
                .into(),
        )
    }

    /// One hand-typed value of an input node.
    fn input_row(&self, id: NodeId, node_row: NodeRow) -> Element<'_, Message> {
        let port = Port {
            node: id,
            field: node_row.field,
        };
        let shown = display(&node_row.value);
        let value: Element<'_, Message> = match node_row.widget {
            RowWidget::Slider { min, max, step } => {
                let current = graph::value::num(&node_row.value).unwrap_or(min);
                let set = port.clone();
                let mut bar = slider(min.min(max)..=max.max(min), current, move |x| {
                    msg(GraphMsg::Set(set.clone(), graph::value::number(x)))
                });
                if step > 0.0 {
                    bar = bar.step(step);
                }
                row![bar.width(Length::Fill), text(shown).size(11)]
                    .spacing(6)
                    .align_y(iced::Center)
                    .width(Length::Fill)
                    .into()
            }
            RowWidget::Toggle => {
                let set = port.clone();
                checkbox(graph::value::boolean(&node_row.value).unwrap_or(false))
                    .on_toggle(move |on| msg(GraphMsg::Set(set.clone(), Value::Bool(on))))
                    .size(14)
                    .width(Length::Fill)
                    .into()
            }
            RowWidget::Field => self.field_input(port, &node_row.value),
            RowWidget::Table => return table_editor(port, &node_row.value),
        };
        row![
            text(t!(node_row.label.as_str()).into_owned())
                .size(11)
                .width(64),
            value
        ]
        .spacing(4)
        .align_y(iced::Center)
        .into()
    }

    /// A typed value: its draft while edited, written on Enter.
    fn field_input(&self, port: Port, value: &Value) -> Element<'_, Message> {
        let draft = self
            .drafts
            .get(&port)
            .cloned()
            .unwrap_or_else(|| display(value));
        let edit = port.clone();
        text_input("", &draft)
            .on_input(move |s| msg(GraphMsg::Input(edit.clone(), s)))
            .on_submit(msg(GraphMsg::Commit(port)))
            .size(11)
            .padding([3, 5])
            .width(Length::Fill)
            .into()
    }

    /// A step of the node sequence: its settable rows, every one a picker button.
    /// Computed rows (lengths, angles, deltas, results…) are left out; the
    /// picker still offers them.
    fn step_card(
        &self,
        step: usize,
        steps: usize,
        node: &graph::Node,
        sections: Vec<NodeSection>,
        sources: &HashMap<Port, (NodeId, String)>,
    ) -> Element<'_, Message> {
        let id = node.id;
        let index = step - 1;
        let extra = vec![
            icon_button(
                crate::ui::icons::TRI_LEFT,
                "Move earlier",
                (index > 0).then_some(GraphMsg::Move(id, -1)),
            ),
            icon_button(
                crate::ui::icons::TRI_RIGHT,
                "Move later",
                (index + 1 < steps).then_some(GraphMsg::Move(id, 1)),
            ),
        ];
        let mut body = column![].spacing(2).padding([4, 6]);
        if !node.issues.is_empty() {
            let mut issues = column![].spacing(2);
            for issue in &node.issues {
                issues = issues.push(
                    row![
                        crate::ui::icons::themed(crate::ui::icons::WARNING, 10.0),
                        text(issue_text(issue)).size(10),
                    ]
                    .spacing(4)
                    .align_y(iced::Center),
                );
            }
            body = body.push(container(issues).padding([3, 6]).width(Length::Fill).style(
                |_: &Theme| container::Style {
                    background: Some(Background::Color(ISSUE_COLOR.scale_alpha(0.15))),
                    border: Border {
                        color: ISSUE_COLOR,
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                },
            ));
        }
        if sections.is_empty() {
            body = body.push(
                container(text(t!("Object deleted")).size(11).style(weak_text)).padding([2, 2]),
            );
        }
        if self.compact {
            return self.compact_card(node, step, body, sections, sources);
        }
        let mut computed: Vec<NodeRow> = Vec::new();
        for section in sections {
            let open = self.is_open(id, &section);
            let (rows, rest): (Vec<NodeRow>, Vec<NodeRow>) =
                section.rows.into_iter().partition(|row| row.input);
            // The object handle is the node itself; the picker offers it.
            computed.extend(rest.into_iter().filter(|row| row.field != "object"));
            if rows.is_empty() {
                continue;
            }
            if !section.title.is_empty() {
                body = body.push(
                    button(
                        row![
                            crate::ui::icons::themed_arrow_toggle(open, 9.0),
                            text(section.title.clone()).size(11)
                        ]
                        .spacing(4)
                        .align_y(iced::Center),
                    )
                    .on_press(msg(GraphMsg::Section(id, section.title)))
                    .style(button::secondary)
                    .width(Length::Fill)
                    .padding([3, 6]),
                );
            }
            if open {
                for node_row in rows {
                    body = body.push(self.step_row(id, node_row, sources));
                }
            }
        }
        // Computed rows (lengths, angles, deltas, results…) are read-only:
        // a collapsed group of their own, closed until opened.
        if !computed.is_empty() {
            let open = self
                .ui
                .get(&id)
                .is_some_and(|ui| ui.open.contains(COMPUTED_SECTION));
            body = body.push(
                button(
                    row![
                        crate::ui::icons::themed_arrow_toggle(open, 9.0),
                        text(t!("Computed values")).size(11).width(Length::Fill),
                        text(computed.len().to_string()).size(10).style(weak_text),
                    ]
                    .spacing(4)
                    .align_y(iced::Center),
                )
                .on_press(msg(GraphMsg::Section(id, COMPUTED_SECTION.to_owned())))
                .style(button::secondary)
                .width(Length::Fill)
                .padding([3, 6]),
            );
            if open {
                for node_row in computed {
                    body = body.push(
                        row![
                            column![
                                text(t!(node_row.label.as_str()).into_owned()).size(11),
                                type_chip(shown_type(node_row.kind, &node_row.value)),
                            ]
                            .spacing(2)
                            .width(LABEL_W),
                            with_list_preview(
                                text(display(&node_row.value))
                                    .size(11)
                                    .style(weak_text)
                                    .width(Length::Fill)
                                    .into(),
                                list_preview(&node_row.value),
                            ),
                        ]
                        .spacing(4)
                        .padding([3, 4])
                        .align_y(iced::Center),
                    );
                }
            }
        }
        body = body.push(
            row![
                icon_button(
                    crate::ui::icons::COPY,
                    "Copy node, to paste here or in another drawing",
                    Some(GraphMsg::CopyNodes(vec![id])),
                ),
                Space::new().width(Length::Fill),
                self.group_picker(id)
            ]
            .spacing(4)
            .padding([4, 2])
            .align_y(iced::Center),
        );
        self.hover_target(
            id,
            Self::card(
                id,
                !node.issues.is_empty(),
                column![self.header(node, Some(step), extra), body],
            )
            .width(NODE_W)
            .into(),
        )
    }

    /// A step in the compact view: its header, issues, how many of its
    /// inputs are linked and its first results.
    fn compact_card<'a>(
        &'a self,
        node: &graph::Node,
        step: usize,
        mut body: iced::widget::Column<'a, Message>,
        sections: Vec<NodeSection>,
        sources: &HashMap<Port, (NodeId, String)>,
    ) -> Element<'a, Message> {
        let id = node.id;
        let rows: Vec<NodeRow> = sections
            .into_iter()
            .flat_map(|section| section.rows)
            .collect();
        let inputs = rows.iter().filter(|row| row.input).count();
        let linked = rows
            .iter()
            .filter(|row| {
                row.input
                    && sources.contains_key(&Port {
                        node: id,
                        field: row.field.clone(),
                    })
            })
            .count();
        if inputs > 0 {
            body = body.push(
                text(crate::tf!("{} of {} values linked", linked, inputs).into_owned())
                    .size(10)
                    .style(weak_text),
            );
        }
        for node_row in rows
            .into_iter()
            .filter(|row| !row.input && row.field != "object")
            .take(3)
        {
            body = body.push(
                row![
                    text(t!(node_row.label.as_str()).into_owned())
                        .size(10)
                        .width(70),
                    with_list_preview(
                        text(display(&node_row.value))
                            .size(10)
                            .style(weak_text)
                            .width(Length::Fill)
                            .into(),
                        list_preview(&node_row.value),
                    ),
                ]
                .spacing(4)
                .align_y(iced::Center),
            );
        }
        self.hover_target(
            id,
            Self::card(
                id,
                !node.issues.is_empty(),
                column![self.header(node, Some(step), Vec::new()), body],
            )
            .width(COMPACT_W)
            .into(),
        )
    }

    /// The nodes of group `id`, in node order.
    fn group_members(&self, id: GroupId) -> Vec<NodeId> {
        self.engine
            .nodes
            .iter()
            .filter(|node| self.node_group(node.id) == Some(id))
            .map(|node| node.id)
            .collect()
    }

    fn step_row(
        &self,
        id: NodeId,
        node_row: NodeRow,
        sources: &HashMap<Port, (NodeId, String)>,
    ) -> Element<'_, Message> {
        let port = Port {
            node: id,
            field: node_row.field,
        };
        let shown = display(&node_row.value);
        let label = column![
            text(t!(node_row.label.as_str()).into_owned()).size(11),
            type_chip(shown_type(node_row.kind, &node_row.value)),
        ]
        .spacing(2)
        .width(LABEL_W);
        let source = sources.get(&port).cloned();
        let picking = self.picking.as_ref() == Some(&port);
        let linked = source.is_some();
        let content: Element<'_, Message> = match source {
            Some((from, source)) => row![
                badge(node_color(from), None),
                column![
                    text(source).size(11),
                    with_list_preview(
                        text(format!("= {shown}")).size(10).style(weak_text).into(),
                        list_preview(&node_row.value),
                    ),
                ]
                .spacing(1)
            ]
            .spacing(6)
            .align_y(iced::Center)
            .into(),
            None => row![
                text(if shown.is_empty() {
                    "—".to_owned()
                } else {
                    shown
                })
                .size(11)
                .style(weak_text)
                .width(Length::Fill),
                crate::ui::icons::themed(crate::ui::icons::EYEDROPPER, 10.0),
            ]
            .align_y(iced::Center)
            .into(),
        };
        let selector = button(content)
            .on_press(msg(GraphMsg::Pick(port.clone())))
            .width(Length::Fill)
            .padding([3, 6])
            .style(move |theme: &Theme, status| {
                let palette = theme.palette();
                let accent = picking || linked;
                let mut style = button::Style {
                    background: Some(Background::Color(if picking {
                        palette.primary.weak.color
                    } else {
                        palette.background.weak.color
                    })),
                    text_color: palette.background.base.text,
                    border: Border {
                        color: if accent {
                            palette.primary.base.color
                        } else {
                            palette.background.strong.color
                        },
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                };
                if matches!(status, button::Status::Hovered) {
                    style.border.color = palette.primary.base.color;
                }
                style
            });
        let mut line = row![label, selector].spacing(4).align_y(iced::Center);
        if linked {
            line = line.push(icon_button(
                crate::ui::icons::CLOSE,
                "Clear value",
                Some(GraphMsg::Unlink(port)),
            ));
        }
        container(line).padding([2, 2]).into()
    }

    /// Values `target` may take: every input, then every output of the
    /// steps before it — computed ones included — one coloured group per
    /// node. Values of the wrong kind are dimmed.
    fn picker(&self, target: &Port, sections: &[Vec<NodeSection>]) -> Element<'_, Message> {
        let steps = self.steps();
        let before = steps.get(&target.node).copied().unwrap_or(0);
        let current = self
            .engine
            .links
            .iter()
            .find(|link| &link.to == target)
            .map(|link| link.from.clone());
        let target_value = self
            .port_row(sections, target, false)
            .map(|row| row.value.clone())
            .unwrap_or(Value::Null);
        let expect = self.expect(target, &target_value);
        let query = self.pick_search.trim().to_lowercase();

        struct Entry {
            port: Port,
            label: String,
            value: String,
            computed: bool,
            fits: bool,
            kind: PortType,
            tip: Option<String>,
        }
        struct Group {
            id: NodeId,
            name: String,
            step: Option<usize>,
            kind: String,
            tag: String,
            entries: Vec<Entry>,
        }
        let mut groups: Vec<Group> = Vec::new();
        let candidates = self
            .engine
            .nodes
            .iter()
            .zip(sections)
            .filter(|(node, _)| is_input(&node.kind))
            .chain(
                self.engine
                    .nodes
                    .iter()
                    .zip(sections)
                    .filter(|(node, _)| steps.get(&node.id).is_some_and(|step| *step < before)),
            );
        for (node, node_sections) in candidates {
            let name = self.name(node.id);
            let name_match = name.to_lowercase().contains(&query);
            let entries: Vec<Entry> = node_sections
                .iter()
                .flat_map(|section| &section.rows)
                .filter(|row| row.output)
                .map(|row| Entry {
                    port: Port {
                        node: node.id,
                        field: row.field.clone(),
                    },
                    label: t!(row.label.as_str()).into_owned(),
                    value: display(&row.value),
                    computed: !row.input,
                    fits: expect.fits(&row.value),
                    kind: shown_type(row.kind, &row.value),
                    tip: list_preview(&row.value),
                })
                .filter(|entry| {
                    query.is_empty() || name_match || entry.label.to_lowercase().contains(&query)
                })
                .filter(|entry| !self.pick_compatible || entry.fits)
                .collect();
            if !entries.is_empty() {
                groups.push(Group {
                    id: node.id,
                    name,
                    step: steps.get(&node.id).map(|step| step + 1),
                    kind: kind_name(&node.kind),
                    tag: kind_tag(&node.kind),
                    entries,
                });
            }
        }

        // Inputs on the left, earlier steps on the right, each scrolling on
        // its own.
        let mut input_list = column![].spacing(8);
        let mut step_list = column![].spacing(8);
        let (mut input_count, mut step_count) = (0, 0);
        for group in groups {
            let is_step = group.step.is_some();
            let color = node_color(group.id);
            let tag =
                container(text(group.tag).size(10))
                    .padding([1, 6])
                    .style(move |_: &Theme| container::Style {
                        background: Some(Background::Color(color.scale_alpha(0.25))),
                        border: Border {
                            color,
                            width: 1.0,
                            radius: 8.0.into(),
                        },
                        ..Default::default()
                    });
            let mut entries = column![row![
                badge(color, group.step.map(|step| step.to_string())),
                column![
                    text(group.name)
                        .size(12)
                        .style(move |_: &Theme| text::Style { color: Some(color) }),
                    text(group.kind).size(10).style(weak_text),
                ]
                .spacing(1)
                .width(Length::Fill),
                tag,
            ]
            .spacing(8)
            .align_y(iced::Center)
            .padding([2, 4])]
            .spacing(1);
            for entry in group.entries {
                let chosen = current.as_ref() == Some(&entry.port);
                let fits = entry.fits;
                let mut label_row = row![text(entry.label).size(11), type_chip(entry.kind)]
                    .spacing(4)
                    .align_y(iced::Center);
                if entry.computed {
                    label_row = label_row.push(text(t!("computed")).size(9).style(weak_text));
                }
                if !fits {
                    label_row = label_row.push(text(t!("other type")).size(9).style(weak_text));
                }
                entries = entries.push(
                    button(
                        row![
                            container(label_row).width(Length::FillPortion(2)),
                            with_list_preview(
                                text(entry.value)
                                    .size(11)
                                    .style(weak_text)
                                    .width(Length::FillPortion(3))
                                    .into(),
                                entry.tip,
                            ),
                        ]
                        .spacing(6)
                        .align_y(iced::Center),
                    )
                    .on_press(msg(GraphMsg::Choose(entry.port)))
                    .width(Length::Fill)
                    .padding([3, 10])
                    .style(move |theme: &Theme, status| {
                        if chosen {
                            return button::primary(theme, status);
                        }
                        let mut style = button::text(theme, status);
                        if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                            style.background = Some(Background::Color(color.scale_alpha(0.25)));
                        }
                        if !fits {
                            style.text_color = style.text_color.scale_alpha(0.45);
                        }
                        style.border.radius = 4.0.into();
                        style
                    }),
                );
            }
            let card = container(entries)
                .padding(4)
                .width(Length::Fill)
                .style(move |_: &Theme| container::Style {
                    background: Some(Background::Color(color.scale_alpha(0.08))),
                    border: Border {
                        color: color.scale_alpha(0.6),
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    ..Default::default()
                });
            if is_step {
                step_list = step_list.push(card);
                step_count += 1;
            } else {
                input_list = input_list.push(card);
                input_count += 1;
            }
        }
        let pane = |title: String,
                    count: usize,
                    list: iced::widget::Column<'static, Message>,
                    empty: &str| {
            let list = if count == 0 {
                list.push(text(t!(empty).into_owned()).size(11).style(weak_text))
            } else {
                list
            };
            column![
                row![
                    text(title).size(12).width(Length::Fill),
                    text(count.to_string()).size(10).style(weak_text),
                ]
                .align_y(iced::Center)
                .padding([0, 4]),
                scrollable(container(list).padding(iced::Padding {
                    top: 0.0,
                    right: 10.0,
                    bottom: 0.0,
                    left: 0.0,
                }))
                .height(Length::Fixed(380.0)),
            ]
            .spacing(6)
            .width(Length::FillPortion(1))
        };
        let list = row![
            pane(
                t!("Inputs").into_owned(),
                input_count,
                input_list,
                "No inputs yet. New input with this value adds one."
            ),
            container(Space::new().width(1.0).height(Length::Fixed(400.0))).style(
                |theme: &Theme| container::Style {
                    background: Some(Background::Color(theme.palette().background.strong.color)),
                    ..Default::default()
                }
            ),
            pane(
                t!("Node Sequence").into_owned(),
                step_count,
                step_list,
                "No earlier steps. Only steps before this one can feed it."
            ),
        ]
        .spacing(10);

        let mut title = column![
            text(t!("Select value")).size(13),
            text(format!(
                "{} › {}",
                self.name(target.node),
                self.port_label(sections, target, false)
            ))
            .size(11)
            .style(weak_text),
        ]
        .spacing(2)
        .width(Length::Fill);
        if let Some(label) = expect.label() {
            title = title.push(text(t!(label)).size(10).style(weak_text));
        }
        let mut actions = row![].spacing(6).align_y(iced::Center);
        if let Some(kind) = expect.input_kind() {
            let value = match kind {
                "Number" => graph::value::num(&target_value)
                    .map_or(graph::value::number(0.0), graph::value::number),
                "Text" => Value::String(graph::value::text(&target_value)),
                _ => Value::Bool(graph::value::boolean(&target_value).unwrap_or(false)),
            };
            actions = actions.push(
                button(
                    row![
                        crate::ui::icons::themed(crate::ui::icons::PLUS, 10.0),
                        text(t!("New input with this value")).size(11)
                    ]
                    .spacing(4)
                    .align_y(iced::Center),
                )
                .on_press(msg(GraphMsg::PickNewInput(kind, value)))
                .style(button::primary)
                .padding([4, 10]),
            );
        }
        actions = actions.push(Space::new().width(Length::Fill));
        if current.is_some() {
            actions = actions.push(
                button(text(t!("Clear value")).size(11))
                    .on_press(msg(GraphMsg::Unlink(target.clone())))
                    .style(button::secondary)
                    .padding([4, 10]),
            );
        }
        actions = actions.push(
            button(text(t!("Cancel")).size(11))
                .on_press(msg(GraphMsg::PickCancel))
                .style(button::secondary)
                .padding([4, 10]),
        );
        let card = container(
            column![
                row![
                    badge(node_color(target.node), None),
                    title,
                    icon_button(
                        crate::ui::icons::CLOSE,
                        "Cancel",
                        Some(GraphMsg::PickCancel)
                    )
                ]
                .spacing(8)
                .align_y(iced::Center),
                row![
                    text_input(&t!("Search values…"), &self.pick_search)
                        .id(iced::widget::Id::new(PICK_SEARCH_ID))
                        .on_input(|value| msg(GraphMsg::PickSearch(value)))
                        .padding([4, 8])
                        .size(12),
                    checkbox(self.pick_compatible)
                        .label(t!("Only values that fit").into_owned())
                        .on_toggle(|on| msg(GraphMsg::PickCompatible(on)))
                        .size(14)
                        .text_size(11),
                ]
                .spacing(10)
                .align_y(iced::Center),
                list,
                actions,
            ]
            .spacing(8),
        )
        .padding(12)
        .width(PICKER_TWO_W)
        .style(card_style);
        modal(card.into(), GraphMsg::PickCancel)
    }

    /// The composite editor: the composite's name and description, and a
    /// name and default per input and a name per output.
    fn composite_editor(&self, draft: &CompositeDraft) -> Element<'_, Message> {
        let field = |value: &str, on: Box<dyn Fn(String) -> GraphMsg>| {
            text_input("", value)
                .on_input(move |text| msg(on(text)))
                .size(11)
                .padding([3, 6])
                .width(Length::Fill)
        };
        let mut inputs = column![row![
            text(t!("Input"))
                .size(10)
                .style(weak_text)
                .width(Length::FillPortion(3)),
            text(t!("Default"))
                .size(10)
                .style(weak_text)
                .width(Length::FillPortion(2)),
        ]
        .spacing(6)]
        .spacing(4);
        for (k, (_, name, default)) in draft.inputs.iter().enumerate() {
            inputs = inputs.push(
                row![
                    container(field(
                        name,
                        Box::new(move |text| GraphMsg::CompositeDraftInput(k, text))
                    ))
                    .width(Length::FillPortion(3)),
                    container(field(
                        default,
                        Box::new(move |text| GraphMsg::CompositeDraftDefault(k, text))
                    ))
                    .width(Length::FillPortion(2)),
                ]
                .spacing(6),
            );
        }
        let mut outputs = column![text(t!("Output")).size(10).style(weak_text)].spacing(4);
        for (k, (_, name)) in draft.outputs.iter().enumerate() {
            outputs = outputs.push(field(
                name,
                Box::new(move |text| GraphMsg::CompositeDraftOutput(k, text)),
            ));
        }
        let card = container(
            column![
                row![
                    crate::ui::icons::themed(crate::ui::icons::MENU, 16.0),
                    text(t!("Edit composite")).size(13)
                ]
                .spacing(8)
                .align_y(iced::Center),
                text(t!("Name")).size(10).style(weak_text),
                field(&draft.name, Box::new(GraphMsg::CompositeDraftName)),
                text(t!("Description")).size(10).style(weak_text),
                field(&draft.description, Box::new(GraphMsg::CompositeDraftDescription)),
                scrollable(column![inputs, outputs].spacing(12)).height(Length::Shrink),
                text(t!(
                    "Links follow renamed ports. Save the composite to the library to keep the changes for other drawings."
                ))
                .size(10)
                .style(weak_text),
                row![
                    Space::new().width(Length::Fill),
                    button(text(t!("Cancel")).size(11))
                        .on_press(msg(GraphMsg::CompositeEditCancel))
                        .style(button::secondary)
                        .padding([4, 10]),
                    button(text(t!("Apply")).size(11))
                        .on_press(msg(GraphMsg::CompositeEditSave))
                        .style(button::primary)
                        .padding([4, 10]),
                ]
                .spacing(6),
            ]
            .spacing(8),
        )
        .padding(14)
        .width(PICKER_W)
        .style(card_style);
        modal(card.into(), GraphMsg::CompositeEditCancel)
    }

    /// The script editor: the program with its line numbers, where the
    /// cursor is, the last check, a short reference of what scripts can call,
    /// and Check, Cancel and Apply.
    fn script_editor<'a>(&'a self, draft: &'a ScriptDraft) -> Element<'a, Message> {
        const SIZE: f32 = 12.0;
        let lines = draft.content.line_count().max(1);
        let numbers = (1..=lines)
            .map(|k| k.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let gutter = container(
            text(numbers)
                .size(SIZE)
                .font(iced::Font::MONOSPACE)
                .align_x(iced::Right)
                .style(weak_text),
        )
        .padding(iced::Padding {
            top: 5.0,
            right: 6.0,
            bottom: 5.0,
            left: 4.0,
        });
        let editor = iced::widget::text_editor(&draft.content)
            .on_action(|action| msg(GraphMsg::ScriptAction(action)))
            .font(iced::Font::MONOSPACE)
            .size(SIZE)
            .wrapping(iced::widget::text::Wrapping::None)
            .height(Length::Shrink);
        let code = scrollable(row![gutter, editor].align_y(iced::Top))
            .height(Length::Fixed(340.0))
            .width(Length::Fill);
        let cursor = draft.content.cursor().position;
        let status =
            text(crate::tf!("Line {}, column {}", cursor.line + 1, cursor.column + 1).into_owned())
                .size(10)
                .style(weak_text);
        let check: Element<'_, Message> = match &draft.checked {
            Some(Ok(ports)) => text(ports.clone()).size(11).into(),
            Some(Err(why)) => row![
                crate::ui::icons::themed(crate::ui::icons::WARNING, 11.0),
                text(why.clone()).size(11).style(|_: &Theme| text::Style {
                    color: Some(ISSUE_COLOR)
                })
            ]
            .spacing(4)
            .align_y(iced::Center)
            .into(),
            None => text(t!("Press Check to read the program"))
                .size(11)
                .style(weak_text)
                .into(),
        };
        let reference = text(t!(
            "Declare the ports in the first lines: // inputs: name: kind = default, … and // outputs: name: kind, … (kinds: number, text, bool, point, vector, curve, plane, geometry, any; [] for a whole list). End with #{ output: value, … }. Points are #{x, y, z}. Functions: point(x, y), line(a, b), polyline(points), circle(center, r), length(curve), point_at(curve, d), angle_at(curve, d), offset(curve, d), distance(a, b), sind, cosd, tand, atan2d (degrees), and op(\"Node name\", #{ input: value }) for any computing node."
        ))
        .size(10)
        .style(weak_text);
        let card = container(
            column![
                row![
                    crate::ui::icons::themed(crate::ui::icons::MENU, 16.0),
                    text(t!("Edit script")).size(13)
                ]
                .spacing(8)
                .align_y(iced::Center),
                text(t!("Name")).size(10).style(weak_text),
                text_input("", &draft.name)
                    .on_input(|name| msg(GraphMsg::ScriptName(name)))
                    .size(11)
                    .padding([3, 6])
                    .width(Length::Fill),
                container(code).style(container::bordered_box),
                row![status, Space::new().width(Length::Fill)],
                check,
                reference,
                row![
                    button(text(t!("Check")).size(11))
                        .on_press(msg(GraphMsg::ScriptCheck))
                        .style(button::secondary)
                        .padding([4, 10]),
                    Space::new().width(Length::Fill),
                    button(text(t!("Cancel")).size(11))
                        .on_press(msg(GraphMsg::ScriptCancel))
                        .style(button::secondary)
                        .padding([4, 10]),
                    button(text(t!("Apply")).size(11))
                        .on_press(msg(GraphMsg::ScriptSave))
                        .style(button::primary)
                        .padding([4, 10]),
                ]
                .spacing(6),
            ]
            .spacing(8),
        )
        .padding(14)
        .width(PICKER_TWO_W)
        .style(card_style);
        // A click beside the editor checks rather than losing the program.
        modal(card.into(), GraphMsg::ScriptCheck)
    }

    /// Asks before an edit drops links, naming each one.
    fn confirm_dialog(
        &self,
        confirm: &Confirm,
        sections: &[Vec<NodeSection>],
    ) -> Element<'_, Message> {
        let name = self.name(confirm.node);
        let (title, lead, action) = if confirm.delete {
            (
                t!("Delete node?").into_owned(),
                crate::tf!("Other nodes read from {}. Deleting it clears:", name).into_owned(),
                t!("Delete").into_owned(),
            )
        } else {
            (
                t!("Move node?").into_owned(),
                crate::tf!("Values only flow forward. Moving {} there clears:", name).into_owned(),
                t!("Move").into_owned(),
            )
        };
        let mut links = column![].spacing(3);
        for link in &confirm.links {
            links = links.push(
                row![
                    badge(node_color(link.to.node), None),
                    text(format!(
                        "{} › {}",
                        self.name(link.to.node),
                        self.port_label(sections, &link.to, false)
                    ))
                    .size(11),
                    text(format!("← {}", self.source_name(sections, &link.from)))
                        .size(11)
                        .style(weak_text),
                ]
                .spacing(6)
                .align_y(iced::Center),
            );
        }
        let card = container(
            column![
                row![
                    crate::ui::icons::themed(crate::ui::icons::WARNING, 16.0),
                    text(title).size(13)
                ]
                .spacing(8)
                .align_y(iced::Center),
                text(lead).size(11),
                scrollable(links).height(Length::Shrink),
                text(t!(
                    "Those inputs fall back to their own values. Undo restores them."
                ))
                .size(10)
                .style(weak_text),
                row![
                    Space::new().width(Length::Fill),
                    button(text(t!("Cancel")).size(11))
                        .on_press(msg(GraphMsg::ConfirmNo))
                        .style(button::secondary)
                        .padding([4, 10]),
                    button(text(action).size(11))
                        .on_press(msg(GraphMsg::ConfirmYes))
                        .style(button::danger)
                        .padding([4, 10]),
                ]
                .spacing(6),
            ]
            .spacing(10),
        )
        .padding(14)
        .width(PICKER_W)
        .style(card_style);
        modal(card.into(), GraphMsg::ConfirmNo)
    }

    fn palette_entry(&self, item: PaletteItem) -> Element<'_, Message> {
        let icon: Element<'_, Message> = match item {
            PaletteItem::Object(kind) => crate::modules::registry::command_icon(kind.command())
                .map_or_else(
                    || Space::new().width(14.0).into(),
                    |bytes| crate::ui::icons::semantic(bytes, 14.0),
                ),
            PaletteItem::Op(_)
            | PaletteItem::Composite(_)
            | PaletteItem::NewScript
            | PaletteItem::Script(_) => Space::new().width(14.0).into(),
        };
        let entry = button(
            row![icon, text(item.title()).size(12)]
                .spacing(6)
                .align_y(iced::Center),
        )
        .on_press(msg(GraphMsg::Add(item)))
        .style(button::text)
        .padding([3, 10])
        .width(Length::Fill);
        let mut about = item.kind().map(|kind| describe(&kind)).unwrap_or_default();
        if let PaletteItem::Op(spec) = item {
            if let Some(signature) = signature(spec) {
                about = format!("{about}\n{signature}");
            }
        }
        let entry: Element<'_, Message> = if about.is_empty() {
            entry.into()
        } else {
            tooltip(entry, tip_box(about), tooltip::Position::Bottom)
                .gap(2)
                .into()
        };
        match item {
            PaletteItem::Composite(k) => row![
                entry,
                icon_button(
                    crate::ui::icons::TRASH,
                    "Remove from library",
                    Some(GraphMsg::ForgetComposite(k))
                )
            ]
            .align_y(iced::Center)
            .into(),
            PaletteItem::Script(k) => row![
                entry,
                icon_button(
                    crate::ui::icons::TRASH,
                    "Remove from library",
                    Some(GraphMsg::ForgetScript(k))
                )
            ]
            .align_y(iced::Center)
            .into(),
            _ => entry.into(),
        }
    }

    /// The node library as a docked panel: title bar, a search box with the
    /// graph's New / Open / Save beside it, then the categories. Clicking an
    /// entry adds it: inputs to the Inputs area, the rest to the node sequence.
    pub fn panel(&self, width: f32, chrome: crate::ui::dock::Chrome) -> Element<'_, Message> {
        use crate::ui::dock::{self, PanelId};
        let tool = |icon: &'static [u8], tip: &str, message: GraphMsg| {
            dock::tool_button(
                crate::ui::icons::themed(icon, dock::TOOL_H - 6.0),
                t!(tip).into_owned(),
                msg(message),
            )
        };
        let toolbar = row![
            text_input(&t!("Search nodes…"), &self.search)
                .on_input(|value| msg(GraphMsg::Search(value)))
                .padding([4, 8])
                .size(12),
            tool(crate::ui::icons::DOC_NEW, "New graph", GraphMsg::New),
            tool(crate::ui::icons::FOLDER_OPEN, "Open graph", GraphMsg::Open),
            tool(crate::ui::icons::SAVE, "Save graph", GraphMsg::Save),
            tool(
                crate::ui::icons::EYEDROPPER,
                "Nodes from the selected objects",
                GraphMsg::FromSelection
            ),
            if self.steps_hidden {
                tool(crate::ui::icons::EYE, "Show nodes", GraphMsg::ToggleSteps)
            } else {
                tool(
                    crate::ui::icons::EYE_OFF,
                    "Hide nodes",
                    GraphMsg::ToggleSteps,
                )
            },
        ]
        .spacing(4)
        .align_y(iced::Center);

        let query = self.search.trim().to_lowercase();
        let wanted =
            |item: PaletteItem| query.is_empty() || item.title().to_lowercase().contains(&query);
        let mut tree = column![].spacing(1);
        let categories = ["Objects", "Composites", "Scripts"]
            .into_iter()
            .chain(graph::CATEGORIES.iter().copied());
        for category in categories {
            let items: Vec<PaletteItem> = if category == "Objects" {
                OBJECTS
                    .iter()
                    .map(|kind| PaletteItem::Object(*kind))
                    .collect()
            } else if category == "Composites" {
                (0..composite_library().len())
                    .map(PaletteItem::Composite)
                    .collect()
            } else if category == "Scripts" {
                std::iter::once(PaletteItem::NewScript)
                    .chain((0..script_library().len()).map(PaletteItem::Script))
                    .collect()
            } else {
                graph::LIBRARY
                    .iter()
                    .filter(|spec| spec.category == category)
                    .map(PaletteItem::Op)
                    .collect()
            };
            let items: Vec<PaletteItem> = items.into_iter().filter(|item| wanted(*item)).collect();
            // Composites stays, empty, for its import button.
            let library = category == "Composites";
            if items.is_empty() && !(library && query.is_empty()) {
                continue;
            }
            // A search shows every match, collapsed categories included.
            let open = !query.is_empty()
                || (category == "Objects") != self.toggled_categories.contains(category);
            // Objects are the nodes that draw; the other categories only
            // compute values, so Objects stands out.
            let draws = category == "Objects";
            let mut heading = row![
                crate::ui::icons::themed_arrow_toggle(open, 9.0),
                text(t!(category)).size(11).width(Length::Fill)
            ]
            .spacing(4)
            .align_y(iced::Center);
            if draws {
                heading = heading.push(text(t!("Draws in the drawing")).size(9));
            }
            let heading = button(heading)
                .on_press(msg(GraphMsg::Category(category)))
                .style(move |theme: &Theme, status| {
                    let palette = theme.palette();
                    let mut style = button::text(theme, status);
                    if draws {
                        style.background = Some(Background::Color(palette.primary.weak.color));
                        style.text_color = palette.primary.weak.text;
                        style.border = Border {
                            color: palette.primary.base.color,
                            width: 1.0,
                            radius: 4.0.into(),
                        };
                    } else {
                        style.background = Some(Background::Color(palette.background.weak.color));
                    }
                    style
                })
                .width(Length::Fill)
                .padding([3, 6]);
            let heading: Element<'_, Message> = if library {
                row![
                    heading,
                    icon_button(
                        crate::ui::icons::FILE_EXPORT,
                        "Export the composite library",
                        (!composite_library().is_empty()).then_some(GraphMsg::LibraryExport)
                    ),
                    icon_button(
                        crate::ui::icons::FOLDER_OPEN,
                        "Import composites",
                        Some(GraphMsg::LibraryImport)
                    ),
                ]
                .align_y(iced::Center)
                .into()
            } else {
                heading.into()
            };
            tree = tree.push(if draws {
                Element::from(
                    tooltip(
                        heading,
                        tip_box(
                            t!("These nodes draw objects in the drawing and keep them up to date. The other categories only compute values.")
                                .into_owned(),
                        ),
                        tooltip::Position::Bottom,
                    )
                    .gap(2),
                )
            } else {
                heading.into()
            });
            if open {
                for item in items {
                    tree = tree.push(self.palette_entry(item));
                }
            }
        }
        let body = scrollable(container(tree).padding(iced::Padding {
            top: 0.0,
            right: 8.0,
            bottom: 6.0,
            left: 0.0,
        }))
        .width(Length::Fill)
        .height(Length::Fill);

        dock::frame(
            dock::title_bar(PanelId::NodeGraph, t!("Node Graph").into_owned(), chrome),
            column![toolbar, body].spacing(6),
            width,
        )
    }

    /// Ends a rename: an empty name keeps the old one, a node name that is
    /// taken gets a number.
    fn commit_rename(&mut self) {
        let Some((target, draft)) = self.renaming.take() else {
            return;
        };
        let wanted = draft.trim();
        if wanted.is_empty() {
            return;
        }
        match target {
            Renaming::Node(id) => {
                let name = self.unique_name(wanted, Some(id), false);
                if self.ui.get(&id).is_some_and(|ui| ui.name != name) {
                    self.record();
                    if let Some(ui) = self.ui.get_mut(&id) {
                        ui.name = name;
                    }
                }
            }
            Renaming::Group(id) => {
                if self.group(id).is_some_and(|group| group.name != wanted) {
                    self.record();
                    if let Some(group) = self.groups.iter_mut().find(|group| group.id == id) {
                        group.name = wanted.to_owned();
                    }
                }
            }
        }
    }

    /// Pure canvas state; document and evaluation work is the app's.
    pub fn update_ui(&mut self, message: &GraphMsg) {
        match message {
            GraphMsg::Category(category) => {
                if !self.toggled_categories.remove(category) {
                    self.toggled_categories.insert(category);
                }
            }
            GraphMsg::Search(value) => self.search = value.clone(),
            GraphMsg::NodeSearch(value) => {
                self.node_search = value.clone();
                self.found = None;
                self.find(0);
            }
            GraphMsg::NodeSearchStep(step) => self.find(*step),
            GraphMsg::ToggleCompact => self.compact = !self.compact,
            GraphMsg::ToggleSlider(id) => {
                self.record();
                self.toggle_slider(*id);
            }
            GraphMsg::Section(id, title) => {
                if let Some(ui) = self.ui.get_mut(id) {
                    if !ui.open.remove(title) {
                        ui.open.insert(title.clone());
                    }
                }
            }
            GraphMsg::RenameStart(id) => {
                self.commit_rename();
                self.renaming = Some((Renaming::Node(*id), self.name(*id)));
            }
            GraphMsg::GroupRenameStart(id) => {
                self.commit_rename();
                let name = self
                    .group(*id)
                    .map(|group| group.name.clone())
                    .unwrap_or_default();
                self.renaming = Some((Renaming::Group(*id), name));
            }
            GraphMsg::RenameInput(value) => {
                if let Some((_, draft)) = &mut self.renaming {
                    *draft = value.clone();
                }
            }
            GraphMsg::RenameCommit => self.commit_rename(),
            GraphMsg::Move(id, direction) => {
                let links = self.move_breaks(*id, *direction);
                if links.is_empty() {
                    self.record();
                    self.move_node(*id, *direction);
                } else {
                    self.confirm = Some(Confirm {
                        action: GraphMsg::MoveNow(*id, *direction),
                        node: *id,
                        delete: false,
                        links,
                    });
                }
            }
            GraphMsg::MoveNow(id, direction) => {
                self.record();
                self.move_node(*id, *direction);
            }
            GraphMsg::NodeDelete(id) => {
                let links = self.dependents(*id);
                if links.is_empty() {
                    self.record();
                    self.forget(*id);
                } else {
                    self.confirm = Some(Confirm {
                        action: GraphMsg::DeleteNow(*id),
                        node: *id,
                        delete: true,
                        links,
                    });
                }
            }
            GraphMsg::DeleteNow(id) => {
                self.record();
                self.forget(*id);
            }
            GraphMsg::ConfirmNo => self.confirm = None,
            GraphMsg::Duplicate(id) => {
                self.record();
                self.duplicated = self.duplicate(*id).map(|copy| (*id, copy));
            }
            GraphMsg::Pick(port) => {
                self.commit_rename();
                self.picking = Some(port.clone());
                self.pick_search.clear();
            }
            GraphMsg::PickSearch(value) => self.pick_search = value.clone(),
            GraphMsg::PickCompatible(on) => self.pick_compatible = *on,
            GraphMsg::Hover(id) => self.hovered = *id,
            GraphMsg::HoverEnd(id) => {
                if self.hovered == Some(*id) {
                    self.hovered = None;
                }
            }
            GraphMsg::DragCancel => self.dragging = None,
            GraphMsg::CompositeEdit(id) => self.editing = self.composite_draft(*id),
            GraphMsg::CompositeDraftName(name) => {
                if let Some(draft) = &mut self.editing {
                    draft.name = name.clone();
                }
            }
            GraphMsg::CompositeDraftDescription(text) => {
                if let Some(draft) = &mut self.editing {
                    draft.description = text.clone();
                }
            }
            GraphMsg::CompositeDraftInput(k, name) => {
                if let Some(entry) = self
                    .editing
                    .as_mut()
                    .and_then(|draft| draft.inputs.get_mut(*k))
                {
                    entry.1 = name.clone();
                }
            }
            GraphMsg::CompositeDraftDefault(k, value) => {
                if let Some(entry) = self
                    .editing
                    .as_mut()
                    .and_then(|draft| draft.inputs.get_mut(*k))
                {
                    entry.2 = value.clone();
                }
            }
            GraphMsg::CompositeDraftOutput(k, name) => {
                if let Some(entry) = self
                    .editing
                    .as_mut()
                    .and_then(|draft| draft.outputs.get_mut(*k))
                {
                    entry.1 = name.clone();
                }
            }
            GraphMsg::CompositeEditCancel => self.editing = None,
            GraphMsg::ScriptEdit(id) => self.scripting = self.script_draft(*id),
            GraphMsg::ScriptName(name) => {
                if let Some(draft) = &mut self.scripting {
                    draft.name = name.clone();
                }
            }
            GraphMsg::ScriptAction(action) => {
                if let Some(draft) = &mut self.scripting {
                    let edits = action.is_edit();
                    draft.content.perform(action.clone());
                    if edits {
                        draft.checked = None;
                    }
                }
            }
            GraphMsg::ScriptCheck => {
                if let Some(draft) = &mut self.scripting {
                    draft.checked = Some(script_check(&draft.content.text()));
                }
            }
            GraphMsg::ScriptCancel => self.scripting = None,
            GraphMsg::ForgetScript(k) => forget_script(*k),
            GraphMsg::TrustScripts => {
                if let Some(fingerprint) = self.script_fingerprint() {
                    trust_scripts(fingerprint);
                }
                self.scripts_paused = false;
            }
            GraphMsg::ScriptSave => {
                if let Some(draft) = self.scripting.take() {
                    self.record();
                    self.apply_script(draft.node, draft.name, draft.content.text());
                }
            }
            GraphMsg::CompositeEditSave => {
                if let Some(draft) = self.editing.take() {
                    self.record();
                    self.apply_composite_draft(draft);
                }
            }
            GraphMsg::DragStart(id) => self.dragging = Some(*id),
            GraphMsg::DragDrop(target) => {
                if let Some(id) = self.dragging.take() {
                    if self.can_move_to(id, *target) {
                        let links = self.move_to_breaks(id, *target);
                        if links.is_empty() {
                            self.record();
                            self.move_to(id, *target);
                        } else {
                            self.confirm = Some(Confirm {
                                action: GraphMsg::MoveToNow(id, *target),
                                node: id,
                                delete: false,
                                links,
                            });
                        }
                    }
                }
            }
            GraphMsg::MoveToNow(id, target) => {
                self.record();
                self.move_to(*id, *target);
            }
            GraphMsg::PickCancel => self.picking = None,
            GraphMsg::Choose(from) => {
                if let Some(to) = self.picking.take() {
                    self.record();
                    self.engine.connect(from.clone(), to);
                }
            }
            GraphMsg::PickNewInput(kind, value) => {
                let (Some(to), Some(spec)) = (self.picking.take(), graph::spec(kind)) else {
                    return;
                };
                self.record();
                let label = to.field.replace('_', " ");
                let name =
                    self.unique_name(&format!("{} {label}", self.name(to.node)), None, false);
                let input = self.add_named(Kind::Op(spec), name, Vec::new());
                if let Some(node) = self.engine.node_mut(input) {
                    node.params.insert("value".to_owned(), value.clone());
                }
                self.engine.connect(
                    Port {
                        node: input,
                        field: "value".to_owned(),
                    },
                    to,
                );
            }
            GraphMsg::Unlink(port) => {
                if self.engine.is_linked(port) {
                    self.record();
                    self.engine.disconnect(port);
                }
                if self.picking.as_ref() == Some(port) {
                    self.picking = None;
                }
            }
            GraphMsg::Input(port, value) => {
                self.drafts.insert(port.clone(), value.clone());
            }
            GraphMsg::Commit(port) => {
                if self.drafts.contains_key(port) {
                    self.record();
                }
            }
            GraphMsg::Set(port, _) => {
                // A slider drag sends many values; keep it one undo step.
                if self.recording.as_ref() != Some(port) {
                    self.record();
                    self.recording = Some(port.clone());
                }
            }
            GraphMsg::Zoom(steps) => {
                self.zoom_steps = if *steps == 0 {
                    0
                } else {
                    (self.zoom_steps + steps).clamp(*ZOOM_STEPS.start(), *ZOOM_STEPS.end())
                };
            }
            GraphMsg::ToggleSteps => {
                self.steps_hidden = !self.steps_hidden;
                self.picking = None;
            }
            GraphMsg::TogglePreview => self.preview_hidden = !self.preview_hidden,
            GraphMsg::GroupNew(id) => {
                self.record();
                self.new_group(*id);
            }
            GraphMsg::GroupAssign(id, group) => {
                if self.node_group(*id) != *group {
                    self.record();
                    if let Some(ui) = self.ui.get_mut(id) {
                        ui.group = *group;
                    }
                    self.prune_groups();
                }
            }
            GraphMsg::GroupCollapse(id) => {
                if let Some(group) = self.groups.iter_mut().find(|group| group.id == *id) {
                    group.collapsed = !group.collapsed;
                }
            }
            GraphMsg::GroupColor(id) => {
                self.record();
                if let Some(group) = self.groups.iter_mut().find(|group| group.id == *id) {
                    group.color = (group.color + 1) % NODE_COLORS.len();
                }
            }
            GraphMsg::GroupDelete(id) => {
                self.record();
                for ui in self.ui.values_mut() {
                    if ui.group == Some(*id) {
                        ui.group = None;
                    }
                }
                self.prune_groups();
            }
            GraphMsg::Undo => {
                if let Some(snapshot) = self.undo.pop() {
                    self.redo.push(self.snapshot());
                    self.restore(snapshot);
                    self.revision += 1;
                }
            }
            GraphMsg::Redo => {
                if let Some(snapshot) = self.redo.pop() {
                    self.undo.push(self.snapshot());
                    self.restore(snapshot);
                    self.revision += 1;
                }
            }
            _ => {}
        }
    }

    /// Clears the canvas state that points at `id`, ahead of its removal.
    fn forget(&mut self, id: NodeId) {
        if self.picking.as_ref().is_some_and(|port| port.node == id) {
            self.picking = None;
        }
        if self.renaming.as_ref().map(|(target, _)| *target) == Some(Renaming::Node(id)) {
            self.renaming = None;
        }
        if self.found == Some(id) {
            self.found = None;
        }
    }

    /// Removes `id` from the graph, returning the entities it owned.
    pub fn remove_node(&mut self, id: NodeId) -> Vec<Handle> {
        self.engine.remove(id);
        self.drafts.retain(|port, _| port.node != id);
        let handles = self.ui.remove(&id).map(|ui| ui.handles).unwrap_or_default();
        self.prune_groups();
        handles
    }
}

/// `card` centred over a dimmed backdrop; a click on the backdrop sends
/// `dismiss`.
fn modal<'a>(card: Element<'a, Message>, dismiss: GraphMsg) -> Element<'a, Message> {
    stack![
        mouse_area(
            container(Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_: &Theme| container::Style {
                    background: Some(Background::Color(Color::BLACK.scale_alpha(0.25))),
                    ..Default::default()
                })
        )
        .on_press(msg(dismiss)),
        center(opaque(card)),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Lays its content out at natural size and draws and hit-tests it scaled by
/// `zoom` from its top-left, reporting the scaled size so a surrounding
/// scrollable scrolls the zoomed picture. Ctrl + wheel over it zooms. Its
/// content's overlays (tooltips, lists) are scaled the same way.
struct Scaled<'a> {
    content: Element<'a, Message>,
    zoom: f32,
}

#[derive(Default)]
struct ScaledState {
    modifiers: iced::keyboard::Modifiers,
}

/// Scaling by `zoom` about `origin`.
fn scale_about(origin: Point, zoom: f32) -> Transformation {
    Transformation::translate(origin.x, origin.y)
        * Transformation::scale(zoom)
        * Transformation::translate(-origin.x, -origin.y)
}

impl Widget<Message, Theme, iced::Renderer> for Scaled<'_> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<ScaledState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(ScaledState::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn diff(&mut self, tree: &mut widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let inner = layout::Limits::new(Size::ZERO, limits.max() * (1.0 / self.zoom));
        let content = self
            .content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, &inner);
        let size = content.size() * self.zoom;
        layout::Node::with_children(size, vec![content])
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &iced::Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        match event {
            iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) => {
                tree.state.downcast_mut::<ScaledState>().modifiers = *modifiers;
            }
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta })
                if tree.state.downcast_ref::<ScaledState>().modifiers.command()
                    && cursor.is_over(*viewport) =>
            {
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / 40.0,
                };
                if lines != 0.0 {
                    shell.publish(msg(GraphMsg::Zoom(lines.signum() as i32)));
                }
                shell.capture_event();
                return;
            }
            _ => {}
        }
        let inverse = scale_about(layout.position(), self.zoom).inverse();
        let Some(content) = layout.children().next() else {
            return;
        };
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            content,
            cursor * inverse,
            renderer,
            shell,
            &(*viewport * inverse),
        );
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let inverse = scale_about(layout.position(), self.zoom).inverse();
        layout
            .children()
            .next()
            .map_or(mouse::Interaction::None, |content| {
                self.content.as_widget().mouse_interaction(
                    &tree.children[0],
                    content,
                    cursor * inverse,
                    &(*viewport * inverse),
                    renderer,
                )
            })
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        if let Some(content) = layout.children().next() {
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                content,
                renderer,
                operation,
            );
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let Some(content) = layout.children().next() else {
            return;
        };
        let bounds = layout.bounds();
        let transformation = scale_about(bounds.position(), self.zoom);
        let inverse = transformation.inverse();
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        renderer.with_layer(clip, |renderer| {
            renderer.with_transformation(transformation, |renderer| {
                self.content.as_widget().draw(
                    &tree.children[0],
                    renderer,
                    theme,
                    style,
                    content,
                    cursor * inverse,
                    &(clip * inverse),
                );
            });
        });
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, iced::Renderer>> {
        let local = scale_about(layout.position(), self.zoom);
        // On screen the content sits at its layout position plus
        // `translation`, scaled about that point.
        let transformation = scale_about(layout.position() + translation, self.zoom);
        let zoom = self.zoom;
        let content = layout.children().next()?;
        let inner = self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            content,
            renderer,
            &(*viewport * local.inverse()),
            translation,
        )?;
        Some(overlay::Element::new(Box::new(ScaledOverlay {
            inner,
            transformation,
            zoom,
        })))
    }
}

impl<'a> From<Scaled<'a>> for Element<'a, Message> {
    fn from(scaled: Scaled<'a>) -> Self {
        Element::new(scaled)
    }
}

/// An overlay of [`Scaled`] content, drawn and hit-tested through the same
/// scaling. Its layout reports the scaled bounds, so the runtime routes the
/// cursor to it where it is actually drawn.
struct ScaledOverlay<'a> {
    inner: overlay::Element<'a, Message, Theme, iced::Renderer>,
    transformation: Transformation,
    zoom: f32,
}

impl overlay::Overlay<Message, Theme, iced::Renderer> for ScaledOverlay<'_> {
    fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        let node = self
            .inner
            .as_overlay_mut()
            .layout(renderer, bounds * (1.0 / self.zoom));
        let unscaled = node.bounds();
        let scaled = unscaled * self.transformation;
        // The child keeps its unscaled absolute position under the scaled
        // parent.
        let child = node.move_to(Point::new(unscaled.x - scaled.x, unscaled.y - scaled.y));
        layout::Node::with_children(scaled.size(), vec![child]).move_to(scaled.position())
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        let Some(child) = layout.children().next() else {
            return;
        };
        let inverse = self.transformation.inverse();
        renderer.with_transformation(self.transformation, |renderer| {
            self.inner
                .as_overlay()
                .draw(renderer, theme, style, child, cursor * inverse);
        });
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        if let Some(child) = layout.children().next() {
            self.inner
                .as_overlay_mut()
                .operate(child, renderer, operation);
        }
    }

    fn update(
        &mut self,
        event: &iced::Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        if let Some(child) = layout.children().next() {
            self.inner.as_overlay_mut().update(
                event,
                child,
                cursor * self.transformation.inverse(),
                renderer,
                shell,
            );
        }
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        layout
            .children()
            .next()
            .map_or(mouse::Interaction::None, |child| {
                self.inner.as_overlay().mouse_interaction(
                    child,
                    cursor * self.transformation.inverse(),
                    renderer,
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(name: &str) -> PaletteItem {
        PaletteItem::Op(graph::spec(name).unwrap())
    }

    fn port(node: NodeId, field: &str) -> Port {
        Port {
            node,
            field: field.to_owned(),
        }
    }

    fn link(graph: &mut Graph, from: Port, to: Port) {
        graph.update_ui(&GraphMsg::Pick(to));
        graph.update_ui(&GraphMsg::Choose(from));
    }

    #[test]
    fn a_dragged_card_drops_into_place_asking_when_it_breaks_links() {
        let mut graph = Graph::default();
        let input = graph.add_node(op("Number"), Vec::new());
        let first = graph.add_node(op("Add"), Vec::new());
        let second = graph.add_node(op("Negate"), Vec::new());
        let third = graph.add_node(op("Pi"), Vec::new());
        link(&mut graph, port(input, "value"), port(first, "a"));
        link(&mut graph, port(first, "result"), port(second, "x"));

        // Pi has no links: dropped on Add it takes its place at once.
        graph.update_ui(&GraphMsg::DragStart(third));
        graph.update_ui(&GraphMsg::DragDrop(first));
        assert!(graph.confirm.is_none());
        assert_eq!((graph.steps()[&third], graph.steps()[&first]), (0, 1));
        assert!(graph.dragging.is_none());

        // Negate before Add would read a later step: it asks first.
        graph.update_ui(&GraphMsg::DragStart(second));
        graph.update_ui(&GraphMsg::DragDrop(first));
        let confirm = graph.confirm.clone().expect("the drop asks");
        assert_eq!(confirm.links.len(), 1);
        graph.confirm = None;
        graph.update_ui(&confirm.action);
        assert_eq!(graph.steps()[&second], 1);
        assert!(!graph.engine.is_linked(&port(second, "x")));

        // An input never drops among the steps.
        graph.update_ui(&GraphMsg::DragStart(input));
        graph.update_ui(&GraphMsg::DragDrop(first));
        assert!(graph.confirm.is_none());
        assert!(is_input(&graph.engine.nodes[0].kind));
        graph.update_ui(&GraphMsg::Undo);
        assert_eq!(graph.steps()[&second], 2);
    }

    #[test]
    fn a_composite_edit_renames_ports_with_their_links() {
        let mut graph = Graph::default();
        let input = graph.add_node(op("Number"), Vec::new());
        let add = graph.add_node(op("Add"), Vec::new());
        let negate = graph.add_node(op("Negate"), Vec::new());
        let after = graph.add_node(op("Absolute"), Vec::new());
        link(&mut graph, port(input, "value"), port(add, "a"));
        link(&mut graph, port(add, "result"), port(negate, "x"));
        link(&mut graph, port(negate, "result"), port(after, "x"));
        graph.update_ui(&GraphMsg::GroupNew(add));
        let group = graph.node_group(add).unwrap();
        graph.update_ui(&GraphMsg::GroupAssign(negate, Some(group)));
        let id = graph.make_composite(group).unwrap();

        graph.update_ui(&GraphMsg::CompositeEdit(id));
        let draft = graph.editing.clone().expect("the editor opens");
        assert_eq!(draft.inputs.len(), 1);
        let old_output = draft.outputs[0].0.clone();
        graph.update_ui(&GraphMsg::CompositeDraftName("Opposite".into()));
        graph.update_ui(&GraphMsg::CompositeDraftDescription(
            "Minus a plus one".into(),
        ));
        graph.update_ui(&GraphMsg::CompositeDraftInput(0, "Amount".into()));
        graph.update_ui(&GraphMsg::CompositeDraftOutput(0, "Opposite".into()));
        graph.update_ui(&GraphMsg::CompositeEditSave);
        assert!(graph.editing.is_none());

        let Kind::Composite(composite) = &graph.engine.node(id).unwrap().kind else {
            panic!("still a composite");
        };
        assert_eq!(composite.name, "Opposite");
        assert_eq!(composite_description(composite), "Minus a plus one");
        assert!(graph.engine.is_linked(&port(id, "Amount")));
        assert!(graph
            .engine
            .links
            .iter()
            .any(|link| link.from == port(id, "Opposite") && link.to == port(after, "x")));
        assert!(describe(&graph.engine.node(id).unwrap().kind).starts_with("Minus a plus one"));
        graph.update_ui(&GraphMsg::Undo);
        assert!(graph
            .engine
            .links
            .iter()
            .any(|link| link.from == port(id, &old_output)));
    }

    #[test]
    fn tables_edit_as_cells_and_lists_preview_their_items() {
        let cells = table_cells(&Value::Null);
        assert_eq!(
            cells,
            vec![
                vec!["A".to_owned(), "B".to_owned()],
                vec![String::new(), String::new()]
            ]
        );
        let pasted = table_cells(&Value::String("at\twidth\n2\t0.9\n6".into()));
        assert_eq!(
            pasted,
            vec![
                vec!["at".to_owned(), "width".to_owned()],
                vec!["2".to_owned(), "0.9".to_owned()],
                vec!["6".to_owned(), String::new()],
            ]
        );
        let mut graph = Graph::default();
        let table = graph.add_node(op("Table"), Vec::new());
        assert!(is_input(&graph.engine.node(table).unwrap().kind));
        // The app writes a Set value; the canvas only records the undo step.
        graph
            .engine
            .node_mut(table)
            .unwrap()
            .params
            .insert("table".into(), table_value(&pasted));
        let sections = graph.sections(HashMap::default());
        let index = graph
            .engine
            .nodes
            .iter()
            .position(|node| node.id == table)
            .unwrap();
        let labels: Vec<&str> = sections[index][0]
            .rows
            .iter()
            .map(|row| row.label.as_str())
            .collect();
        assert!(
            labels.contains(&"at") && labels.contains(&"width"),
            "{labels:?}"
        );
        assert_eq!(list_preview(&json!(1)), None);
        let preview = list_preview(&json!([1, 2, 3])).unwrap();
        assert!(preview.ends_with("2: 3"), "{preview}");
    }

    #[test]
    fn nodes_of_one_type_get_distinct_names() {
        let mut graph = Graph::default();
        let a = graph.add_node(op("Add"), Vec::new());
        let b = graph.add_node(op("Add"), Vec::new());
        assert_ne!(graph.name(a), graph.name(b));
        graph.update_ui(&GraphMsg::RenameStart(b));
        graph.update_ui(&GraphMsg::RenameInput(graph.name(a).to_uppercase()));
        graph.update_ui(&GraphMsg::RenameCommit);
        assert_ne!(graph.name(a).to_lowercase(), graph.name(b).to_lowercase());
    }

    #[test]
    fn a_move_that_breaks_links_asks_first() {
        let mut graph = Graph::default();
        let input = graph.add_node(op("Number"), Vec::new());
        let first = graph.add_node(op("Add"), Vec::new());
        let second = graph.add_node(op("Negate"), Vec::new());
        link(&mut graph, port(input, "value"), port(first, "a"));
        link(&mut graph, port(first, "result"), port(second, "x"));
        assert_eq!(graph.engine.links.len(), 2);
        assert!(graph.picking.is_none());

        graph.update_ui(&GraphMsg::Move(second, -1));
        let confirm = graph.confirm.clone().expect("the move asks");
        assert_eq!(confirm.links.len(), 1);
        assert_eq!(graph.steps()[&second], 1, "nothing moved yet");

        graph.update_ui(&GraphMsg::ConfirmYes);
        graph.confirm = None;
        graph.update_ui(&confirm.action);
        assert_eq!(graph.steps()[&second], 0);
        assert!(!graph.engine.is_linked(&port(second, "x")));
        assert!(graph.engine.is_linked(&port(first, "a")));

        graph.update_ui(&GraphMsg::Undo);
        assert_eq!(graph.steps()[&second], 1);
        assert!(graph.engine.is_linked(&port(second, "x")));
        graph.update_ui(&GraphMsg::Redo);
        assert_eq!(graph.steps()[&second], 0);
    }

    #[test]
    fn deleting_a_used_node_asks_first() {
        let mut graph = Graph::default();
        let input = graph.add_node(op("Number"), Vec::new());
        let step = graph.add_node(op("Negate"), Vec::new());
        link(&mut graph, port(input, "value"), port(step, "x"));
        graph.update_ui(&GraphMsg::NodeDelete(input));
        assert!(graph.confirm.as_ref().is_some_and(|confirm| confirm.delete));
        graph.update_ui(&GraphMsg::NodeDelete(step));
        assert!(graph
            .confirm
            .as_ref()
            .is_some_and(|confirm| confirm.node == input));
    }

    #[test]
    fn duplicates_keep_values_and_sources() {
        let mut graph = Graph::default();
        let input = graph.add_node(op("Number"), Vec::new());
        let step = graph.add_node(op("Add"), Vec::new());
        let last = graph.add_node(op("Negate"), Vec::new());
        link(&mut graph, port(input, "value"), port(step, "a"));
        graph
            .engine
            .node_mut(step)
            .unwrap()
            .params
            .insert("b".into(), json!(7));
        graph.update_ui(&GraphMsg::Duplicate(step));
        let (original, copy) = graph.duplicated.unwrap();
        assert_eq!(original, step);
        assert_eq!(graph.steps()[&copy], 1, "right after the original");
        assert_eq!(graph.steps()[&last], 2);
        assert!(graph.engine.is_linked(&port(copy, "a")));
        assert_eq!(graph.engine.node(copy).unwrap().params["b"], json!(7));
        assert_ne!(graph.name(copy), graph.name(step));
        graph.update_ui(&GraphMsg::Undo);
        assert!(graph.engine.node(copy).is_none());
    }

    #[test]
    fn copied_nodes_paste_with_their_links_groups_and_named_sources() {
        let mut source = Graph::default();
        let size = source.add_node(op("Number"), Vec::new());
        let add = source.add_node(op("Add"), Vec::new());
        let negate = source.add_node(op("Negate"), Vec::new());
        link(&mut source, port(size, "value"), port(add, "a"));
        link(&mut source, port(add, "result"), port(negate, "x"));
        source.update_ui(&GraphMsg::GroupNew(add));
        source.update_ui(&GraphMsg::GroupAssign(negate, source.node_group(add)));
        let text = source.copy_text(&[add, negate]);

        // Another graph with an input of the same name as the outer source.
        let mut target = Graph::default();
        let other = target.add_node(op("Number"), Vec::new());
        assert_eq!(target.name(other), source.name(size));
        let added = target.paste_text(&text).unwrap();
        assert_eq!(added.len(), 2);
        let (copy_add, copy_negate) = (added[0], added[1]);
        assert!(
            target.engine.is_linked(&port(copy_negate, "x")),
            "inner link kept"
        );
        assert_eq!(
            target
                .engine
                .links
                .iter()
                .find(|l| l.to == port(copy_add, "a"))
                .map(|l| l.from.node),
            Some(other),
            "outer link found by name"
        );
        let group = target.node_group(copy_add);
        assert!(group.is_some() && group == target.node_group(copy_negate));

        // Pasting again beside the originals renames the copies.
        let again = source.paste_text(&text).unwrap();
        assert_ne!(source.name(again[0]), source.name(add));
        assert!(source.paste_text("not nodes").is_err());
        source.update_ui(&GraphMsg::Undo);
        assert!(source.engine.node(again[0]).is_none());
    }

    #[test]
    fn numbers_turn_into_sliders_and_back_keeping_links() {
        let mut graph = Graph::default();
        let size = graph.add_node(op("Number"), Vec::new());
        let step = graph.add_node(op("Negate"), Vec::new());
        link(&mut graph, port(size, "value"), port(step, "x"));
        graph
            .engine
            .node_mut(size)
            .unwrap()
            .params
            .insert("value".into(), json!(4.0));
        graph.update_ui(&GraphMsg::ToggleSlider(size));
        let node = graph.engine.node(size).unwrap();
        assert!(matches!(&node.kind, Kind::Op(spec) if spec.name == "Number Slider"));
        assert_eq!(node.params["value"], json!(4.0));
        assert_eq!(node.params["max"], json!(8.0));
        assert!(graph.engine.is_linked(&port(step, "x")));
        graph.update_ui(&GraphMsg::ToggleSlider(size));
        let node = graph.engine.node(size).unwrap();
        assert!(matches!(&node.kind, Kind::Op(spec) if spec.name == "Number"));
        assert!(!node.params.contains_key("max"));
    }

    #[test]
    fn the_find_box_steps_through_matches_and_opens_their_group() {
        let mut graph = Graph::default();
        let wall = graph.add_node(op("Add"), Vec::new());
        let _other = graph.add_node(op("Negate"), Vec::new());
        let wall_two = graph.add_node(op("Add"), Vec::new());
        graph.update_ui(&GraphMsg::GroupNew(wall_two));
        let group = graph.node_group(wall_two).unwrap();
        graph.update_ui(&GraphMsg::GroupCollapse(group));
        graph.update_ui(&GraphMsg::NodeSearch("add".into()));
        assert_eq!(graph.search_matches(), vec![wall, wall_two]);
        assert_eq!(graph.found, Some(wall));
        assert_eq!(graph.found_place(), Some((false, 0.0)));
        graph.update_ui(&GraphMsg::NodeSearchStep(1));
        assert_eq!(graph.found, Some(wall_two));
        assert_eq!(graph.found_place(), Some((false, 1.0)));
        assert!(!graph.group(group).unwrap().collapsed, "its group opens");
        graph.update_ui(&GraphMsg::NodeSearchStep(1));
        assert_eq!(graph.found, Some(wall), "wraps round");
        graph.update_ui(&GraphMsg::NodeSearch("nothing".into()));
        assert_eq!(graph.found, None);
    }

    #[test]
    fn script_edits_keep_values_and_links_of_the_ports_they_keep() {
        let mut graph = Graph::default();
        let input = graph.add_node(op("Number"), Vec::new());
        let script = graph.add_node(PaletteItem::NewScript, Vec::new());
        let after = graph.add_node(op("Negate"), Vec::new());
        link(&mut graph, port(input, "value"), port(script, "a"));
        link(&mut graph, port(script, "sum"), port(after, "x"));
        graph.update_ui(&GraphMsg::ScriptEdit(script));
        let draft = graph.scripting.as_ref().unwrap();
        assert!(draft.checked.as_ref().is_some_and(Result::is_ok));
        // Keep `a` and `sum`; drop `b` and `product`; add `k`.
        graph.update_ui(&GraphMsg::ScriptName("Scaled".into()));
        graph.scripting.as_mut().unwrap().content = iced::widget::text_editor::Content::with_text(
            "// inputs: a: number, k: number = 3\n// outputs: sum: number\na * k",
        );
        graph.update_ui(&GraphMsg::ScriptCheck);
        let checked = graph
            .scripting
            .as_ref()
            .unwrap()
            .checked
            .clone()
            .unwrap()
            .unwrap();
        assert!(checked.contains("k (number)"), "{checked}");
        graph.update_ui(&GraphMsg::ScriptSave);
        assert!(graph.scripting.is_none());
        assert!(graph.engine.is_linked(&port(script, "a")));
        assert!(
            graph.engine.is_linked(&port(after, "x")),
            "sum is still there"
        );
        let node = graph.engine.node(script).unwrap();
        assert_eq!(node.params["k"], json!(3));
        assert!(!node.params.contains_key("b"));
        assert_eq!(kind_name(&node.kind), "Scaled");
        graph.update_ui(&GraphMsg::Undo);
        let Kind::Script(back) = &graph.engine.node(script).unwrap().kind else {
            panic!()
        };
        assert_eq!(back.source, graph::script::TEMPLATE);
    }

    #[test]
    fn the_picker_can_create_the_input_it_links() {
        let mut graph = Graph::default();
        let step = graph.add_node(op("Add"), Vec::new());
        graph.update_ui(&GraphMsg::Pick(port(step, "b")));
        graph.update_ui(&GraphMsg::PickNewInput("Number", json!(3)));
        let link = graph.engine.links.first().expect("linked");
        let input = graph.engine.node(link.from.node).unwrap();
        assert!(is_input(&input.kind));
        assert_eq!(input.params["value"], json!(3));
        assert_eq!(link.to, port(step, "b"));
    }

    #[test]
    fn moving_across_a_group_edge_joins_the_group() {
        let mut graph = Graph::default();
        let a = graph.add_node(op("Pi"), Vec::new());
        let b = graph.add_node(op("E"), Vec::new());
        graph.update_ui(&GraphMsg::GroupNew(b));
        let group = graph.node_group(b);
        assert!(group.is_some());
        graph.update_ui(&GraphMsg::Move(a, 1));
        assert_eq!(graph.node_group(a), group, "joined in place");
        assert_eq!(graph.steps()[&a], 0);
        graph.update_ui(&GraphMsg::Move(a, 1));
        assert_eq!(graph.steps()[&a], 1, "then moves within it");
        graph.update_ui(&GraphMsg::GroupDelete(group.unwrap()));
        assert!(graph.groups.is_empty());
    }

    #[test]
    fn picker_types_dim_what_does_not_fit() {
        assert!(Expect::Number.fits(&json!(2.5)));
        assert!(Expect::Number.fits(&json!([1, 2])));
        assert!(!Expect::Number.fits(&json!("abc")));
        assert!(Expect::Text.fits(&json!(4)));
        assert!(!Expect::Bool.fits(&json!(1)));
        assert!(Expect::Object.fits(&json!({ "handle": 5 })));
        assert!(Expect::Any.fits(&json!({ "rgb": [1, 2, 3] })));
    }

    #[test]
    fn saved_graphs_keep_names_order_groups_and_view() {
        let mut graph = Graph::default();
        graph.add_node(op("Number"), Vec::new());
        let a = graph.add_node(op("Add"), Vec::new());
        let b = graph.add_node(op("Add"), Vec::new());
        graph.update_ui(&GraphMsg::RenameStart(a));
        graph.update_ui(&GraphMsg::RenameInput("Width".into()));
        graph.update_ui(&GraphMsg::RenameCommit);
        graph.update_ui(&GraphMsg::Move(b, -1));
        graph.update_ui(&GraphMsg::GroupNew(a));
        graph.update_ui(&GraphMsg::Zoom(3));
        graph.update_ui(&GraphMsg::ToggleSteps);
        let (loaded, _) = Graph::from_file(&graph.to_file("")).unwrap();
        assert_eq!(loaded.name(a), "Width");
        assert_eq!(loaded.steps()[&b], 0);
        assert_eq!(loaded.groups.len(), 1);
        assert_eq!(loaded.node_group(a), graph.node_group(a));
        assert_eq!(loaded.zoom_steps, 3);
        assert!(loaded.steps_hidden);
    }
}
