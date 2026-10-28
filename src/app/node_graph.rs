//! Node graph handling. An object node's rows are the Properties panel's own
//! page for its entity, and every write goes through the panel's handlers (via
//! the control protocol's `set_property_value`) aimed at that entity. The
//! `graph` crate evaluates; [`AppHost`] gives it the drawing.

use super::{Message, OpenCADStudio};
use crate::scene::model::object::PropSection;
use crate::ui::node_graph::{GraphMsg, NodeRow, NodeSection, ObjectKind, RowWidget};
use codec::Handle;
use graph::docs::{PortType, Type};
use graph::{Kind, NodeId, Port};
use iced::Task;
use rustc_hash::FxHashMap as HashMap;
use serde_json::{json, Value};

impl OpenCADStudio {
    /// The Properties panel's single-object page for `handle`.
    fn graph_panel_sections(&self, i: usize, handle: Handle) -> Vec<PropSection> {
        match self.tabs[i].scene.document.get_entity(handle) {
            Some(entity) => self.graph_entity_sections(i, handle, entity),
            None => Vec::new(),
        }
    }

    /// The Properties panel's page for `entity`, which need not be in the
    /// drawing (`handle` is then null).
    fn graph_entity_sections(
        &self,
        i: usize,
        handle: Handle,
        entity: &codec::EntityType,
    ) -> Vec<PropSection> {
        let tab = &self.tabs[i];
        crate::entities::common::set_unit_context(
            crate::entities::common::UnitContext::from_header(&tab.scene.document.header),
        );
        crate::entities::common::set_fixed_text_heights(&tab.scene.document);
        let text_style_names: Vec<String> = tab
            .scene
            .document
            .text_styles
            .iter()
            .map(|style| style.name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect();
        self.entity_property_sections(
            i,
            handle,
            entity,
            &text_style_names,
            tab.scene.displayed_annotation_scale_handle(),
            0,
        )
        .0
    }

    /// Every row of `handle`'s panel page as `(field, protocol value)`.
    fn graph_fields(&self, i: usize, handle: Handle) -> Vec<(String, Value)> {
        self.graph_panel_sections(i, handle)
            .iter()
            .flat_map(|section| &section.props)
            .map(|property| {
                let json = crate::app::control::property_json(property);
                (property.field.to_owned(), json["value"].clone())
            })
            .collect()
    }

    /// An object node's rows: its object handle(s), then the panel page of its
    /// first entity. With several entities the values shown are the lists.
    fn graph_object_sections(&self, i: usize, id: NodeId) -> Vec<NodeSection> {
        let graph = &self.tabs[i].graph;
        if graph
            .engine
            .node(id)
            .is_some_and(|node| is_placing(&node.kind))
        {
            return self.graph_draw_sections(i, id);
        }
        let Some(&first) = graph.ui.get(&id).and_then(|ui| ui.handles.first()) else {
            return Vec::new();
        };
        let outputs = graph.engine.node(id).map(|node| &node.outputs);
        let listed = graph.ui.get(&id).is_some_and(|ui| ui.handles.len() > 1);
        let value = |field: &str, own: Value| match outputs.and_then(|outputs| outputs.get(field)) {
            Some(value) if listed => value.clone(),
            _ => own,
        };
        let panel = self.graph_panel_sections(i, first);
        graph_rows(panel, json!({ "handle": first.value() }), value)
    }

    /// A placing node's rows: what it draws and how, and the entities it
    /// drew.
    fn graph_draw_sections(&self, i: usize, id: NodeId) -> Vec<NodeSection> {
        let graph = &self.tabs[i].graph;
        let Some(node) = graph.engine.node(id) else {
            return Vec::new();
        };
        let Kind::Object(name) = &node.kind else {
            return Vec::new();
        };
        let Some(kind) = ObjectKind::from_name(name) else {
            return Vec::new();
        };
        let first = graph.ui.get(&id).and_then(|ui| ui.handles.first().copied());
        let value = |field: &str| node.outputs.get(field).cloned().unwrap_or(Value::Null);
        let rows = place_fields(kind)
            .iter()
            .map(|&(field, label, input, kind, list)| NodeRow {
                field: field.to_owned(),
                label: crate::t!(label).into_owned(),
                value: match (field, first) {
                    ("object", Some(handle)) if value(field).is_null() => {
                        json!({ "handle": handle.value() })
                    }
                    _ => value(field),
                },
                input,
                output: true,
                widget: RowWidget::Field,
                kind: Some(PortType { kind, list }),
            })
            .collect();
        vec![NodeSection {
            title: String::new(),
            rows,
        }]
    }

    /// The ports of an object node of `kind`, with the value kinds the
    /// Properties panel gives its rows, for automation clients.
    pub(super) fn graph_kind_ports(
        &self,
        i: usize,
        kind: ObjectKind,
    ) -> Vec<crate::app::graph_ops::Field> {
        use crate::app::graph_ops::{property_kind, Field};
        use crate::ui::node_graph::Expect;
        if kind.places() {
            return place_fields(kind)
                .iter()
                .map(|&(name, label, input, kind, list)| Field {
                    name: name.to_owned(),
                    label: crate::t!(label).into_owned(),
                    input,
                    output: true,
                    kind: Expect::of_type(PortType { kind, list }),
                })
                .collect();
        }
        let panel = self.graph_entity_sections(i, Handle::NULL, &kind.entity());
        let object = Field {
            name: "object".to_owned(),
            label: crate::t!("Object").into_owned(),
            input: false,
            output: true,
            kind: crate::ui::node_graph::Expect::Object,
        };
        std::iter::once(object)
            .chain(
                panel
                    .iter()
                    .flat_map(|section| &section.props)
                    .map(|property| {
                        let json = crate::app::control::property_json(property);
                        let kind = json["kind"].as_str().unwrap_or_default();
                        Field {
                            name: property.field.to_owned(),
                            label: crate::t!(property.label.as_str()).into_owned(),
                            input: !matches!(kind, "readonly" | "specialized"),
                            output: true,
                            kind: property_kind(kind, &json["value"]),
                        }
                    }),
            )
            .collect()
    }
}

/// An object node's rows from its entity's panel page: the `object` handle
/// row, then every property, settable unless the panel shows it read-only.
/// `value` picks what a row shows from its field and the entity's own value.
fn graph_rows(
    panel: Vec<PropSection>,
    object: Value,
    value: impl Fn(&str, Value) -> Value,
) -> Vec<NodeSection> {
    if panel.is_empty() {
        return Vec::new();
    }
    let object = NodeRow {
        field: "object".to_owned(),
        label: crate::t!("Object").into_owned(),
        value: value("object", object),
        input: false,
        output: true,
        widget: RowWidget::Field,
        kind: Some(PortType {
            kind: Type::Object,
            list: false,
        }),
    };
    std::iter::once(NodeSection {
        title: String::new(),
        rows: vec![object],
    })
    .chain(panel.into_iter().map(|section| {
        NodeSection {
            title: section.title,
            rows: section
                .props
                .iter()
                .map(|property| {
                    let json = crate::app::control::property_json(property);
                    let kind = json["kind"].as_str().unwrap_or_default();
                    NodeRow {
                        field: property.field.to_owned(),
                        label: property.label.clone(),
                        input: !matches!(kind, "readonly" | "specialized"),
                        output: true,
                        kind: Some(PortType {
                            kind: crate::app::graph_ops::property_kind(kind, &json["value"])
                                .port_kind(),
                            list: false,
                        }),
                        value: value(property.field, json["value"].clone()),
                        widget: RowWidget::Field,
                    }
                })
                .collect(),
        }
    }))
    .collect()
}

impl OpenCADStudio {
    /// Every node's rows, parallel to the graph's node list.
    pub(super) fn graph_all_sections(&self, i: usize) -> Vec<Vec<NodeSection>> {
        let graph = &self.tabs[i].graph;
        let objects: HashMap<NodeId, Vec<NodeSection>> = graph
            .engine
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, Kind::Object(_)))
            .map(|node| (node.id, self.graph_object_sections(i, node.id)))
            .collect();
        graph.sections(objects)
    }

    /// Write one Properties row of `handle` through the panel's handlers.
    fn graph_write(
        &mut self,
        i: usize,
        handle: Handle,
        field: &str,
        value: &Value,
    ) -> Task<Message> {
        // An empty target falls back to the drawing defaults in some handlers.
        if self.tabs[i].scene.is_layer_locked(handle) {
            return Task::none();
        }
        let Some(property) = self
            .graph_panel_sections(i, handle)
            .into_iter()
            .flat_map(|section| section.props)
            .find(|property| property.field == field)
        else {
            return Task::none();
        };
        self.property_target_override = Some(handle);
        let result = self.set_property_value(property, value);
        self.property_target_override = None;
        result.unwrap_or_else(|error| {
            self.command_line
                .push_error(error["error"].as_str().unwrap_or_default());
            Task::none()
        })
    }

    /// Runs `edit`, then re-evaluates the graph, all as one undo step.
    pub(super) fn graph_step(
        &mut self,
        i: usize,
        edit: impl FnOnce(&mut Self) -> Vec<Task<Message>>,
    ) -> Task<Message> {
        self.push_undo_snapshot(i, "GRAPH");
        self.graph_undo_open = true;
        let mut tasks = edit(self);
        let mut engine = std::mem::take(&mut self.tabs[i].graph.engine);
        let mut host = AppHost {
            app: self,
            i,
            tasks: Vec::new(),
        };
        engine.evaluate(&mut host);
        tasks.append(&mut host.tasks);
        self.tabs[i].graph.engine = engine;
        self.graph_undo_open = false;
        self.graph_refresh_preview();
        Task::batch(tasks)
    }

    /// Shows the curves the active graph computes as a viewport overlay
    /// while the graph is open and its preview on; clears it elsewhere.
    pub(super) fn graph_refresh_preview(&mut self) {
        let (shown, active) = (self.show_node_graph, self.active_tab);
        for (k, tab) in self.tabs.iter_mut().enumerate() {
            tab.scene.graph_preview_wires = if shown && k == active && !tab.graph.preview_hidden {
                preview_wires(&tab.graph, &tab.scene.document)
            } else {
                Vec::new()
            };
        }
    }

    /// Draws a Draw node's curve values: one entity per curve (lists drawn
    /// whole), reusing the
    /// node's entities of the same kind, on the given layer and colour (the
    /// current layer and ByLayer when unset). Curves no entity can hold are
    /// skipped; entities left over are erased.
    fn graph_draw(
        &mut self,
        i: usize,
        node: NodeId,
        kind: ObjectKind,
        instances: &[Vec<(String, Value)>],
    ) -> Vec<Vec<(String, Value)>> {
        let mut spare: Vec<Handle> = self.tabs[i]
            .graph
            .ui
            .get(&node)
            .map(|ui| ui.handles.clone())
            .unwrap_or_default();
        spare.retain(|handle| self.tabs[i].scene.document.get_entity(*handle).is_some());
        let mut drawn = Vec::new();
        let mut results = Vec::new();
        for fields in instances {
            let get = |name: &str| {
                fields
                    .iter()
                    .find(|(field, _)| field == name)
                    .map(|(_, value)| value.clone())
            };
            // A layer or linetype named but not in the drawing yet is added,
            // so a graph can sort what it draws onto layers of its own.
            let layer = get("layer")
                .map(|value| graph::value::text(&value).trim().to_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| self.tabs[i].active_layer.clone());
            let linetype = get("linetype")
                .map(|value| graph::value::text(&value).trim().to_owned())
                .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case("ByLayer"));
            let line_weight = get("lineweight")
                .as_ref()
                .and_then(graph::value::num)
                .filter(|mm| *mm >= 0.0)
                .map_or(codec::types::LineWeight::ByLayer, |mm| {
                    codec::types::LineWeight::from_value((mm * 100.0).round().min(211.0) as i16)
                });
            self.tabs[i].scene.ensure_layer(&layer);
            let linetype = linetype.filter(|name| {
                let document = &mut self.tabs[i].scene.document;
                document.line_types.contains(name)
                    || crate::io::linetypes::ensure_catalog_linetype(document, name)
            });
            let document = &self.tabs[i].scene.document;
            let entities = place_entities(kind, &get, document);
            let color = match get("color") {
                Some(Value::Number(index)) => index
                    .as_i64()
                    .map_or(codec::types::Color::ByLayer, |index| {
                        codec::types::Color::from_index(index as i16)
                    }),
                _ => codec::types::Color::ByLayer,
            };
            let data = get("data")
                .map(|value| data_pairs(&graph::value::text(&value)))
                .unwrap_or_default();
            if !data.is_empty() {
                self.tabs[i].scene.ensure_app_id(DATA_APP);
            }
            let style = |entity: &mut codec::EntityType| {
                let common = entity.common_mut();
                common.layer = layer.clone();
                common.color = color;
                common.linetype = linetype.clone().unwrap_or_default();
                common.line_weight = line_weight;
                common.extended_data.remove_record(DATA_APP);
                if !data.is_empty() {
                    write_data(entity, &data);
                }
            };
            let mut objects = Vec::new();
            for mut entity in entities {
                let document = &self.tabs[i].scene.document;
                let reuse = spare.iter().position(|handle| {
                    document.get_entity(*handle).is_some_and(|existing| {
                        std::mem::discriminant(existing) == std::mem::discriminant(&entity)
                    })
                });
                let handle = match reuse {
                    Some(k) => {
                        let handle = spare.remove(k);
                        if let Some(existing) = self.tabs[i].scene.document.get_entity(handle) {
                            *entity.common_mut() = existing.common().clone();
                        }
                        style(&mut entity);
                        // Unchanged entities stay as they are: no redraw, no
                        // undo record.
                        if self.tabs[i].scene.document.get_entity(handle) != Some(&entity) {
                            self.tabs[i].scene.update_entity(entity);
                            self.tabs[i].dirty = true;
                        }
                        handle
                    }
                    None => {
                        entity.common_mut().handle = Handle::NULL;
                        style(&mut entity);
                        let Some(handle) = self.commit_entity_handle_preserve_style(entity) else {
                            continue;
                        };
                        self.tabs[i].dirty = true;
                        handle
                    }
                };
                drawn.push(handle);
                objects.push(json!({ "handle": handle.value() }));
            }
            if objects.is_empty() {
                continue;
            }
            let object = if objects.len() == 1 {
                objects.remove(0)
            } else {
                Value::Array(objects)
            };
            let mut result: Vec<(String, Value)> = fields
                .iter()
                .filter(|(field, _)| {
                    !matches!(
                        field.as_str(),
                        "layer" | "color" | "linetype" | "lineweight" | "data" | "object"
                    )
                })
                .cloned()
                .collect();
            result.extend([
                ("layer".to_owned(), json!(layer)),
                (
                    "color".to_owned(),
                    match color {
                        codec::types::Color::Index(index) => json!(index),
                        _ => json!("ByLayer"),
                    },
                ),
                (
                    "linetype".to_owned(),
                    json!(linetype.clone().unwrap_or_else(|| "ByLayer".to_owned())),
                ),
                (
                    "lineweight".to_owned(),
                    match line_weight {
                        codec::types::LineWeight::Value(value) => json!(f64::from(value) / 100.0),
                        _ => json!("ByLayer"),
                    },
                ),
                (
                    "data".to_owned(),
                    json!(data
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect::<Vec<_>>()
                        .join("; ")),
                ),
                ("object".to_owned(), object),
            ]);
            results.push(result);
        }
        if !spare.is_empty() {
            self.tabs[i].scene.erase_entities(&spare);
            self.tabs[i].dirty = true;
        }
        if let Some(ui) = self.tabs[i].graph.ui.get_mut(&node) {
            ui.handles = drawn;
        }
        results
    }

    pub(super) fn graph_run(&mut self, i: usize) -> Task<Message> {
        self.graph_step(i, |_| Vec::new())
    }

    /// Runs the active graph again when what its Layer Objects nodes read
    /// has changed since the last edit: objects drawn, edited or erased on a
    /// watched layer. Undo and redo leave it be (see `graph_follow_absorb`),
    /// as they bring back the graph's output with the drawing.
    pub(super) fn graph_follow_drawing(&mut self) -> Task<Message> {
        let i = self.active_tab;
        let Some(tab) = self.tabs.get(i) else {
            return Task::none();
        };
        if tab.graph.followed_revision == tab.edit_revision {
            return Task::none();
        }
        let fingerprint = watched_fingerprint(tab);
        let changed = fingerprint.is_some() && fingerprint != tab.graph.watched;
        self.tabs[i].graph.followed_revision = self.tabs[i].edit_revision;
        if !changed {
            return Task::none();
        }
        let task = self.graph_run(i);
        self.tabs[i].graph.watched = watched_fingerprint(&self.tabs[i]);
        task
    }

    /// Writes each instance's data onto the objects it names, which the
    /// node does not own.
    fn graph_tag(
        &mut self,
        i: usize,
        instances: &[Vec<(String, Value)>],
    ) -> Vec<Vec<(String, Value)>> {
        let mut results = Vec::new();
        for fields in instances {
            let get = |name: &str| {
                fields
                    .iter()
                    .find(|(field, _)| field == name)
                    .map(|(_, value)| value.clone())
                    .unwrap_or(Value::Null)
            };
            let pairs = data_pairs(&graph::value::text(&get("data")));
            let objects = get("objects");
            let handles: Vec<Handle> = match &objects {
                Value::Array(items) => items.iter().collect::<Vec<_>>(),
                other => vec![other],
            }
            .into_iter()
            .filter_map(|object| object.get("handle")?.as_u64())
            .map(Handle::new)
            .collect();
            if !pairs.is_empty() {
                self.tabs[i].scene.ensure_app_id(DATA_APP);
            }
            for handle in handles {
                if self.tabs[i].scene.is_layer_locked(handle) {
                    continue;
                }
                let Some(mut entity) = self.tabs[i].scene.document.get_entity(handle).cloned()
                else {
                    continue;
                };
                let before = entity_data(&entity);
                write_data(&mut entity, &pairs);
                if entity_data(&entity) != before {
                    self.tabs[i].scene.update_entity(entity);
                    self.tabs[i].dirty = true;
                }
            }
            results.push(vec![
                ("objects".to_owned(), objects),
                ("data".to_owned(), get("data")),
            ]);
        }
        results
    }

    /// `path` as a file path: relative paths are taken from the drawing's
    /// folder, or the working folder for an unsaved drawing.
    #[cfg(not(target_arch = "wasm32"))]
    fn graph_file_path(&self, i: usize, path: &str) -> Option<std::path::PathBuf> {
        let path = std::path::Path::new(path.trim());
        if path.as_os_str().is_empty() {
            return None;
        }
        if path.is_absolute() {
            return Some(path.to_path_buf());
        }
        let folder = self.tabs[i]
            .current_path
            .as_ref()
            .and_then(|file| file.parent().map(std::path::Path::to_path_buf))
            .unwrap_or_default();
        Some(folder.join(path))
    }

    /// Takes the drawing as the graph's Layer Objects nodes now see it,
    /// without running the graph.
    pub(super) fn graph_follow_absorb(&mut self, i: usize) {
        let tab = &mut self.tabs[i];
        tab.graph.watched = watched_fingerprint(tab);
        tab.graph.followed_revision = tab.edit_revision;
    }

    pub(super) fn on_graph(&mut self, message: GraphMsg) -> Task<Message> {
        let i = self.active_tab;
        let owned_before = matches!(message, GraphMsg::Undo | GraphMsg::Redo)
            .then(|| self.tabs[i].graph.owned_handles());
        self.tabs[i].graph.update_ui(&message);
        match message {
            GraphMsg::Toggle => {
                self.show_node_graph = !self.show_node_graph;
                if self.show_node_graph {
                    // The node library docks on the right on first use; after
                    // that it stays wherever the user moved it.
                    let id = crate::ui::dock::PanelId::NodeGraph;
                    self.dock_open_at_default(id);
                    return self.graph_run(i);
                }
                self.graph_refresh_preview();
            }
            GraphMsg::TogglePreview | GraphMsg::Hover(_) | GraphMsg::HoverEnd(_) => {
                self.graph_refresh_preview()
            }
            GraphMsg::Add(item) => {
                self.tabs[i].graph.record();
                self.tabs[i].graph.add_node(item, Vec::new());
                return self.graph_run(i);
            }
            // The canvas already changed the graph; re-evaluate. A move held
            // for confirmation changed nothing yet.
            GraphMsg::Choose(_)
            | GraphMsg::Unlink(_)
            | GraphMsg::PickNewInput(..)
            | GraphMsg::Move(..)
            | GraphMsg::MoveNow(..)
            | GraphMsg::DragDrop(_)
            | GraphMsg::MoveToNow(..) => {
                if self.tabs[i].graph.confirm.is_none() {
                    return self.graph_run(i);
                }
            }
            GraphMsg::ToggleSlider(_) => return self.graph_run(i),
            GraphMsg::NodeSearch(_) | GraphMsg::NodeSearchStep(_) => {
                self.graph_refresh_preview();
                let Some((input, along)) = self.tabs[i].graph.found_place() else {
                    return Task::none();
                };
                let (id, offset) = if input {
                    (
                        crate::ui::node_graph::INPUTS_SCROLL_ID,
                        iced::widget::scrollable::RelativeOffset {
                            x: None,
                            y: Some(along),
                        },
                    )
                } else {
                    (
                        crate::ui::node_graph::SEQUENCE_SCROLL_ID,
                        iced::widget::scrollable::RelativeOffset {
                            x: Some(along),
                            y: None,
                        },
                    )
                };
                return iced::widget::operation::snap_to(iced::widget::Id::new(id), offset);
            }
            GraphMsg::CopyNodes(ids) => {
                if ids.is_empty() {
                    return Task::none();
                }
                let text = self.tabs[i].graph.copy_text(&ids);
                self.command_line
                    .push_info(crate::tf!("{} nodes copied", ids.len()).as_ref());
                return iced::clipboard::write(text).discard();
            }
            GraphMsg::Paste => {
                #[cfg(target_arch = "wasm32")]
                return Task::perform(crate::sys::read_clipboard_text(), |text| {
                    Message::Graph(GraphMsg::Pasted(text))
                });
                #[cfg(not(target_arch = "wasm32"))]
                return iced::clipboard::read_text().map(|result| {
                    Message::Graph(GraphMsg::Pasted(result.ok().map(|text| (*text).clone())))
                });
            }
            GraphMsg::Pasted(text) => {
                match self.tabs[i]
                    .graph
                    .paste_text(text.as_deref().unwrap_or_default())
                {
                    Ok(added) => {
                        self.tabs[i].graph.steps_hidden = false;
                        self.command_line
                            .push_output(crate::tf!("{} nodes pasted", added.len()).as_ref());
                        return self.graph_run(i);
                    }
                    Err(error) => self.command_line.push_error(&error),
                }
            }
            GraphMsg::ConfirmYes => {
                if let Some(confirm) = self.tabs[i].graph.confirm.take() {
                    return self.on_graph(confirm.action);
                }
            }
            GraphMsg::Duplicate(_) => {
                // An object copy starts from its own copy of the original's
                // first entity.
                if let Some((original, copy)) = self.tabs[i].graph.duplicated.take() {
                    self.graph_copy_entity(i, original, copy);
                }
                return self.graph_run(i);
            }
            GraphMsg::Undo | GraphMsg::Redo => {
                // Entities of nodes the step took away go with them; nodes it
                // brought back recreate theirs on evaluation.
                let owned = self.tabs[i].graph.owned_handles();
                let dropped: Vec<Handle> = owned_before
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|handle| !owned.contains(handle))
                    .collect();
                return self.graph_step(i, |app| {
                    app.graph_erase(i, dropped);
                    Vec::new()
                });
            }
            GraphMsg::FromSelection => return self.graph_from_selection(i),
            GraphMsg::GroupToComposite(group) => match self.tabs[i].graph.make_composite(group) {
                Ok(_) => return self.graph_run(i),
                Err(error) => self.command_line.push_error(&error),
            },
            GraphMsg::ExpandComposite(id) => match self.tabs[i].graph.expand_composite(id) {
                Ok(_) => return self.graph_run(i),
                Err(error) => self.command_line.push_error(&error),
            },
            GraphMsg::SaveComposite(id) => {
                let graph = &self.tabs[i].graph;
                if let Some(Kind::Composite(composite)) =
                    graph.engine.node(id).map(|node| node.kind.clone())
                {
                    let name = graph.name(id);
                    let mut saved = (*composite).clone();
                    saved.name = name.clone();
                    crate::ui::node_graph::save_to_library(std::sync::Arc::new(saved));
                    self.command_line.push_output(
                        crate::tf!("{} saved to the composite library", name).as_ref(),
                    );
                }
            }
            GraphMsg::ForgetComposite(k) => crate::ui::node_graph::forget_library_entry(k),
            GraphMsg::GroupRenameStart(_) => {
                return iced::widget::operation::focus(iced::widget::Id::new(
                    crate::ui::node_graph::RENAME_ID,
                ));
            }
            GraphMsg::Pick(_) => {
                return iced::widget::operation::focus(iced::widget::Id::new(
                    crate::ui::node_graph::PICK_SEARCH_ID,
                ));
            }
            GraphMsg::RenameStart(_) => {
                return iced::widget::operation::focus(iced::widget::Id::new(
                    crate::ui::node_graph::RENAME_ID,
                ));
            }
            GraphMsg::NodeDelete(id) | GraphMsg::DeleteNow(id) => {
                // A node others read from waits for confirmation.
                if self.tabs[i].graph.confirm.is_some() {
                    return Task::none();
                }
                let handles = self.tabs[i].graph.remove_node(id);
                return self.graph_step(i, |app| {
                    app.graph_erase(i, handles);
                    Vec::new()
                });
            }
            GraphMsg::Commit(port) => {
                let Some(raw) = self.tabs[i].graph.drafts.remove(&port) else {
                    return Task::none();
                };
                // Typed text is protocol JSON when it parses (numbers, true,
                // {"rgb":[..]}), otherwise a plain string.
                let value = serde_json::from_str(raw.trim()).unwrap_or(Value::String(raw));
                return self.graph_set(i, port, value);
            }
            GraphMsg::Set(port, value) => return self.graph_set(i, port, value),
            // Typing in the paste box changes nothing; a paste of rows does.
            GraphMsg::TablePaste(port, text) if text.contains(['\t', '\n']) => {
                self.tabs[i].graph.record();
                let rows = graph::value::table(&Value::String(text));
                let value = Value::Array(rows.into_iter().map(Value::Array).collect());
                return self.graph_set(i, port, value);
            }
            GraphMsg::New => {
                self.tabs[i].graph.clear();
            }
            GraphMsg::CompositeEditSave => return self.graph_run(i),
            GraphMsg::LibraryExport => {
                let bytes = crate::ui::node_graph::library_file();
                return Task::perform(
                    async move {
                        let handle = crate::sys::file_dialog()
                            .set_title(crate::t!("Export the composite library").as_ref())
                            .add_filter(crate::t!("Composites").as_ref(), &["json"])
                            .set_file_name("composites.json")
                            .save_file()
                            .await?;
                        let name = crate::sys::handle_path(&handle)
                            .to_string_lossy()
                            .into_owned();
                        Some(
                            handle
                                .write(&bytes)
                                .await
                                .map(|()| name)
                                .map_err(|e| e.to_string()),
                        )
                    },
                    |result| Message::Graph(GraphMsg::LibraryExported(result)),
                );
            }
            GraphMsg::LibraryExported(Some(Ok(name))) => self
                .command_line
                .push_output(crate::tf!("Composite library exported to {}", name).as_ref()),
            GraphMsg::LibraryExported(Some(Err(error))) => self.command_line.push_error(&error),
            GraphMsg::LibraryImport => {
                return Task::perform(
                    async {
                        let handle = crate::sys::file_dialog()
                            .set_title(crate::t!("Import composites").as_ref())
                            .add_filter(crate::t!("Composites").as_ref(), &["json"])
                            .pick_file()
                            .await?;
                        Some(handle.read().await)
                    },
                    |bytes| Message::Graph(GraphMsg::LibraryImported(bytes)),
                );
            }
            GraphMsg::LibraryImported(Some(bytes)) => {
                match crate::ui::node_graph::import_library(&bytes) {
                    Ok(count) => self.command_line.push_output(
                        crate::tf!("{} composites added to the library", count).as_ref(),
                    ),
                    Err(error) => self
                        .command_line
                        .push_error(crate::tf!("Not a composite library: {}", error).as_ref()),
                }
            }
            GraphMsg::Save => {
                // The file names the drawing by its fingerprint so reopening it
                // elsewhere cannot take over unrelated entities; a drawing that
                // never had one gets it now and saves it with its next save.
                let tab = &mut self.tabs[i];
                let header = &mut tab.scene.document.header;
                if header.fingerprint_guid.is_empty() {
                    header.fingerprint_guid =
                        format!("{{{}}}", crate::app::control::new_guid_v4().to_uppercase());
                    tab.dirty = true;
                }
                let bytes = tab
                    .graph
                    .to_file(&tab.scene.document.header.fingerprint_guid);
                return Task::perform(
                    async move {
                        let handle = crate::sys::file_dialog()
                            .set_title(crate::t!("Save Node Graph").as_ref())
                            .add_filter(crate::t!("Node Graph").as_ref(), &["ocg"])
                            .set_file_name("graph.ocg")
                            .save_file()
                            .await?;
                        let name = crate::sys::handle_path(&handle)
                            .to_string_lossy()
                            .into_owned();
                        Some(
                            handle
                                .write(&bytes)
                                .await
                                .map(|()| name)
                                .map_err(|e| e.to_string()),
                        )
                    },
                    |result| Message::Graph(GraphMsg::Saved(result)),
                );
            }
            GraphMsg::Saved(Some(Ok(name))) => {
                self.command_line
                    .push_output(crate::tf!("Node graph saved to {}", name).as_ref());
            }
            GraphMsg::Saved(Some(Err(error))) => self.command_line.push_error(&error),
            GraphMsg::Open => {
                return Task::perform(
                    async {
                        let handle = crate::sys::file_dialog()
                            .set_title(crate::t!("Open Node Graph").as_ref())
                            .add_filter(crate::t!("Node Graph").as_ref(), &["ocg"])
                            .pick_file()
                            .await?;
                        let _ = crate::sys::handle_path(&handle);
                        Some(handle.read().await)
                    },
                    |bytes| Message::Graph(GraphMsg::Loaded(bytes)),
                );
            }
            GraphMsg::Loaded(Some(bytes)) => {
                let (mut graph, drawing) = match crate::ui::node_graph::Graph::from_file(&bytes) {
                    Ok(loaded) => loaded,
                    Err(error) => {
                        self.command_line.push_error(&error);
                        return Task::none();
                    }
                };
                // Entities are reused only in the drawing the graph was saved
                // against, and only while they are still the node's kind;
                // otherwise evaluation creates fresh ones.
                let document = &self.tabs[i].scene.document;
                let same_drawing =
                    !drawing.is_empty() && drawing == document.header.fingerprint_guid;
                for node in &graph.engine.nodes {
                    let kind = match &node.kind {
                        Kind::Object(name) => ObjectKind::from_name(name),
                        Kind::Op(_) | Kind::Composite(_) | Kind::Script(_) => None,
                    };
                    if let Some(ui) = graph.ui.get_mut(&node.id) {
                        ui.handles.retain(|handle| {
                            same_drawing
                                && kind
                                    .zip(document.get_entity(*handle))
                                    .is_some_and(|(kind, entity)| kind.matches(entity))
                        });
                    }
                }
                self.tabs[i].graph = graph;
                return self.graph_run(i);
            }
            _ => {}
        }
        Task::none()
    }

    /// Adopts the selected drawing entities: one object node each, owning
    /// the entity as drawn. Entities another node owns, and kinds no node
    /// can own, are skipped and counted.
    fn graph_from_selection(&mut self, i: usize) -> Task<Message> {
        let tab = &self.tabs[i];
        let owned = tab.graph.owned_handles();
        let mut selected: Vec<Handle> = tab.scene.selected.iter().copied().collect();
        selected.sort_by_key(|handle| handle.value());
        let (mut picked, mut taken, mut unsupported) = (Vec::new(), 0, 0);
        for handle in selected {
            let Some(entity) = tab.scene.document.get_entity(handle) else {
                continue;
            };
            match ObjectKind::of(entity) {
                _ if owned.contains(&handle) => taken += 1,
                Some(kind) => picked.push((kind, handle)),
                None => unsupported += 1,
            }
        }
        if taken > 0 {
            self.command_line
                .push_info(crate::tf!("{} objects already belong to a node", taken).as_ref());
        }
        if unsupported > 0 {
            self.command_line.push_info(
                crate::tf!("{} objects are of a kind nodes cannot hold", unsupported).as_ref(),
            );
        }
        if picked.is_empty() {
            self.command_line.push_error(
                crate::t!("Select lines, circles, arcs, ellipses, points, polylines, rays, construction lines or texts in the drawing first").as_ref(),
            );
            return Task::none();
        }
        let graph = &mut self.tabs[i].graph;
        graph.record();
        graph.steps_hidden = false;
        let count = picked.len();
        for (kind, handle) in picked {
            graph.add_node(
                crate::ui::node_graph::PaletteItem::Object(kind),
                vec![handle],
            );
        }
        self.command_line
            .push_output(crate::tf!("{} nodes added from the drawing", count).as_ref());
        self.graph_run(i)
    }

    /// Gives the copy `copy` of object node `original` its own copy of the
    /// original's first entity.
    pub(super) fn graph_copy_entity(&mut self, i: usize, original: NodeId, copy: NodeId) {
        let first = self.tabs[i]
            .graph
            .ui
            .get(&original)
            .and_then(|ui| ui.handles.first().copied());
        let entity = first
            .and_then(|handle| self.tabs[i].scene.document.get_entity(handle))
            .cloned();
        if let Some(mut entity) = entity {
            entity.common_mut().handle = Handle::NULL;
            if let Some(handle) = self.commit_entity_handle(entity) {
                self.tabs[i].dirty = true;
                if let Some(ui) = self.tabs[i].graph.ui.get_mut(&copy) {
                    ui.handles = vec![handle];
                }
            }
        }
    }

    /// Erases the entities in `handles` that still exist.
    pub(super) fn graph_erase(&mut self, i: usize, handles: Vec<Handle>) {
        let live: Vec<Handle> = handles
            .into_iter()
            .filter(|handle| self.tabs[i].scene.document.get_entity(*handle).is_some())
            .collect();
        if !live.is_empty() {
            self.tabs[i].scene.erase_entities(&live);
            self.tabs[i].dirty = true;
            self.refresh_properties();
        }
    }

    /// An edited input: an operation keeps it as a parameter; an object node
    /// writes it to every entity it owns.
    fn graph_set(&mut self, i: usize, port: Port, value: Value) -> Task<Message> {
        let graph = &mut self.tabs[i].graph;
        let Some(node) = graph.engine.node_mut(port.node) else {
            return Task::none();
        };
        if let Kind::Op(_) = node.kind {
            node.params.insert(port.field, value);
            return self.graph_run(i);
        }
        let handles = graph
            .ui
            .get(&port.node)
            .map(|ui| ui.handles.clone())
            .unwrap_or_default();
        self.graph_step(i, |app| {
            handles
                .into_iter()
                .map(|handle| app.graph_write(i, handle, &port.field, &value))
                .collect()
        })
    }
}

