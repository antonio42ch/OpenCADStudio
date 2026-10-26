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

/// Operations that live in the Inputs area: the only nodes whose values are
/// typed by hand.
const INPUT_KINDS: &[&str] = &[
    "Number",
    "Number Slider",
    "Integer Slider",
    "Text",
    "Boolean",
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
    /// Draws curve values the graph computes: the one object node whose
    /// entity kind follows its input.
    Draw,
}

const OBJECTS: &[ObjectKind] = &[
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
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        OBJECTS.iter().copied().find(|kind| kind.name() == name)
    }

    /// The kind of a drawn entity, when a node can own it.
    pub fn of(entity: &EntityType) -> Option<Self> {
        OBJECTS.iter().copied().find(|kind| kind.matches(entity))
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
            // A Draw node's entities come from its geometry input.
            ObjectKind::Draw => {
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
        }
    }

    /// The node kind it adds; `None` for a library entry that is gone.
    pub fn kind(self) -> Option<Kind> {
        Some(match self {
            PaletteItem::Object(kind) => Kind::Object(kind.name().to_owned()),
            PaletteItem::Op(spec) => Kind::Op(spec),
            PaletteItem::Composite(k) => Kind::Composite(composite_library().get(k)?.clone()),
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

/// Drops library entry `k`.
pub fn forget_library_entry(k: usize) {
    if let Ok(mut items) = library_store().lock() {
        if k < items.len() {
            items.remove(k);
            persist_library(&items);
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
        Kind::Object(_) => None,
    }
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
        Kind::Composite(composite) => crate::tf!(
            "A composite of {} steps used as one node. Expand it to see or change them.",
            composite.graph.nodes.len()
        )
        .into_owned(),
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
    Slider { min: f64, max: f64, step: f64 },
    Toggle,
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
        rows.extend(
            spec.outputs
                .iter()
                .filter(|name| !spec.inputs.iter().any(|input| input.name == **name))
                .map(|name| NodeRow {
                    field: (*name).to_owned(),
                    label: (*name).to_owned(),
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
            Some(Kind::Object(_)) => Expect::of_value(value),
            None => Expect::Any,
        }
    }

    pub fn view(&self, sections: Vec<Vec<NodeSection>>) -> Element<'_, Message> {
        let overlay = if self.steps_hidden {
            None
        } else if let Some(confirm) = &self.confirm {
            Some(self.confirm_dialog(confirm, &sections))
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
        let toolbar = row![
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
            Space::new().width(10),
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
        let sequence = column![
            row![
                text(t!("Node Sequence")).size(13).width(Length::Fill),
                toolbar
            ]
            .align_y(iced::Center),
            scrollable(container(sequence_body).padding(iced::Padding {
                top: 0.0,
                right: 12.0,
                bottom: 4.0,
                left: 0.0,
            }))
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
        let mut layers = stack![backdrop];
        if let Some(overlay) = overlay {
            layers = layers.push(overlay);
        }
        opaque(layers.width(Length::Fill).height(Length::Fill))
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
            Space::new().width(Length::Fill),
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
        if matches!(node.kind, Kind::Composite(_)) {
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
        container(header)
            .padding([5, 6])
            .width(Length::Fill)
            .style(header_style)
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
        content: impl Into<Element<'a, Message>>,
    ) -> container::Container<'a, Message> {
        let color = node_color(id);
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
        let extra = vec![
            icon_button(
                crate::ui::icons::TRI_UP,
                "Move up",
                self.can_move(id, -1).then_some(GraphMsg::Move(id, -1)),
            ),
            icon_button(
                crate::ui::icons::TRI_DOWN,
                "Move down",
                self.can_move(id, 1).then_some(GraphMsg::Move(id, 1)),
            ),
        ];
        let mut body = column![].spacing(4).padding([6, 8]);
        for node_row in sections.into_iter().flat_map(|section| section.rows) {
            if node_row.input {
                body = body.push(self.input_row(id, node_row));
            }
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
                self.group_picker(id),
            ]
            .spacing(4)
            .align_y(iced::Center),
        );
        Self::card(id, column![self.header(node, None, extra), body])
            .width(Length::Fill)
            .into()
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
            RowWidget::Field => {
                let draft = self.drafts.get(&port).cloned().unwrap_or(shown);
                let edit = port.clone();
                text_input("", &draft)
                    .on_input(move |s| msg(GraphMsg::Input(edit.clone(), s)))
                    .on_submit(msg(GraphMsg::Commit(port)))
                    .size(11)
                    .padding([3, 5])
                    .width(Length::Fill)
                    .into()
            }
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
        if sections.is_empty() {
            body = body.push(
                container(text(t!("Object deleted")).size(11).style(weak_text)).padding([2, 2]),
            );
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
                            text(display(&node_row.value))
                                .size(11)
                                .style(weak_text)
                                .width(Length::Fill),
                        ]
                        .spacing(4)
                        .padding([3, 4])
                        .align_y(iced::Center),
                    );
                }
            }
        }
        body = body.push(
            row![Space::new().width(Length::Fill), self.group_picker(id)]
                .spacing(4)
                .padding([4, 2])
                .align_y(iced::Center),
        );
        Self::card(id, column![self.header(node, Some(step), extra), body])
            .width(NODE_W)
            .into()
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
                    text(format!("= {shown}")).size(10).style(weak_text),
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
                })
                .filter(|entry| {
                    query.is_empty() || name_match || entry.label.to_lowercase().contains(&query)
                })
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

        let mut list = column![].spacing(8);
        if groups.is_empty() {
            list = list.push(
                text(t!(
                    "No values to choose from. Add inputs, or steps earlier in the node sequence."
                ))
                .size(11)
                .style(weak_text),
            );
        }
        for group in groups {
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
                            text(entry.value)
                                .size(11)
                                .style(weak_text)
                                .width(Length::FillPortion(3)),
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
            list = list.push(container(entries).padding(4).width(Length::Fill).style(
                move |_: &Theme| container::Style {
                    background: Some(Background::Color(color.scale_alpha(0.08))),
                    border: Border {
                        color: color.scale_alpha(0.6),
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    ..Default::default()
                },
            ));
        }

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
                text_input(&t!("Search values…"), &self.pick_search)
                    .id(iced::widget::Id::new(PICK_SEARCH_ID))
                    .on_input(|value| msg(GraphMsg::PickSearch(value)))
                    .padding([4, 8])
                    .size(12),
                scrollable(container(list).padding(iced::Padding {
                    top: 0.0,
                    right: 10.0,
                    bottom: 0.0,
                    left: 0.0,
                }))
                .height(Length::Fixed(380.0)),
                actions,
            ]
            .spacing(8),
        )
        .padding(12)
        .width(PICKER_W)
        .style(card_style);
        modal(card.into(), GraphMsg::PickCancel)
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
            PaletteItem::Op(_) | PaletteItem::Composite(_) => Space::new().width(14.0).into(),
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
        let categories = ["Objects", "Composites"]
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
            } else {
                graph::LIBRARY
                    .iter()
                    .filter(|spec| spec.category == category)
                    .map(PaletteItem::Op)
                    .collect()
            };
            let items: Vec<PaletteItem> = items.into_iter().filter(|item| wanted(*item)).collect();
            if items.is_empty() {
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
