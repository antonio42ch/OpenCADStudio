//! Node graph operations for automation clients: the `graph_library`,
//! `graph_state` and `graph_validate` reads and the `graph_apply` edit.
//!
//! A `graph_apply` request is a list of changes applied all-or-nothing as one
//! graph undo step, with the canvas's own rules: values flow only forward
//! along the node sequence, hand-typed values live only in the Inputs area,
//! and an edit that drops links needs `force`. Nodes are addressed by their
//! unique names (or `#id`), ports as `"Node.port"`.

use std::cell::RefCell;

use super::{Message, OpenCADStudio};
use crate::ui::node_graph::{
    is_input, kind_name, Expect, Graph, NodeGroup, NodeSection, ObjectKind, PaletteItem,
};
use codec::Handle;
use graph::{Kind, Link, NodeId, Port};
use iced::Task;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use serde_json::{json, Value};

/// Most changes one request may carry.
const MAX_CHANGES: usize = 256;

const VERBS: &[&str] = &[
    "add",
    "remove",
    "rename",
    "set",
    "link",
    "unlink",
    "move",
    "group",
    "ungroup",
    "duplicate",
    "adopt",
    "clear",
    "composite",
    "expand",
    "save_composite",
];

/// A refused change.
#[derive(Debug)]
pub(crate) struct ChangeError {
    pub index: usize,
    pub code: &'static str,
    pub message: String,
    /// What to try instead.
    pub hint: Option<String>,
    /// Links the change would drop, for `would_break_links`.
    pub links: Vec<Value>,
}

impl ChangeError {
    fn to_json(&self) -> Value {
        let mut value = json!({
            "ok": false,
            "status": "failed",
            "code": self.code,
            "error": self.message,
            "index": self.index,
        });
        if let Some(hint) = &self.hint {
            value["hint"] = json!(hint);
        }
        if !self.links.is_empty() {
            value["links"] = json!(self.links);
        }
        value
    }
}

fn fail(code: &'static str, message: impl Into<String>) -> ChangeError {
    ChangeError {
        index: 0,
        code,
        message: message.into(),
        hint: None,
        links: Vec::new(),
    }
}

fn fail_hint(code: &'static str, message: impl Into<String>, hint: &str) -> ChangeError {
    ChangeError {
        hint: Some(hint.to_owned()),
        ..fail(code, message)
    }
}

/// One port of a node, as automation sees it.
#[derive(Clone, Debug)]
pub(crate) struct Field {
    pub name: String,
    pub label: String,
    /// Takes a value: from a link, or (on an input node) typed by hand.
    pub input: bool,
    pub output: bool,
    pub kind: Expect,
}

/// The kind of value a Properties row holds, from the kind the panel gives
/// it: editable numbers are formatted text, so the value alone cannot tell.
pub(crate) fn property_kind(property: &str, value: &Value) -> Expect {
    match property {
        "number" => Expect::Number,
        "text" | "hyperlink" | "layer" | "linetype" | "choice" | "editable_choice" => Expect::Text,
        "bool" => Expect::Bool,
        "readonly" => match value.as_str().map(|text| text.trim().parse::<f64>()) {
            Some(Ok(_)) => Expect::Number,
            _ => Expect::of_value(value),
        },
        _ => Expect::Any,
    }
}

/// What applying changes needs from the drawing.
pub(crate) trait Drawing {
    /// The ports of an object node of `kind`.
    fn fields(&self, kind: ObjectKind) -> Vec<Field>;
    /// The node kind that can own entity `handle`, or why none can.
    fn entity(&self, handle: Handle) -> Result<ObjectKind, ChangeError>;
}

/// What a successful apply did.
#[derive(Default, Debug)]
pub(crate) struct Report {
    pub results: Vec<Value>,
    pub warnings: Vec<Value>,
    /// `(original, copy)` per duplicated node, for the app to copy entities.
    pub duplicated: Vec<(NodeId, NodeId)>,
    /// Entities of removed nodes.
    pub removed: Vec<Handle>,
    /// Composites to save to the library once the request succeeds.
    pub saved: Vec<std::sync::Arc<graph::Composite>>,
}

impl Report {
    fn warn(&mut self, index: usize, code: &str, message: impl Into<String>) {
        self.warnings
            .push(json!({ "index": index, "code": code, "warning": message.into() }));
    }
}

fn link_json(graph: &Graph, link: &Link) -> Value {
    json!({
        "from": format!("{}.{}", graph.name(link.from.node), link.from.field),
        "to": format!("{}.{}", graph.name(link.to.node), link.to.field),
    })
}

/// The kind an input node's `value` holds.
fn input_kind(spec_name: &str) -> Expect {
    match spec_name {
        "Text" => Expect::Text,
        "Boolean" => Expect::Bool,
        _ => Expect::Number,
    }
}

/// Every port of node `id`.
pub(crate) fn node_ports(graph: &Graph, drawing: &dyn Drawing, id: NodeId) -> Vec<Field> {
    let Some(node) = graph.engine.node(id) else {
        return Vec::new();
    };
    match &node.kind {
        Kind::Op(spec) => {
            let mut fields: Vec<Field> = spec
                .inputs
                .iter()
                .map(|input| {
                    let output = spec.outputs.contains(&input.name);
                    let kind = if is_input(&node.kind) && output {
                        input_kind(spec.name)
                    } else {
                        graph.expect(
                            &Port {
                                node: id,
                                field: input.name.to_owned(),
                            },
                            &Value::Null,
                        )
                    };
                    Field {
                        name: input.name.to_owned(),
                        label: input.name.to_owned(),
                        input: true,
                        output,
                        kind,
                    }
                })
                .collect();
            fields.extend(
                spec.outputs
                    .iter()
                    .filter(|name| !spec.inputs.iter().any(|input| input.name == **name))
                    .map(|name| Field {
                        name: (*name).to_owned(),
                        label: (*name).to_owned(),
                        input: false,
                        output: true,
                        kind: output_kind(name),
                    }),
            );
            fields
        }
        Kind::Object(name) => ObjectKind::from_name(name)
            .map(|kind| drawing.fields(kind))
            .unwrap_or_default(),
        Kind::Composite(composite) => composite_ports(composite),
    }
}

/// A composite's exposed ports, typed as the inner ports they stand for.
pub(crate) fn composite_ports(composite: &graph::Composite) -> Vec<Field> {
    let inputs = composite.inputs.iter().map(|(name, _)| Field {
        name: name.clone(),
        label: name.clone(),
        input: true,
        output: false,
        kind: crate::ui::node_graph::composite_expect(composite, name),
    });
    let outputs = composite.outputs.iter().map(|(name, port)| Field {
        name: name.clone(),
        label: name.clone(),
        input: false,
        output: true,
        kind: output_kind(&port.field),
    });
    inputs.chain(outputs).collect()
}

/// The kind an operation output holds, by its name.
fn output_kind(name: &str) -> Expect {
    match name {
        "curve" | "curves" => Expect::Curve,
        "point" | "vector" | "origin" | "x_axis" | "y_axis" | "normal" | "start" | "end" => {
            Expect::Point
        }
        "plane" => Expect::Plane,
        _ => Expect::Any,
    }
}

fn resolve_node(graph: &Graph, reference: &Value) -> Result<NodeId, ChangeError> {
    let Some(text) = reference.as_str().map(str::trim) else {
        return Err(fail(
            "invalid_change",
            "A node reference is a name or \"#id\"",
        ));
    };
    if let Some(id) = text.strip_prefix('#') {
        if let Some(id) = id
            .parse::<NodeId>()
            .ok()
            .filter(|id| graph.engine.node(*id).is_some())
        {
            return Ok(id);
        }
    } else {
        let wanted = text.to_lowercase();
        if let Some(id) = graph
            .engine
            .nodes
            .iter()
            .map(|node| node.id)
            .find(|id| graph.name(*id).to_lowercase() == wanted)
        {
            return Ok(id);
        }
    }
    Err(fail_hint(
        "unknown_node",
        format!("No node is called {text}"),
        "Read graph_state for the node names",
    ))
}