/// The drawing as the graph sees it.
struct AppHost<'a> {
    app: &'a mut OpenCADStudio,
    i: usize,
    tasks: Vec<Task<Message>>,
}

impl graph::Host for AppHost<'_> {
    fn objects(
        &mut self,
        node: NodeId,
        kind: &str,
        instances: &[Vec<(String, Value)>],
    ) -> Vec<Vec<(String, Value)>> {
        let (app, i) = (&mut *self.app, self.i);
        let Some(kind) = ObjectKind::from_name(kind) else {
            return Vec::new();
        };
        match kind {
            ObjectKind::Tag => return app.graph_tag(i, instances),
            // One table from all the rows, which arrive one per instance.
            ObjectKind::Table => {
                let mut table = instances[0].clone();
                table.retain(|(field, _)| field != "rows");
                let rows = instances
                    .iter()
                    .filter_map(|fields| fields.iter().find(|(field, _)| field == "rows"))
                    .map(|(_, row)| row.clone())
                    .collect();
                table.push(("rows".to_owned(), Value::Array(rows)));
                return app.graph_draw(i, node, kind, &[table]);
            }
            _ if kind.places() => return app.graph_draw(i, node, kind, instances),
            _ => {}
        }
        let mut handles: Vec<Handle> = app.tabs[i]
            .graph
            .ui
            .get(&node)
            .map(|ui| ui.handles.clone())
            .unwrap_or_default();
        handles.retain(|handle| app.tabs[i].scene.document.get_entity(*handle).is_some());
        // New entities copy the node's first one, so edits made by hand to
        // unlinked rows carry over to every element.
        while handles.len() < instances.len() {
            let mut entity = handles
                .first()
                .and_then(|handle| app.tabs[i].scene.document.get_entity(*handle))
                .cloned()
                .unwrap_or_else(|| kind.entity());
            entity.common_mut().handle = Handle::NULL;
            let Some(handle) = app.commit_entity_handle(entity) else {
                break;
            };
            handles.push(handle);
            app.tabs[i].dirty = true;
        }
        if handles.len() > instances.len() {
            let extra = handles.split_off(instances.len());
            app.tabs[i].scene.erase_entities(&extra);
            app.tabs[i].dirty = true;
        }
        if let Some(ui) = app.tabs[i].graph.ui.get_mut(&node) {
            ui.handles = handles.clone();
        }
        for (handle, fields) in handles.iter().zip(instances) {
            for (field, value) in fields {
                let current = app
                    .graph_fields(i, *handle)
                    .into_iter()
                    .find(|(name, _)| name == field)
                    .map(|(_, value)| value);
                if current.as_ref() != Some(value) {
                    let task = app.graph_write(i, *handle, field, value);
                    self.tasks.push(task);
                }
            }
        }
        handles
            .iter()
            .map(|handle| {
                let mut fields = app.graph_fields(i, *handle);
                fields.push(("object".to_owned(), json!({ "handle": handle.value() })));
                fields
            })
            .collect()
    }

    fn curve(&self, object: &Value) -> Option<kernel::space::PlanarCurve> {
        let handle = Handle::new(object.get("handle")?.as_u64()?);
        let entity = self.app.tabs[self.i].scene.document.get_entity(handle)?;
        crate::entities::curve::entity_curve(entity)
    }

    fn expression(&self, text: &str) -> Option<f64> {
        crate::app::expr_eval::eval_number(text)
    }

    fn object_data(&self, object: &Value, key: &str) -> Value {
        let Some(entity) = object
            .get("handle")
            .and_then(Value::as_u64)
            .and_then(|handle| {
                self.app.tabs[self.i]
                    .scene
                    .document
                    .get_entity(Handle::new(handle))
            })
        else {
            return Value::Null;
        };
        let data = entity_data(entity);
        if key.is_empty() {
            return Value::Array(
                data.iter()
                    .map(|(key, value)| json!(format!("{key}={value}")))
                    .collect(),
            );
        }
        data.into_iter()
            .find(|(seen, _)| seen.eq_ignore_ascii_case(key))
            .map_or(Value::Null, |(_, value)| match value.parse::<f64>() {
                Ok(x) => graph::value::number(x),
                Err(_) => Value::String(value),
            })
    }

    fn read_file(&self, path: &str) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = self.app.graph_file_path(self.i, path)?;
            let bytes = std::fs::read(path).ok()?;
            Some(String::from_utf8_lossy(&bytes).into_owned())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            None
        }
    }

    fn write_file(&self, path: &str, text: &str) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(path) = self.app.graph_file_path(self.i, path) else {
                return false;
            };
            // Only data files, so a graph never overwrites a drawing.
            let data_file = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("csv") || ext.eq_ignore_ascii_case("txt")
                });
            if !data_file {
                return false;
            }
            std::fs::read(&path).is_ok_and(|old| old == text.as_bytes())
                || std::fs::write(&path, text).is_ok()
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (path, text);
            false
        }
    }

    fn layer_objects(&self, layer: &str, kind: &str) -> Vec<Value> {
        watched_objects(&self.app.tabs[self.i], layer, kind)
            .map(|entity| json!({ "handle": entity.common().handle.value() }))
            .collect()
    }
}

/// The drawn objects a Layer Objects node reads: those on `layer` (any
/// when empty) of the object type `kind` (any when empty), leaving out what
/// the graph draws itself.
fn watched_objects<'t>(
    tab: &'t crate::app::document::DocumentTab,
    layer: &str,
    kind: &str,
) -> impl Iterator<Item = &'t codec::EntityType> {
    let owned: std::collections::HashSet<Handle> = tab.graph.owned_handles().into_iter().collect();
    let kind = (!kind.is_empty()).then(|| {
        crate::ui::node_graph::OBJECTS
            .iter()
            .copied()
            .find(|object| object.name().eq_ignore_ascii_case(kind))
    });
    let layer = layer.to_owned();
    tab.scene.document.entities().filter(move |entity| {
        let common = entity.common();
        kind != Some(None)
            && (layer.is_empty() || common.layer.eq_ignore_ascii_case(&layer))
            && !owned.contains(&common.handle)
            && kind.flatten().is_none_or(|kind| kind.matches(entity))
    })
}

/// A fingerprint of everything the graph's Layer Objects nodes read, or
/// `None` when it has none: when it changes the graph has to run again.
pub(super) fn watched_fingerprint(tab: &crate::app::document::DocumentTab) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let texts = |value: Value| -> Vec<String> {
        match value {
            Value::Array(items) => items.iter().map(graph::value::text).collect(),
            other => vec![graph::value::text(&other)],
        }
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut any = false;
    for node in &tab.graph.engine.nodes {
        if !matches!(&node.kind, Kind::Op(spec) if spec.name == "Layer Objects") {
            continue;
        }
        any = true;
        let input = |field: &str| {
            tab.graph.engine.input(&Port {
                node: node.id,
                field: field.to_owned(),
            })
        };
        for layer in texts(input("layer")) {
            for kind in texts(input("type")) {
                for entity in watched_objects(tab, layer.trim(), kind.trim()) {
                    format!("{entity:?}").hash(&mut hasher);
                }
            }
        }
    }
    any.then(|| hasher.finish())
}