/// A port reference: `"Node.port"` (split at the last dot) or
/// `{"node": …, "port": …}`. The port may be named by its field or its label.
fn resolve_port(
    graph: &Graph,
    drawing: &dyn Drawing,
    reference: &Value,
) -> Result<(Port, Field), ChangeError> {
    let (node, port) = match reference {
        Value::String(text) => match text.rsplit_once('.') {
            Some((node, port)) => (json!(node), port.trim().to_owned()),
            None => {
                return Err(fail(
                    "invalid_change",
                    format!("Port {text} is not written as Node.port"),
                ))
            }
        },
        Value::Object(object) => (
            object.get("node").cloned().unwrap_or(Value::Null),
            object
                .get("port")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        _ => {
            return Err(fail(
                "invalid_change",
                "A port reference is \"Node.port\" or {\"node\", \"port\"}",
            ))
        }
    };
    let id = resolve_node(graph, &node)?;
    let wanted = port.to_lowercase();
    let field = node_ports(graph, drawing, id)
        .into_iter()
        .find(|field| field.name.to_lowercase() == wanted)
        .or_else(|| {
            node_ports(graph, drawing, id)
                .into_iter()
                .find(|field| field.label.to_lowercase() == wanted)
        })
        .ok_or_else(|| {
            fail_hint(
                "unknown_port",
                format!("{} has no port {port}", graph.name(id)),
                "Read graph_library or graph_state for its ports",
            )
        })?;
    Ok((
        Port {
            node: id,
            field: field.name.clone(),
        },
        field,
    ))
}

/// The library item a type name stands for, ignoring case. `source`
/// (`"object"` or `"builtin"`) settles names both kinds use, such as Point
/// and Text.
fn resolve_type(name: &str, source: Option<&str>) -> Result<PaletteItem, ChangeError> {
    let wanted = name.trim().to_lowercase();
    let object = ObjectKind::ALL
        .iter()
        .copied()
        .find(|kind| kind.name().to_lowercase() == wanted)
        .map(PaletteItem::Object);
    let builtin = graph::LIBRARY
        .iter()
        .find(|spec| spec.name.to_lowercase() == wanted)
        .map(PaletteItem::Op);
    match (source, object, builtin) {
        (Some("object"), Some(item), _) | (Some("builtin"), _, Some(item)) => Ok(item),
        (None, Some(item), None) | (None, None, Some(item)) => Ok(item),
        (None, Some(_), Some(_)) => Err(fail_hint(
            "ambiguous_type",
            format!("{name} is both a drawing object and an operation"),
            "Add \"source\": \"object\" or \"source\": \"builtin\"",
        )),
        _ => Err(fail_hint(
            "unknown_type",
            format!("There is no node type {name}"),
            "Read graph_library for the node types",
        )),
    }
}

/// Moves node `id` right after `after` in `nodes`: `"start"` puts it first
/// in its area, a node reference after that node (which must share its area).
fn place(
    graph: &Graph,
    nodes: &mut Vec<graph::Node>,
    id: NodeId,
    after: &Value,
) -> Result<(), ChangeError> {
    let input = nodes
        .iter()
        .find(|node| node.id == id)
        .is_some_and(|node| is_input(&node.kind));
    let anchor = if after.as_str().map(str::trim) == Some("start") {
        None
    } else {
        let anchor = resolve_node(graph, after)?;
        if anchor == id {
            return Ok(());
        }
        let same_area = nodes
            .iter()
            .find(|node| node.id == anchor)
            .is_some_and(|node| is_input(&node.kind) == input);
        if !same_area {
            return Err(fail(
                "wrong_area",
                "Inputs are placed among inputs, steps among steps",
            ));
        }
        Some(anchor)
    };
    let Some(index) = nodes.iter().position(|node| node.id == id) else {
        return Ok(());
    };
    let node = nodes.remove(index);
    let at = match anchor {
        Some(anchor) => nodes
            .iter()
            .position(|node| node.id == anchor)
            .map_or(nodes.len(), |index| index + 1),
        None => nodes
            .iter()
            .position(|node| is_input(&node.kind) == input)
            .unwrap_or(nodes.len()),
    };
    nodes.insert(at, node);
    Ok(())
}

/// Puts node `id` in the group called `name` of its area, creating it.
fn join_group(graph: &mut Graph, id: NodeId, name: &str, color: Option<usize>) {
    let input = graph
        .engine
        .node(id)
        .is_some_and(|node| is_input(&node.kind));
    let wanted = name.to_lowercase();
    let group = match graph
        .groups
        .iter()
        .find(|group| group.inputs == input && group.name.to_lowercase() == wanted)
    {
        Some(group) => group.id,
        None => {
            let group = graph.groups.iter().map(|group| group.id).max().unwrap_or(0) + 1;
            let color = color.unwrap_or(graph.groups.len() + 2);
            graph.groups.push(NodeGroup {
                id: group,
                name: name.to_owned(),
                color,
                collapsed: false,
                inputs: input,
            });
            group
        }
    };
    if let Some(ui) = graph.ui.get_mut(&id) {
        ui.group = Some(group);
    }
    graph.prune_groups();
}

/// Writes a hand-typed value into an input node, in the kind its port holds.
fn set_value(
    graph: &mut Graph,
    drawing: &dyn Drawing,
    id: NodeId,
    port: &str,
    value: &Value,
) -> Result<(), ChangeError> {
    let node_kind = graph.engine.node(id).map(|node| node.kind.clone());
    if !node_kind.as_ref().is_some_and(is_input) {
        return Err(fail_hint(
            "manual_value_outside_inputs",
            format!(
                "{} is not an input; values are typed only in the Inputs area",
                graph.name(id)
            ),
            "Add a Number, Slider, Text or Boolean input, set it, and link it",
        ));
    }
    let wanted = port.to_lowercase();
    let Some(field) = node_ports(graph, drawing, id)
        .into_iter()
        .find(|field| field.input && field.name.to_lowercase() == wanted)
    else {
        return Err(fail(
            "unknown_port",
            format!("{} has no value {port}", graph.name(id)),
        ));
    };
    let invalid = || {
        fail(
            "invalid_value",
            format!(
                "{}.{} expects {}",
                graph.name(id),
                field.name,
                field.kind.name()
            ),
        )
    };
    let value = match field.kind {
        Expect::Number => match value {
            Value::Number(_) => value.clone(),
            Value::String(text) => text
                .trim()
                .parse::<f64>()
                .map(graph::value::number)
                .map_err(|_| invalid())?,
            _ => return Err(invalid()),
        },
        Expect::Bool => match value {
            Value::Bool(_) => value.clone(),
            _ => return Err(invalid()),
        },
        Expect::Text => match value {
            Value::String(_) => value.clone(),
            Value::Number(_) | Value::Bool(_) => Value::String(graph::value::text(value)),
            _ => return Err(invalid()),
        },
        _ => value.clone(),
    };
    if let Some(node) = graph.engine.node_mut(id) {
        node.params.insert(field.name, value);
    }
    Ok(())
}

fn text_of<'a>(change: &'a Value, key: &str) -> Result<&'a str, ChangeError> {
    change[key]
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| fail("invalid_change", format!("Supply {key}")))
}