/// Every curve in `value`, through nested lists.
fn collect_curves(value: &Value, curves: &mut Vec<kernel::space::PlanarCurve>) {
    match value {
        Value::Array(items) => items.iter().for_each(|item| collect_curves(item, curves)),
        Value::Object(_) => curves.extend(graph::value::curve(value)),
        _ => {}
    }
}

/// Whether `kind` is a node whose entities follow its inputs.
fn is_placing(kind: &Kind) -> bool {
    matches!(kind, Kind::Object(name) if ObjectKind::from_name(name).is_some_and(ObjectKind::places))
}

/// A placing node's rows: field, label, whether it is an input, and the
/// kind of value it takes. Every one ends with the layer, colour, linetype
/// and lineweight (in millimetres) it draws with, the data it writes on its
/// entities, and the entities it drew. Tag Objects draws nothing: it takes
/// the objects to write its data on.
fn place_fields(kind: ObjectKind) -> Vec<(&'static str, &'static str, bool, Type, bool)> {
    let mut fields = vec![("object", "Object", false, Type::Object, false)];
    fields.extend_from_slice(match kind {
        ObjectKind::Hatch => &[
            ("boundary", "Boundary", true, Type::Curve, true),
            ("pattern", "Pattern", true, Type::Text, false),
            ("scale", "Scale", true, Type::Number, false),
            ("angle", "Angle", true, Type::Number, false),
        ][..],
        ObjectKind::Block => &[
            ("block", "Block", true, Type::Text, false),
            ("point", "Point", true, Type::Point, false),
            ("rotation", "Rotation", true, Type::Number, false),
            ("scale", "Scale", true, Type::Number, false),
        ][..],
        ObjectKind::Dimension => &[
            ("from", "From", true, Type::Point, false),
            ("to", "To", true, Type::Point, false),
            ("offset", "Offset", true, Type::Number, false),
            ("text", "Text", true, Type::Text, false),
        ][..],
        ObjectKind::Table => &[
            ("rows", "Rows", true, Type::Any, true),
            ("point", "Point", true, Type::Point, false),
            ("height", "Text height", true, Type::Number, false),
            ("width", "Column width", true, Type::Number, false),
        ][..],
        ObjectKind::Label => &[
            ("curve", "Curve", true, Type::Curve, false),
            ("text", "Text", true, Type::Text, false),
            ("distance", "Distance", true, Type::Number, false),
            ("offset", "Offset", true, Type::Number, false),
            ("height", "Text height", true, Type::Number, false),
        ][..],
        ObjectKind::Tag => {
            return vec![
                ("objects", "Objects", true, Type::Object, true),
                ("data", "Data", true, Type::Text, false),
            ];
        }
        _ => &[("geometry", "Geometry", true, Type::Curve, true)][..],
    });
    fields.push(("layer", "Layer", true, Type::Text, false));
    fields.push(("color", "Color", true, Type::Number, false));
    fields.push(("linetype", "Linetype", true, Type::Text, false));
    fields.push(("lineweight", "Lineweight", true, Type::Number, false));
    fields.push(("data", "Data", true, Type::Text, false));
    fields
}

/// What a placing node of `kind` draws from one set of its inputs.
fn place_entities(
    kind: ObjectKind,
    get: &dyn Fn(&str) -> Option<Value>,
    document: &codec::CadDocument,
) -> Vec<codec::EntityType> {
    use codec::entities as e;
    use codec::types::Vector3;
    let number = |field: &str, default: f64| {
        get(field)
            .as_ref()
            .and_then(graph::value::num)
            .unwrap_or(default)
    };
    let point = |field: &str| get(field).as_ref().and_then(graph::value::point);
    match kind {
        ObjectKind::Hatch => {
            let mut curves = Vec::new();
            collect_curves(&get("boundary").unwrap_or(Value::Null), &mut curves);
            let paths: Vec<e::BoundaryPath> = curves
                .iter()
                .filter(|curve| curve.curve.is_closed())
                .filter_map(boundary_path)
                .enumerate()
                .map(|(k, edge)| {
                    let mut path = if k == 0 {
                        e::BoundaryPath::external()
                    } else {
                        e::BoundaryPath::new()
                    };
                    path.add_edge(edge);
                    path
                })
                .collect();
            if paths.is_empty() {
                return Vec::new();
            }
            let pattern = get("pattern")
                .map(|value| graph::value::text(&value).trim().to_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "SOLID".to_owned());
            let mut hatch = if pattern.eq_ignore_ascii_case("SOLID") {
                e::Hatch::solid()
            } else {
                e::Hatch::with_pattern(e::HatchPattern::new(pattern.to_uppercase()))
                    .with_pattern_scale(number("scale", 1.0).abs().max(1e-6))
                    .with_pattern_angle(number("angle", 0.0).to_radians())
            };
            hatch.paths = paths;
            vec![codec::EntityType::Hatch(Box::new(hatch))]
        }
        ObjectKind::Block => {
            let name = get("block").map_or_else(String::new, |value| graph::value::text(&value));
            let Some(at) = point("point") else {
                return Vec::new();
            };
            if document.block_records.get(name.trim()).is_none() {
                return Vec::new();
            }
            let insert = e::Insert::new(name.trim(), Vector3::new(at[0], at[1], at[2]))
                .with_uniform_scale(number("scale", 1.0))
                .with_rotation(number("rotation", 0.0).to_radians());
            vec![codec::EntityType::Insert(insert)]
        }
        ObjectKind::Dimension => {
            let (Some(from), Some(to)) = (point("from"), point("to")) else {
                return Vec::new();
            };
            let (from, to) = (glam::DVec3::from_array(from), glam::DVec3::from_array(to));
            let along = to - from;
            if along.length() <= 1e-9 {
                return Vec::new();
            }
            let left = glam::DVec3::new(-along.y, along.x, 0.0).normalize_or_zero();
            let line = (from + to) * 0.5 + left * number("offset", 1.0);
            let text = get("text")
                .map(|value| graph::value::text(&value))
                .filter(|text| !text.trim().is_empty());
            vec![
                crate::modules::annotate::aligned_dim::aligned_dimension_entity(
                    from, to, line, text,
                ),
            ]
        }
        ObjectKind::Table => table_entities(
            &graph::value::list(&get("rows").unwrap_or(Value::Null)),
            point("point").unwrap_or_default(),
            number("height", 2.5).abs().max(1e-6),
            number("width", 0.0).max(0.0),
        ),
        ObjectKind::Label => {
            let (Some(curve), Some(label)) = (
                get("curve").as_ref().and_then(graph::value::curve),
                get("text").map(|value| graph::value::text(&value)),
            ) else {
                return Vec::new();
            };
            let length = curve.length();
            let distance = get("distance")
                .as_ref()
                .and_then(graph::value::num)
                .unwrap_or(length * 0.5);
            let Some(frame) =
                graph::geometry::curve_frame(&curve, curve.parameter_at_distance(distance))
            else {
                return Vec::new();
            };
            let offset = number("offset", 0.0);
            let at =
                graph::geometry::add(frame.origin, graph::geometry::scale(frame.y_axis, offset));
            // Kept readable: a text running leftwards turns round.
            let mut angle = frame.x_axis[1].atan2(frame.x_axis[0]);
            if angle.cos() < -1e-9 {
                angle += std::f64::consts::PI;
            }
            let at = Vector3::new(at[0], at[1], at[2]);
            let mut text = e::Text::with_value(label, at)
                .with_height(number("height", 2.5).abs().max(1e-6))
                .with_rotation(angle);
            text.horizontal_alignment = e::TextHorizontalAlignment::Center;
            text.vertical_alignment = if offset < 0.0 {
                e::TextVerticalAlignment::Top
            } else {
                e::TextVerticalAlignment::Bottom
            };
            text.alignment_point = Some(at);
            vec![codec::EntityType::Text(text)]
        }
        _ => {
            let mut curves = Vec::new();
            collect_curves(&get("geometry").unwrap_or(Value::Null), &mut curves);
            curves
                .iter()
                .filter_map(crate::entities::curve::curve_entity)
                .collect()
        }
    }
}