fn apply_one(
    graph: &mut Graph,
    drawing: &dyn Drawing,
    index: usize,
    change: &Value,
    report: &mut Report,
) -> Result<(), ChangeError> {
    let Some(object) = change.as_object() else {
        return Err(fail("invalid_change", "Each change is an object"));
    };
    // `add` and `adopt` take `group` as an option, so they win over it.
    let verbs: Vec<&str> = match ["add", "adopt"]
        .iter()
        .find(|verb| object.contains_key(**verb))
    {
        Some(verb) if !(object.contains_key("add") && object.contains_key("adopt")) => vec![*verb],
        _ => VERBS
            .iter()
            .copied()
            .filter(|verb| object.contains_key(*verb))
            .collect(),
    };
    let [verb] = verbs[..] else {
        return Err(fail_hint(
            "invalid_change",
            "A change names exactly one action",
            &format!("Use one of: {}", VERBS.join(", ")),
        ));
    };
    let force = change["force"].as_bool() == Some(true);
    let mut result = json!({ "index": index, "change": verb });
    match verb {
        "add" | "adopt" => {
            let (item, handles) = if verb == "adopt" {
                let text = text_of(change, "adopt")?;
                let handle = u64::from_str_radix(text.trim_start_matches("0x"), 16)
                    .map(Handle::new)
                    .map_err(|_| fail("invalid_handle", "Expected a hexadecimal handle"))?;
                if graph.owned_handles().contains(&handle) {
                    return Err(fail(
                        "entity_owned",
                        format!("Entity {text} already belongs to a node"),
                    ));
                }
                (PaletteItem::Object(drawing.entity(handle)?), vec![handle])
            } else {
                let ty = &change["add"];
                if ty.get("script").is_some() {
                    return Err(fail_hint(
                        "not_supported",
                        "Script nodes are not available yet",
                        "Use node types from graph_library",
                    ));
                }
                if let Some(wanted) = ty.get("composite") {
                    let name = wanted.as_str().unwrap_or_default();
                    let library = crate::ui::node_graph::composite_library();
                    let k = library
                        .iter()
                        .position(|composite| {
                            composite.name.to_lowercase() == name.trim().to_lowercase()
                        })
                        .ok_or_else(|| {
                            fail_hint(
                                "unknown_type",
                                format!("The composite library has no {name}"),
                                "Read graph_library with search: composite",
                            )
                        })?;
                    (PaletteItem::Composite(k), Vec::new())
                } else {
                    let name = ty
                        .as_str()
                        .ok_or_else(|| fail("invalid_change", "add names a node type"))?;
                    (resolve_type(name, change["source"].as_str())?, Vec::new())
                }
            };
            let kind = item
                .kind()
                .ok_or_else(|| fail("unknown_type", "That library entry is gone"))?;
            let wanted = change["name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty());
            let name = match wanted {
                Some(wanted) => {
                    let name = graph.unique_name(wanted, None, false);
                    if name != wanted {
                        report.warn(
                            index,
                            "name_taken",
                            format!("{wanted} was taken; named {name}"),
                        );
                    }
                    name
                }
                None => graph.unique_name(&kind_name(&kind), None, true),
            };
            let id = graph.add_named(kind, name.clone(), handles);
            if let Some(values) = change.get("values") {
                let Some(values) = values.as_object() else {
                    return Err(fail("invalid_change", "values maps port names to values"));
                };
                for (port, value) in values {
                    set_value(graph, drawing, id, port, value)?;
                }
            }
            if let Some(after) = change.get("after") {
                let mut nodes = std::mem::take(&mut graph.engine.nodes);
                let placed = place(graph, &mut nodes, id, after);
                graph.engine.nodes = nodes;
                placed?;
            }
            if let Some(group) = change["group"].as_str().map(str::trim) {
                if !group.is_empty() {
                    join_group(graph, id, group, None);
                }
            }
            result["node"] = json!(name);
            result["id"] = json!(id);
        }
        "remove" => {
            let id = resolve_node(graph, &change["remove"])?;
            let links = graph.dependents(id);
            if !links.is_empty() && !force {
                return Err(ChangeError {
                    links: links.iter().map(|link| link_json(graph, link)).collect(),
                    ..fail_hint(
                        "would_break_links",
                        format!("Other nodes read from {}", graph.name(id)),
                        "Pass \"force\": true to remove it and clear those inputs",
                    )
                });
            }
            result["node"] = json!(graph.name(id));
            result["links_dropped"] = json!(links
                .iter()
                .map(|link| link_json(graph, link))
                .collect::<Vec<_>>());
            report.removed.extend(graph.remove_node(id));
        }
        "rename" => {
            let id = resolve_node(graph, &change["rename"])?;
            let wanted = text_of(change, "to")?;
            let name = graph.unique_name(wanted, Some(id), false);
            if name != wanted {
                report.warn(
                    index,
                    "name_taken",
                    format!("{wanted} was taken; named {name}"),
                );
            }
            if let Some(ui) = graph.ui.get_mut(&id) {
                ui.name = name.clone();
            }
            result["node"] = json!(name);
        }
        "set" => {
            let (port, _) = resolve_port(graph, drawing, &change["set"])?;
            let Some(value) = change.get("value") else {
                return Err(fail("invalid_change", "Supply value"));
            };
            set_value(graph, drawing, port.node, &port.field, value)?;
        }
        "link" => {
            let (from, source) = resolve_port(graph, drawing, &change["link"])?;
            let (to, target) = resolve_port(graph, drawing, &change["to"])?;
            if !source.output {
                return Err(fail(
                    "port_not_output",
                    format!("{}.{} gives no value", graph.name(from.node), from.field),
                ));
            }
            let target_input = graph
                .engine
                .node(to.node)
                .is_some_and(|node| is_input(&node.kind));
            if target_input {
                return Err(fail_hint(
                    "manual_value_outside_inputs",
                    format!(
                        "{} is an input; its values are typed, not linked",
                        graph.name(to.node)
                    ),
                    "Use set to change an input's value",
                ));
            }
            if !target.input {
                return Err(fail(
                    "port_not_input",
                    format!(
                        "{}.{} is computed and takes no value",
                        graph.name(to.node),
                        to.field
                    ),
                ));
            }
            let steps = graph.steps();
            let forward = match (steps.get(&from.node), steps.get(&to.node)) {
                (Some(from), Some(to)) => from < to,
                (None, Some(_)) => true,
                _ => false,
            };
            if !forward || !graph.engine.connect(from.clone(), to.clone()) {
                return Err(fail_hint(
                    "backward_link",
                    format!(
                        "{} comes after {}; values only flow forward",
                        graph.name(from.node),
                        graph.name(to.node)
                    ),
                    "Move the source earlier or the target later first",
                ));
            }
            if !target.kind.accepts(source.kind) {
                report.warn(
                    index,
                    "type_mismatch",
                    format!(
                        "{}.{} expects {} but {}.{} gives {}",
                        graph.name(to.node),
                        to.field,
                        target.kind.name(),
                        graph.name(from.node),
                        from.field,
                        source.kind.name()
                    ),
                );
            }
        }
        "unlink" => {
            let (port, _) = resolve_port(graph, drawing, &change["unlink"])?;
            if graph.engine.disconnect(&port).is_none() {
                report.warn(
                    index,
                    "not_linked",
                    format!("{}.{} was not linked", graph.name(port.node), port.field),
                );
            }
        }
        "move" => {
            let id = resolve_node(graph, &change["move"])?;
            let Some(after) = change.get("after") else {
                return Err(fail("invalid_change", "Supply after: \"start\" or a node"));
            };
            let mut nodes = graph.engine.nodes.clone();
            place(graph, &mut nodes, id, after)?;
            let order: Vec<(NodeId, &Kind)> =
                nodes.iter().map(|node| (node.id, &node.kind)).collect();
            let broken = graph.broken_links(&Graph::ranks(order.into_iter()));
            let links: Vec<Value> = broken.iter().map(|link| link_json(graph, link)).collect();
            if !broken.is_empty() && !force {
                return Err(ChangeError {
                    links,
                    ..fail_hint(
                        "would_break_links",
                        format!("Moving {} there breaks links", graph.name(id)),
                        "Pass \"force\": true to move it and clear those inputs",
                    )
                });
            }
            graph.engine.nodes = nodes;
            graph.prune_links();
            result["node"] = json!(graph.name(id));
            result["links_dropped"] = json!(links);
        }
        "group" => {
            let Some(members) = change["group"].as_array().filter(|list| !list.is_empty()) else {
                return Err(fail("invalid_change", "group lists the nodes to gather"));
            };
            let name = text_of(change, "name")?.to_owned();
            let ids = members
                .iter()
                .map(|member| resolve_node(graph, member))
                .collect::<Result<Vec<_>, _>>()?;
            let areas: HashSet<bool> = ids
                .iter()
                .filter_map(|id| graph.engine.node(*id))
                .map(|node| is_input(&node.kind))
                .collect();
            if areas.len() > 1 {
                return Err(fail(
                    "mixed_areas",
                    "A group holds inputs or steps, not both",
                ));
            }
            let color = change["color"].as_u64().map(|color| color as usize);
            for id in ids {
                join_group(graph, id, &name, color);
            }
            result["group"] = json!(name);
        }
        "ungroup" => {
            let wanted = text_of(change, "ungroup")?.to_lowercase();
            let Some(group) = graph
                .groups
                .iter()
                .find(|group| group.name.to_lowercase() == wanted)
                .map(|group| group.id)
            else {
                return Err(fail(
                    "unknown_group",
                    format!("No group is called {wanted}"),
                ));
            };
            for ui in graph.ui.values_mut() {
                if ui.group == Some(group) {
                    ui.group = None;
                }
            }
            graph.prune_groups();
        }
        "duplicate" => {
            let id = resolve_node(graph, &change["duplicate"])?;
            let copy = graph
                .duplicate(id)
                .ok_or_else(|| fail("unknown_node", "The node is gone"))?;
            if let Some(wanted) = change["name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                let name = graph.unique_name(wanted, Some(copy), false);
                if let Some(ui) = graph.ui.get_mut(&copy) {
                    ui.name = name;
                }
            }
            report.duplicated.push((id, copy));
            result["node"] = json!(graph.name(copy));
            result["id"] = json!(copy);
        }
        "clear" => {
            graph.engine = graph::Graph::default();
            graph.ui.clear();
            graph.groups.clear();
        }
        "composite" => {
            let Some(members) = change["composite"]
                .as_array()
                .filter(|list| !list.is_empty())
            else {
                return Err(fail(
                    "invalid_change",
                    "composite lists the steps to gather",
                ));
            };
            let name = text_of(change, "name")?.to_owned();
            let ids = members
                .iter()
                .map(|member| resolve_node(graph, member))
                .collect::<Result<Vec<_>, _>>()?;
            let id = graph.extract_members(&ids, &name).map_err(|error| {
                fail_hint(
                    "invalid_composite",
                    error,
                    "Gather operation steps only, with no other step both reading from and feeding them",
                )
            })?;
            result["node"] = json!(graph.name(id));
            result["id"] = json!(id);
            if let Some(Kind::Composite(composite)) = graph.engine.node(id).map(|node| &node.kind) {
                result["inputs"] = json!(composite
                    .inputs
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>());
                result["outputs"] = json!(composite
                    .outputs
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>());
            }
        }
        "expand" => {
            let id = resolve_node(graph, &change["expand"])?;
            let ids = graph
                .expand_node(id)
                .map_err(|error| fail("not_a_composite", error))?;
            result["nodes"] = json!(ids.iter().map(|id| graph.name(*id)).collect::<Vec<_>>());
        }
        "save_composite" => {
            let id = resolve_node(graph, &change["save_composite"])?;
            let Some(Kind::Composite(composite)) =
                graph.engine.node(id).map(|node| node.kind.clone())
            else {
                return Err(fail(
                    "not_a_composite",
                    format!("{} is not a composite", graph.name(id)),
                ));
            };
            let mut saved = (*composite).clone();
            saved.name = change["name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map_or_else(|| graph.name(id), str::to_owned);
            result["saved"] = json!(saved.name);
            report.saved.push(std::sync::Arc::new(saved));
        }
        _ => unreachable!("verbs are listed above"),
    }
    report.results.push(result);
    Ok(())
}

/// Applies `changes` all-or-nothing. On success the graph holds the result
/// and the snapshot taken before is returned for the undo history; on
/// failure the graph is as it was.
pub(crate) fn apply(
    graph: &mut Graph,
    drawing: &dyn Drawing,
    changes: &[Value],
) -> Result<(Report, crate::ui::node_graph::Snapshot), ChangeError> {
    if changes.is_empty() || changes.len() > MAX_CHANGES {
        return Err(fail(
            "invalid_change",
            format!("Send between 1 and {MAX_CHANGES} changes"),
        ));
    }
    let before = graph.snapshot();
    let mut report = Report::default();
    for (index, change) in changes.iter().enumerate() {
        if let Err(mut error) = apply_one(graph, drawing, index, change, &mut report) {
            error.index = index;
            graph.restore_content(before);
            return Err(error);
        }
    }
    Ok((report, before))
}

/// One node as `graph_state` reports it.
fn node_json(
    graph: &Graph,
    drawing: &dyn Drawing,
    node: &graph::Node,
    sections: &[NodeSection],
    position: usize,
    summary: bool,
) -> Value {
    let ui = graph.ui.get(&node.id);
    let group = ui
        .and_then(|ui| ui.group)
        .and_then(|group| graph.groups.iter().find(|known| known.id == group))
        .map(|group| group.name.clone());
    let source = match &node.kind {
        Kind::Op(_) => "builtin",
        Kind::Object(_) => "object",
        Kind::Composite(_) => "composite",
    };
    let mut value = json!({
        "id": node.id,
        "name": graph.name(node.id),
        "type": match &node.kind {
            Kind::Op(spec) => spec.name.to_owned(),
            Kind::Object(name) => name.clone(),
            Kind::Composite(composite) => composite.name.clone(),
        },
        "source": source,
        "position": position,
        "group": group,
    });
    if let Some(ui) = ui.filter(|ui| !ui.handles.is_empty()) {
        value["entities"] = json!(ui
            .handles
            .iter()
            .map(|handle| format!("{:X}", handle.value()))
            .collect::<Vec<_>>());
    }
    if is_input(&node.kind) {
        value["values"] = json!(node.params);
    }
    if summary {
        return value;
    }
    let kinds: HashMap<String, Expect> = node_ports(graph, drawing, node.id)
        .into_iter()
        .map(|field| (field.name, field.kind))
        .collect();
    let ports: Vec<Value> = sections
        .iter()
        .flat_map(|section| &section.rows)
        .map(|row| {
            let port = Port {
                node: node.id,
                field: row.field.clone(),
            };
            let direction = match (row.input, row.output) {
                (true, true) => "inout",
                (true, false) => "in",
                _ => "out",
            };
            let mut entry = json!({
                "name": row.field,
                "label": crate::t!(row.label.as_str()).into_owned(),
                "direction": direction,
                "value": row.value,
            });
            if !row.input {
                entry["computed"] = json!(true);
            }
            let kind = kinds
                .get(&row.field)
                .copied()
                .unwrap_or_else(|| graph.expect(&port, &row.value));
            entry["kind"] = json!(kind.name());
            if let Some(link) = graph.engine.links.iter().find(|link| link.to == port) {
                entry["source"] = json!(format!(
                    "{}.{}",
                    graph.name(link.from.node),
                    link.from.field
                ));
            }
            entry
        })
        .collect();
    value["ports"] = json!(ports);
    value
}

impl OpenCADStudio {
    /// Every node type: library operations and drawable objects, with their
    /// ports. `name` picks one type; `search` filters by name or category.
    pub(super) fn control_graph_library(&self, req: &Value) -> Value {
        let i = self.active_tab;
        let drawing = AppDrawing::new(self, i);
        let search = req["search"].as_str().map(str::to_lowercase);
        let only = req["name"].as_str().map(str::to_lowercase);
        let wanted = |name: &str, category: &str| {
            only.as_ref()
                .is_none_or(|only| &name.to_lowercase() == only)
                && search.as_ref().is_none_or(|search| {
                    name.to_lowercase().contains(search) || category.to_lowercase().contains(search)
                })
        };
        let port = |field: &Field| json!({ "name": field.name, "label": field.label, "kind": field.kind.name() });
        let mut types = Vec::new();
        for kind in ObjectKind::ALL {
            if !wanted(kind.name(), "Objects") {
                continue;
            }
            let fields = drawing.fields(*kind);
            types.push(json!({
                "type": kind.name(),
                "source": "object",
                "category": "Objects",
                "area": "sequence",
                "inputs": fields.iter().filter(|field| field.input).map(port).collect::<Vec<_>>(),
                "outputs": fields.iter().filter(|field| field.output).map(|field| {
                    let mut value = port(field);
                    if !field.input {
                        value["computed"] = json!(true);
                    }
                    value
                }).collect::<Vec<_>>(),
            }));
        }
        let mut probe = Graph::default();
        for spec in graph::LIBRARY {
            if !wanted(spec.name, spec.category) {
                continue;
            }
            let id = probe.add_node(PaletteItem::Op(spec), Vec::new());
            let fields = node_ports(&probe, &drawing, id);
            let inputs: Vec<Value> = spec
                .inputs
                .iter()
                .zip(&fields)
                .map(|(input, field)| {
                    let mut value = port(field);
                    let default = input.default.value();
                    if !default.is_null() {
                        value["default"] = default;
                    }
                    if input.list {
                        value["list"] = json!(true);
                    }
                    value
                })
                .collect();
            types.push(json!({
                "type": spec.name,
                "source": "builtin",
                "category": spec.category,
                "area": if is_input(&Kind::Op(spec)) { "inputs" } else { "sequence" },
                "inputs": inputs,
                "outputs": fields.iter().filter(|field| field.output).map(port).collect::<Vec<_>>(),
            }));
        }
        for composite in crate::ui::node_graph::composite_library() {
            if !wanted(&composite.name, "Composites") && !wanted("composite", "Composites") {
                continue;
            }
            let fields = composite_ports(&composite);
            types.push(json!({
                "type": composite.name,
                "source": "composite",
                "category": "Composites",
                "area": "sequence",
                "add": { "add": { "composite": composite.name } },
                "inputs": fields.iter().filter(|field| field.input).map(port).collect::<Vec<_>>(),
                "outputs": fields.iter().filter(|field| field.output).map(port).collect::<Vec<_>>(),
            }));
        }
        json!({
            "ok": true,
            "status": "completed",
            "count": types.len(),
            "types": types,
            "sources": ["builtin", "object", "composite"],
            "reserved_sources": ["script"],
        })
    }

    /// The graph: inputs, the node sequence, groups and revision.
    /// `detail: "summary"` leaves out the ports.
    pub(super) fn control_graph_state(&self, req: &Value) -> Value {
        let i = self.active_tab;
        let graph = &self.tabs[i].graph;
        let summary = req["detail"].as_str() == Some("summary");
        let sections = self.graph_all_sections(i);
        let drawing = AppDrawing::new(self, i);
        let (mut inputs, mut sequence) = (Vec::new(), Vec::new());
        for (node, node_sections) in graph.engine.nodes.iter().zip(&sections) {
            let list = if is_input(&node.kind) {
                &mut inputs
            } else {
                &mut sequence
            };
            let position = list.len() + 1;
            list.push(node_json(
                graph,
                &drawing,
                node,
                node_sections,
                position,
                summary,
            ));
        }
        let groups: Vec<Value> = graph
            .groups
            .iter()
            .map(|group| {
                json!({
                    "name": group.name,
                    "color": group.color,
                    "collapsed": group.collapsed,
                    "area": if group.inputs { "inputs" } else { "sequence" },
                })
            })
            .collect();
        json!({
            "ok": true,
            "status": "completed",
            "graph_revision": graph.revision,
            "inputs": inputs,
            "sequence": sequence,
            "groups": groups,
            "steps_hidden": graph.steps_hidden,
        })
    }

    /// Dry run of a `graph_apply` request against the live graph.
    pub(super) fn control_graph_validate(&mut self, req: &Value) -> Value {
        let request = &req["request"];
        let Some(changes) = request["changes"].as_array() else {
            return json!({"ok":false,"status":"failed","code":"invalid_request","error":"Supply request: a graph_apply request with changes"});
        };
        let i = self.active_tab;
        let mut graph = std::mem::take(&mut self.tabs[i].graph);
        let outcome = {
            let drawing = AppDrawing::new(self, i);
            let links_before: Vec<Link> = graph.engine.links.clone();
            match apply(&mut graph, &drawing, changes) {
                Ok((report, before)) => {
                    let dropped: Vec<Value> = links_before
                        .iter()
                        .filter(|link| {
                            !graph
                                .engine
                                .links
                                .iter()
                                .any(|now| now.from == link.from && now.to == link.to)
                        })
                        .map(|link| {
                            json!({
                                "from": format!("{}.{}", name_in(&before, link.from.node), link.from.field),
                                "to": format!("{}.{}", name_in(&before, link.to.node), link.to.field),
                            })
                        })
                        .collect();
                    graph.restore_content(before);
                    json!({
                        "ok": true,
                        "status": "completed",
                        "valid": true,
                        "results": report.results,
                        "warnings": report.warnings,
                        "links_dropped": dropped,
                    })
                }
                Err(error) => {
                    let mut value = error.to_json();
                    value["ok"] = json!(true);
                    value["status"] = json!("completed");
                    value["valid"] = json!(false);
                    value
                }
            }
        };
        self.tabs[i].graph = graph;
        outcome
    }

    /// `graph_apply`: the changes as one graph undo step, then one
    /// evaluation.
    pub(super) fn control_graph_apply(&mut self, req: &Value) -> Result<Task<Message>, Value> {
        let i = self.active_tab;
        if self.tabs[i].is_start {
            return Err(
                json!({"ok":false,"status":"failed","code":"no_document","error":"Create or open a drawing first"}),
            );
        }
        if let Some(expected) = req["graph_revision"].as_u64() {
            if expected != self.tabs[i].graph.revision {
                return Err(json!({
                    "ok": false,
                    "status": "failed",
                    "code": "stale_graph",
                    "error": "The graph changed; read graph_state again",
                    "graph_revision": self.tabs[i].graph.revision,
                }));
            }
        }
        let Some(changes) = req["changes"].as_array() else {
            return Err(
                json!({"ok":false,"status":"failed","code":"invalid_request","error":"Supply changes"}),
            );
        };
        let mut graph = std::mem::take(&mut self.tabs[i].graph);
        let outcome = {
            let drawing = AppDrawing::new(self, i);
            apply(&mut graph, &drawing, changes)
        };
        self.tabs[i].graph = graph;
        let (report, before) = outcome.map_err(|error| error.to_json())?;
        self.tabs[i].graph.push_undo(before);
        for (original, copy) in &report.duplicated {
            self.graph_copy_entity(i, *original, *copy);
        }
        for composite in &report.saved {
            crate::ui::node_graph::save_to_library(composite.clone());
        }
        self.command_line.push_info(
            crate::tf!(
                "Node graph changed by an automation client ({} changes)",
                changes.len()
            )
            .as_ref(),
        );
        let removed = report.removed.clone();
        let task = self.graph_step(i, |app| {
            app.graph_erase(i, removed);
            Vec::new()
        });
        let graph = &self.tabs[i].graph;
        let nodes: Vec<Value> = graph
            .engine
            .nodes
            .iter()
            .map(|node| json!({ "id": node.id, "name": graph.name(node.id) }))
            .collect();
        self.set_control_result(json!({
            "changes": report.results,
            "warnings": report.warnings,
            "graph_revision": graph.revision,
            "nodes": nodes,
        }));
        Ok(task)
    }
}

fn name_in(snapshot: &crate::ui::node_graph::Snapshot, id: NodeId) -> String {
    snapshot.name(id)
}

/// The drawing of tab `i`, as applying changes sees it. Object ports are
/// read once per kind.
struct AppDrawing<'a> {
    app: &'a OpenCADStudio,
    i: usize,
    fields: RefCell<HashMap<ObjectKind, Vec<Field>>>,
}

impl<'a> AppDrawing<'a> {
    fn new(app: &'a OpenCADStudio, i: usize) -> Self {
        Self {
            app,
            i,
            fields: RefCell::default(),
        }
    }
}

impl Drawing for AppDrawing<'_> {
    fn fields(&self, kind: ObjectKind) -> Vec<Field> {
        self.fields
            .borrow_mut()
            .entry(kind)
            .or_insert_with(|| self.app.graph_kind_ports(self.i, kind))
            .clone()
    }

    fn entity(&self, handle: Handle) -> Result<ObjectKind, ChangeError> {
        let entity = self.app.tabs[self.i]
            .scene
            .document
            .get_entity(handle)
            .ok_or_else(|| {
                fail(
                    "entity_absent",
                    format!("Entity {:X} does not exist", handle.value()),
                )
            })?;
        ObjectKind::of(entity).ok_or_else(|| {
            fail(
                "unsupported_entity",
                format!("No node can hold entity {:X}", handle.value()),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> OpenCADStudio {
        let mut app = OpenCADStudio::new_for_test();
        app.automation_op(r#"{"op":"new"}"#);
        app
    }

    fn read(app: &mut OpenCADStudio, op: &str, extra: Value) -> Value {
        let mut request = json!({ "protocol": 1, "op": op });
        if let (Some(request), Some(extra)) = (request.as_object_mut(), extra.as_object()) {
            request.extend(extra.clone());
        }
        app.automation_op(&request.to_string())
    }

    fn apply_changes(app: &mut OpenCADStudio, id: &str, changes: Value) -> Value {
        let state = app.automation_op(r#"{"protocol":1,"op":"state"}"#);
        let request = json!({
            "protocol": 1,
            "op": "graph_apply",
            "request_id": id,
            "document_id": state["document_id"],
            "changes": changes,
        });
        app.automation_op(&request.to_string())
    }

    fn node<'a>(state: &'a Value, area: &str, name: &str) -> &'a Value {
        state[area]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["name"] == name)
            .unwrap_or_else(|| panic!("{name} in {area}: {state}"))
    }

    /// A Line's End X port, checked to be a settable number in the library.
    fn line_number_port(app: &mut OpenCADStudio) -> String {
        let library = read(app, "graph_library", json!({ "name": "Line" }));
        let port = library["types"][0]["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|port| port["name"] == "end_x")
            .cloned()
            .expect("a Line has End X");
        assert_eq!(port["kind"], "number");
        "end_x".to_owned()
    }

    #[test]
    fn the_library_lists_objects_and_operations_with_their_ports() {
        let mut app = app();
        let library = read(&mut app, "graph_library", json!({}));
        assert_eq!(library["ok"], true, "{library}");
        let types = library["types"].as_array().unwrap();
        let add = types.iter().find(|ty| ty["type"] == "Add").unwrap();
        assert_eq!(add["source"], "builtin");
        assert_eq!(add["area"], "sequence");
        assert_eq!(add["inputs"][0]["kind"], "number");
        let number = types.iter().find(|ty| ty["type"] == "Number").unwrap();
        assert_eq!(number["area"], "inputs");
        let line = types.iter().find(|ty| ty["type"] == "Line").unwrap();
        assert_eq!(line["source"], "object");
        assert!(!line["inputs"].as_array().unwrap().is_empty());
        assert!(line["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|port| port["computed"] == true));
        assert_eq!(library["reserved_sources"], json!(["script"]));
    }

    #[test]
    fn an_apply_builds_links_and_evaluates_into_the_drawing() {
        let mut app = app();
        let field = line_number_port(&mut app);
        let result = apply_changes(
            &mut app,
            "g1",
            json!([
                {"add": "Number", "name": "Width", "values": {"value": 7}},
                {"add": "Line", "name": "Axis", "group": "Wall"},
                {"link": "Width.value", "to": format!("Axis.{field}")},
            ]),
        );
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(result["result"]["changes"][1]["node"], "Axis");

        let state = read(&mut app, "graph_state", json!({}));
        let width = node(&state, "inputs", "Width");
        assert_eq!(width["values"]["value"], 7);
        let axis = node(&state, "sequence", "Axis");
        assert_eq!(axis["group"], "Wall");
        assert_eq!(axis["entities"].as_array().map(Vec::len), Some(1));
        let port = axis["ports"]
            .as_array()
            .unwrap()
            .iter()
            .find(|port| port["name"] == field.as_str())
            .unwrap();
        assert_eq!(port["source"], "Width.value");
        assert_eq!(graph::value::num(&port["value"]), Some(7.0));
        assert_eq!(state["groups"][0]["name"], "Wall");
    }

    #[test]
    fn the_canvas_rules_hold_and_a_failed_request_changes_nothing() {
        let mut app = app();
        let first = apply_changes(
            &mut app,
            "r1",
            json!([
                {"add": "Number", "name": "N"},
                {"add": "Add", "name": "Sum"},
                {"add": "Negate", "name": "Neg"},
                {"link": "Sum.result", "to": "Neg.x"},
            ]),
        );
        assert_eq!(first["ok"], true, "{first}");
        let revision = read(&mut app, "graph_state", json!({}))["graph_revision"].clone();

        let refused = |app: &mut OpenCADStudio, id: &str, changes: Value| {
            let response = apply_changes(app, id, changes);
            assert_eq!(response["ok"], false, "{response}");
            response
        };
        let typed = refused(&mut app, "r2", json!([{"set": "Sum.a", "value": 1}]));
        assert_eq!(typed["code"], "manual_value_outside_inputs");
        let backward = refused(
            &mut app,
            "r3",
            json!([{"link": "Neg.result", "to": "Sum.a"}]),
        );
        assert_eq!(backward["code"], "backward_link");
        let used = refused(&mut app, "r4", json!([{"remove": "Sum"}]));
        assert_eq!(used["code"], "would_break_links");
        assert_eq!(used["links"][0]["to"], "Neg.x");
        let moved = refused(&mut app, "r5", json!([{"move": "Neg", "after": "start"}]));
        assert_eq!(moved["code"], "would_break_links");
        let ambiguous = refused(&mut app, "r6", json!([{"add": "Point"}]));
        assert_eq!(ambiguous["code"], "ambiguous_type");
        let script = refused(&mut app, "r7", json!([{"add": {"script": {}}}]));
        assert_eq!(script["code"], "not_supported");
        // The first change is fine; the second fails, so neither lands.
        let partial = refused(
            &mut app,
            "r8",
            json!([{"add": "Number", "name": "Extra"}, {"add": "Nope"}]),
        );
        assert_eq!(partial["code"], "unknown_type");
        assert_eq!(partial["index"], 1);

        let state = read(&mut app, "graph_state", json!({"detail": "summary"}));
        assert_eq!(
            state["graph_revision"], revision,
            "refusals changed nothing"
        );
        assert_eq!(state["inputs"].as_array().map(Vec::len), Some(1));

        let forced = apply_changes(&mut app, "r9", json!([{"remove": "Sum", "force": true}]));
        assert_eq!(forced["ok"], true, "{forced}");
        assert_eq!(
            forced["result"]["changes"][0]["links_dropped"][0]["to"],
            "Neg.x"
        );
    }

    #[test]
    fn a_stale_revision_is_refused() {
        let mut app = app();
        let state = app.automation_op(r#"{"protocol":1,"op":"state"}"#);
        let request = json!({
            "protocol": 1,
            "op": "graph_apply",
            "request_id": "s1",
            "document_id": state["document_id"],
            "graph_revision": 99,
            "changes": [{"add": "Number"}],
        });
        let response = app.automation_op(&request.to_string());
        assert_eq!(response["code"], "stale_graph", "{response}");
    }

    #[test]
    fn validate_reports_without_changing_the_graph() {
        let mut app = app();
        let ok = read(
            &mut app,
            "graph_validate",
            json!({"request": {"changes": [{"add": "Number", "name": "A"}]}}),
        );
        assert_eq!(ok["valid"], true, "{ok}");
        let bad = read(
            &mut app,
            "graph_validate",
            json!({"request": {"changes": [{"set": "Nobody.value", "value": 1}]}}),
        );
        assert_eq!(bad["valid"], false);
        assert_eq!(bad["code"], "unknown_node");
        let state = read(&mut app, "graph_state", json!({}));
        assert_eq!(state["inputs"], json!([]));
        assert_eq!(state["graph_revision"], 0);
    }

    #[test]
    fn drawn_objects_are_adopted_once() {
        let mut app = app();
        app.automation_op(r#"{"op":"run","cmd":"LINE 0,0 10,0"}"#);
        let query = app.automation_op(r#"{"op":"query","type":"Line","detail":"summary"}"#);
        let handle = query["entities"][0]["handle"].as_str().unwrap().to_owned();
        let adopted = apply_changes(&mut app, "a1", json!([{"adopt": handle, "name": "Drawn"}]));
        assert_eq!(adopted["ok"], true, "{adopted}");
        let state = read(&mut app, "graph_state", json!({"detail": "summary"}));
        assert_eq!(
            node(&state, "sequence", "Drawn")["entities"][0]
                .as_str()
                .map(str::to_lowercase),
            Some(handle.trim_start_matches("0x").to_lowercase())
        );
        let again = apply_changes(&mut app, "a2", json!([{"adopt": handle}]));
        assert_eq!(again["code"], "entity_owned", "{again}");
    }

    #[test]
    fn an_apply_is_one_graph_undo_step() {
        let mut app = app();
        apply_changes(
            &mut app,
            "u1",
            json!([{"add": "Number", "name": "A"}, {"add": "Number", "name": "B"}]),
        );
        let _ = app.on_graph(crate::ui::node_graph::GraphMsg::Undo);
        let state = read(&mut app, "graph_state", json!({}));
        assert_eq!(state["inputs"], json!([]));
    }

    fn count_lines(app: &mut OpenCADStudio, kind: &str) -> usize {
        let query = app.automation_op(&format!(
            r#"{{"op":"query","type":"{kind}","detail":"summary"}}"#
        ));
        query["count"].as_u64().unwrap_or(0) as usize
    }

    #[test]
    fn curves_flow_as_values_and_only_the_draw_node_draws() {
        let mut app = app();
        let built = apply_changes(
            &mut app,
            "c1",
            json!([
                {"add": "Number", "name": "Length", "values": {"value": 10}},
                {"add": "Number", "name": "Gap", "values": {"value": 2}},
                {"add": "Point", "source": "builtin", "name": "Start"},
                {"add": "Point", "source": "builtin", "name": "End"},
                {"link": "Length.value", "to": "End.x"},
                {"add": "Line Curve", "name": "Axis"},
                {"link": "Start.point", "to": "Axis.start"},
                {"link": "End.point", "to": "Axis.end"},
                {"add": "Offset Curve", "name": "Face"},
                {"link": "Axis.curve", "to": "Face.curve"},
                {"link": "Gap.value", "to": "Face.distance"},
                {"add": "Draw Geometry", "name": "Wall"},
                {"link": "Face.curve", "to": "Wall.geometry"},
            ]),
        );
        assert_eq!(built["ok"], true, "{built}");
        assert_eq!(
            built["result"]["warnings"],
            json!([]),
            "no type warnings: {built}"
        );
        // Only the Draw node put anything in the drawing: one line.
        assert_eq!(count_lines(&mut app, "Line"), 1);
        let state = read(&mut app, "graph_state", json!({}));
        let face = node(&state, "sequence", "Face");
        let curve = face["ports"]
            .as_array()
            .unwrap()
            .iter()
            .find(|port| port["name"] == "curve")
            .unwrap();
        assert_eq!(curve["kind"], "curve");
        assert_eq!(curve["value"]["start"], json!([0.0, 2.0]));
        let wall = node(&state, "sequence", "Wall");
        let handle = wall["entities"][0].clone();

        // A new value moves the same entity.
        let set = apply_changes(
            &mut app,
            "c2",
            json!([{"set": "Length.value", "value": 25}]),
        );
        assert_eq!(set["ok"], true, "{set}");
        let state = read(&mut app, "graph_state", json!({}));
        assert_eq!(node(&state, "sequence", "Wall")["entities"][0], handle);
        let query = app.automation_op(r#"{"op":"query","type":"Line","detail":"geometry"}"#);
        assert_eq!(count_lines(&mut app, "Line"), 1);
        let line = &query["entities"][0];
        assert!(line.to_string().contains("25"), "{line}");

        // The preview shows the computed curves while the graph is open.
        app.show_node_graph = true;
        app.graph_refresh_preview();
        let i = app.active_tab;
        assert_eq!(
            app.tabs[i].scene.graph_preview_wires.len(),
            2,
            "axis and face"
        );
        app.show_node_graph = false;
        app.graph_refresh_preview();
        assert!(app.tabs[i].scene.graph_preview_wires.is_empty());

        // Unlinking the geometry erases what the node drew.
        let cut = apply_changes(&mut app, "c3", json!([{"unlink": "Wall.geometry"}]));
        assert_eq!(cut["ok"], true, "{cut}");
        assert_eq!(count_lines(&mut app, "Line"), 0);
    }

    #[test]
    fn a_closed_polyline_offset_draws_a_polyline() {
        let mut app = app();
        let built = apply_changes(
            &mut app,
            "p1",
            json!([
                {"add": "Number", "name": "Size", "values": {"value": 10}},
                {"add": "Point", "source": "builtin", "name": "A"},
                {"add": "Point", "source": "builtin", "name": "B"},
                {"add": "Point", "source": "builtin", "name": "C"},
                {"link": "Size.value", "to": "B.x"},
                {"link": "Size.value", "to": "C.x"},
                {"link": "Size.value", "to": "C.y"},
                {"add": "Create List", "name": "Corners"},
            ]),
        );
        assert_eq!(built["ok"], true, "{built}");
        let library = read(&mut app, "graph_library", json!({"name": "Create List"}));
        let ports: Vec<String> = library["types"][0]["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|port| port["name"].as_str().unwrap().to_owned())
            .collect();
        let mut changes = vec![];
        for (port, point) in ports.iter().zip(["A", "B", "C"]) {
            changes
                .push(json!({"link": format!("{point}.point"), "to": format!("Corners.{port}")}));
        }
        changes.extend([
            json!({"add": "Polyline Curve", "name": "Outline"}),
            json!({"link": "Corners.list", "to": "Outline.points"}),
            json!({"add": "Draw Geometry", "name": "Drawn"}),
            json!({"link": "Outline.curve", "to": "Drawn.geometry"}),
        ]);
        let drawn = apply_changes(&mut app, "p2", Value::Array(changes));
        assert_eq!(drawn["ok"], true, "{drawn}");
        assert_eq!(count_lines(&mut app, "LwPolyline"), 1, "{drawn}");
    }

    #[test]
    fn transforms_and_arrays_draw_through_one_draw_node() {
        let mut app = app();
        let built = apply_changes(
            &mut app,
            "t1",
            json!([
                {"add": "Number", "name": "Step", "values": {"value": 15}},
                {"add": "Rectangle", "name": "Room"},
                {"add": "Fillet Corners", "name": "Rounded"},
                {"link": "Room.curve", "to": "Rounded.curve"},
                {"add": "Vector", "name": "Offset"},
                {"link": "Step.value", "to": "Offset.x"},
                {"add": "Linear Array", "name": "Rooms"},
                {"link": "Rounded.curve", "to": "Rooms.geometry"},
                {"link": "Offset.vector", "to": "Rooms.vector"},
                {"add": "Draw Geometry", "name": "Plan"},
                {"link": "Rooms.geometry", "to": "Plan.geometry"},
                {"add": "Rotate", "name": "Turned"},
                {"link": "Room.curve", "to": "Turned.geometry"},
                {"add": "Draw Geometry", "name": "Turned plan"},
                {"link": "Turned.geometry", "to": "Turned plan.geometry"},
            ]),
        );
        assert_eq!(built["ok"], true, "{built}");
        assert_eq!(built["result"]["warnings"], json!([]), "{built}");
        // Three arrayed rooms and the turned one, all drawn polylines.
        assert_eq!(count_lines(&mut app, "LwPolyline"), 4, "{built}");
        let state = read(&mut app, "graph_state", json!({}));
        assert_eq!(
            node(&state, "sequence", "Plan")["entities"]
                .as_array()
                .map(Vec::len),
            Some(3)
        );

        let library = read(&mut app, "graph_library", json!({"search": "Transform"}));
        let names: Vec<&str> = library["types"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|ty| ty["type"].as_str())
            .collect();
        for name in [
            "Move",
            "Rotate",
            "Scale",
            "Mirror",
            "Orient",
            "Linear Array",
            "Polar Array",
            "Curve Array",
        ] {
            assert!(names.contains(&name), "{name} in {names:?}");
        }
    }

    fn output_of(state: &Value, name: &str, port: &str) -> Value {
        node(state, "sequence", name)["ports"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == port)
            .map(|entry| entry["value"].clone())
            .unwrap_or(Value::Null)
    }

    /// W=4, One=1, Two=2 → Sum(W+One) → Twice(Sum×Two) → Flip(−Twice).
    fn arithmetic(app: &mut OpenCADStudio) {
        let built = apply_changes(
            app,
            "k0",
            json!([
                {"add": "Number", "name": "W", "values": {"value": 4}},
                {"add": "Number", "name": "One", "values": {"value": 1}},
                {"add": "Number", "name": "Two", "values": {"value": 2}},
                {"add": "Add", "name": "Sum"},
                {"link": "W.value", "to": "Sum.a"},
                {"link": "One.value", "to": "Sum.b"},
                {"add": "Multiply", "name": "Twice"},
                {"link": "Sum.result", "to": "Twice.a"},
                {"link": "Two.value", "to": "Twice.b"},
                {"add": "Negate", "name": "Flip"},
                {"link": "Twice.result", "to": "Flip.x"},
            ]),
        );
        assert_eq!(built["ok"], true, "{built}");
    }

    #[test]
    fn steps_become_a_composite_that_computes_the_same_and_expands_back() {
        let mut app = app();
        arithmetic(&mut app);
        let state = read(&mut app, "graph_state", json!({}));
        assert_eq!(
            graph::value::num(&output_of(&state, "Flip", "result")),
            Some(-10.0)
        );

        let made = apply_changes(
            &mut app,
            "k1",
            json!([{"composite": ["Sum", "Twice"], "name": "Grow"}]),
        );
        assert_eq!(made["ok"], true, "{made}");
        assert_eq!(
            made["result"]["changes"][0]["inputs"]
                .as_array()
                .map(Vec::len),
            Some(3)
        );
        let state = read(&mut app, "graph_state", json!({}));
        assert_eq!(
            state["sequence"].as_array().map(Vec::len),
            Some(2),
            "{state}"
        );
        assert_eq!(node(&state, "sequence", "Grow")["source"], "composite");
        assert_eq!(
            graph::value::num(&output_of(&state, "Flip", "result")),
            Some(-10.0)
        );

        let set = apply_changes(&mut app, "k2", json!([{"set": "W.value", "value": 9}]));
        assert_eq!(set["ok"], true, "{set}");
        let state = read(&mut app, "graph_state", json!({}));
        assert_eq!(
            graph::value::num(&output_of(&state, "Flip", "result")),
            Some(-20.0)
        );

        // Saved, and used again from the library.
        let saved = apply_changes(
            &mut app,
            "k3",
            json!([{"save_composite": "Grow", "name": "Grow test"}]),
        );
        assert_eq!(saved["ok"], true, "{saved}");
        let library = read(&mut app, "graph_library", json!({"name": "Grow test"}));
        assert_eq!(library["types"][0]["source"], "composite", "{library}");
        let again = apply_changes(
            &mut app,
            "k4",
            json!([{"add": {"composite": "Grow test"}, "name": "Grow 2"}]),
        );
        assert_eq!(again["ok"], true, "{again}");

        // Expanded back, the nodes keep their names and their result.
        let expanded = apply_changes(&mut app, "k5", json!([{"expand": "Grow"}]));
        assert_eq!(expanded["ok"], true, "{expanded}");
        let state = read(&mut app, "graph_state", json!({}));
        assert_eq!(node(&state, "sequence", "Sum")["group"], "Grow");
        assert_eq!(
            graph::value::num(&output_of(&state, "Flip", "result")),
            Some(-20.0)
        );

        // A saved file keeps composites.
        let i = app.active_tab;
        let (loaded, _) =
            crate::ui::node_graph::Graph::from_file(&app.tabs[i].graph.to_file("")).unwrap();
        assert!(loaded
            .engine
            .nodes
            .iter()
            .any(|node| matches!(node.kind, Kind::Composite(_))));
        crate::ui::node_graph::forget_library_entry(
            crate::ui::node_graph::composite_library()
                .iter()
                .position(|item| item.name == "Grow test")
                .unwrap(),
        );
    }

    #[test]
    fn composites_refuse_drawing_nodes_and_undo_as_one_step() {
        let mut app = app();
        arithmetic(&mut app);
        apply_changes(
            &mut app,
            "d0",
            json!([{"add": "Draw Geometry", "name": "Out"}]),
        );
        let refused = apply_changes(
            &mut app,
            "d1",
            json!([{"composite": ["Twice", "Out"], "name": "Bad"}]),
        );
        assert_eq!(refused["code"], "invalid_composite", "{refused}");

        // From a group in the canvas, then undone.
        let i = app.active_tab;
        let ids: Vec<NodeId> = ["Sum", "Twice"]
            .iter()
            .map(|name| {
                let graph = &app.tabs[i].graph;
                *graph.ui.iter().find(|(_, ui)| ui.name == *name).unwrap().0
            })
            .collect();
        let _ = app.on_graph(crate::ui::node_graph::GraphMsg::GroupNew(ids[0]));
        let group = app.tabs[i].graph.ui[&ids[0]].group.unwrap();
        let _ = app.on_graph(crate::ui::node_graph::GraphMsg::GroupAssign(
            ids[1],
            Some(group),
        ));
        let _ = app.on_graph(crate::ui::node_graph::GraphMsg::GroupToComposite(group));
        let state = read(&mut app, "graph_state", json!({}));
        assert_eq!(
            state["sequence"].as_array().map(Vec::len),
            Some(3),
            "{state}"
        );
        assert_eq!(
            graph::value::num(&output_of(&state, "Flip", "result")),
            Some(-10.0)
        );
        let _ = app.on_graph(crate::ui::node_graph::GraphMsg::Undo);
        let state = read(&mut app, "graph_state", json!({}));
        assert_eq!(state["sequence"].as_array().map(Vec::len), Some(4));
    }
}