/// A table of `rows` with its top left corner at `corner`: a line on every
/// cell edge and each cell's text, `height` high. Columns are `width` wide,
/// or as wide as their longest text when it is zero.
fn table_entities(
    rows: &[Value],
    corner: [f64; 3],
    height: f64,
    width: f64,
) -> Vec<codec::EntityType> {
    use codec::entities as e;
    use codec::types::Vector3;
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| match row {
            Value::Array(cells) => cells.iter().map(crate::ui::node_graph::display).collect(),
            other => vec![crate::ui::node_graph::display(other)],
        })
        .collect();
    let columns = cells.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return Vec::new();
    }
    let pad = height * 0.5;
    let widths: Vec<f64> = (0..columns)
        .map(|k| {
            if width > 0.0 {
                return width;
            }
            let chars = cells
                .iter()
                .filter_map(|row| row.get(k))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0);
            (chars as f64 * height * 0.8 + 2.0 * pad).max(height * 2.0)
        })
        .collect();
    let row_height = height + 2.0 * pad;
    let [x0, y0, z] = corner;
    let total_w: f64 = widths.iter().sum();
    let total_h = row_height * cells.len() as f64;
    let line = |a: (f64, f64), b: (f64, f64)| {
        codec::EntityType::Line(e::Line::from_coords(a.0, a.1, z, b.0, b.1, z))
    };
    let mut entities = Vec::new();
    for k in 0..=cells.len() {
        let y = y0 - row_height * k as f64;
        entities.push(line((x0, y), (x0 + total_w, y)));
    }
    let mut x = x0;
    for k in 0..=columns {
        entities.push(line((x, y0), (x, y0 - total_h)));
        x += widths.get(k).copied().unwrap_or(0.0);
    }
    for (r, row) in cells.iter().enumerate() {
        let mut x = x0;
        for (k, cell) in row.iter().enumerate() {
            if !cell.is_empty() {
                let at = Vector3::new(x + pad, y0 - row_height * (r + 1) as f64 + pad, z);
                entities.push(codec::EntityType::Text(
                    e::Text::with_value(cell.clone(), at).with_height(height),
                ));
            }
            x += widths[k];
        }
    }
    entities
}

/// The XDATA application drawn objects keep the graph's data under.
const DATA_APP: &str = "OCS_DATA";

/// `key=value` pairs from text such as `kind=door; segment=A`, one pair per
/// `;` or line.
fn data_pairs(text: &str) -> Vec<(String, String)> {
    text.split([';', '\n'])
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            let key = key.trim();
            (!key.is_empty()).then(|| (key.to_owned(), value.trim().to_owned()))
        })
        .collect()
}

/// The data pairs `entity` carries.
fn entity_data(entity: &codec::EntityType) -> Vec<(String, String)> {
    entity
        .common()
        .extended_data
        .get_record(DATA_APP)
        .map(|record| {
            record
                .values
                .iter()
                .filter_map(|value| match value {
                    codec::xdata::XDataValue::String(pair) => data_pairs(pair).into_iter().next(),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Writes `pairs` onto `entity`, over the values of the same keys it had.
fn write_data(entity: &mut codec::EntityType, pairs: &[(String, String)]) {
    let mut data = entity_data(entity);
    for (key, value) in pairs {
        match data
            .iter_mut()
            .find(|(seen, _)| seen.eq_ignore_ascii_case(key))
        {
            Some(entry) => entry.1 = value.clone(),
            None => data.push((key.clone(), value.clone())),
        }
    }
    let mut record = codec::xdata::ExtendedDataRecord::new(DATA_APP);
    for (key, value) in data {
        record.add_value(codec::xdata::XDataValue::String(format!("{key}={value}")));
    }
    entity.common_mut().extended_data.upsert_record(record);
}

/// A closed curve on a plane facing up as one hatch boundary edge, in world
/// coordinates; arcs keep their bulges.
fn boundary_path(curve: &kernel::space::PlanarCurve) -> Option<codec::entities::BoundaryEdge> {
    use codec::types::Vector3;
    use kernel::geom2d::Curve;
    let normal = curve.normal()?;
    if (normal[2] - 1.0).abs() > 1e-9 {
        return None;
    }
    let world = |p: [f64; 2], bulge: f64| {
        let w = curve.plane.point_at(p);
        Vector3::new(w[0], w[1], bulge)
    };
    let vertices = match &curve.curve {
        Curve::Polyline(polyline) => polyline
            .vertices
            .iter()
            .map(|vertex| world(vertex.position, vertex.bulge))
            .collect(),
        Curve::Circle(circle) => {
            let [x, y] = circle.centre;
            vec![
                world([x + circle.radius, y], 1.0),
                world([x - circle.radius, y], 1.0),
            ]
        }
        _ => return None,
    };
    Some(codec::entities::BoundaryEdge::Polyline(
        codec::entities::PolylineEdge {
            vertices,
            is_closed: true,
        },
    ))
}

/// Accent of the graph's curve preview.
const PREVIEW_COLOR: [f32; 4] = [1.0, 0.55, 0.1, 0.9];
/// The geometry of the card under the pointer.
const HIGHLIGHT_COLOR: [f32; 4] = [0.1, 0.85, 1.0, 1.0];

/// The curves operation nodes compute, as overlay wires. Drawn objects show
/// themselves; unbounded curves (rays, construction lines) are left out.
fn preview_wires(
    graph: &crate::ui::node_graph::Graph,
    document: &codec::CadDocument,
) -> Vec<crate::scene::WireModel> {
    let mut wires = Vec::new();
    let mut add = |name: String, curve: &kernel::space::PlanarCurve, color: [f32; 4]| {
        if curve.curve.extent() != kernel::geom2d::Extent::Bounded {
            return;
        }
        let tolerance = (curve.length() * 1e-3).max(1e-6);
        let points = curve.tessellate_within(tolerance);
        if points.len() >= 2 {
            wires.push(crate::scene::WireModel::solid_f64(
                name, points, color, false,
            ));
        }
    };
    for node in &graph.engine.nodes {
        let hovered = graph.hovered == Some(node.id);
        let color = if hovered {
            HIGHLIGHT_COLOR
        } else {
            PREVIEW_COLOR
        };
        match node.kind {
            Kind::Op(_) | Kind::Composite(_) | Kind::Script(_) => {
                let mut curves = Vec::new();
                node.outputs
                    .values()
                    .for_each(|value| collect_curves(value, &mut curves));
                for (k, curve) in curves.iter().enumerate() {
                    add(format!("graph-preview-{}-{k}", node.id), curve, color);
                }
            }
            // Drawn objects show themselves; the hovered node's are traced
            // over them.
            Kind::Object(_) if hovered => {
                let handles = graph.ui.get(&node.id).map(|ui| ui.handles.as_slice());
                for (k, handle) in handles.unwrap_or_default().iter().enumerate() {
                    let curve = document
                        .get_entity(*handle)
                        .and_then(crate::entities::curve::entity_curve);
                    if let Some(curve) = curve {
                        add(format!("graph-hover-{}-{k}", node.id), &curve, color);
                    }
                }
            }
            Kind::Object(_) => {}
        }
    }
    wires
}
