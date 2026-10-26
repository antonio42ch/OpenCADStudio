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
            .is_some_and(|node| is_draw(&node.kind))
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

    /// A Draw node's rows: what it draws and how, and the entities it drew.
    fn graph_draw_sections(&self, i: usize, id: NodeId) -> Vec<NodeSection> {
        let graph = &self.tabs[i].graph;
        let outputs = graph.engine.node(id).map(|node| &node.outputs);
        let first = graph.ui.get(&id).and_then(|ui| ui.handles.first().copied());
        let value = |field: &str| {
            outputs
                .and_then(|outputs| outputs.get(field))
                .cloned()
                .unwrap_or(Value::Null)
        };
        let row = |field: &str, label: &str, input: bool, kind: Type, list: bool| NodeRow {
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
        };
        vec![NodeSection {
            title: String::new(),
            rows: vec![
                row("object", "Object", false, Type::Object, false),
                row("geometry", "Geometry", true, Type::Curve, true),
                row("layer", "Layer", true, Type::Text, false),
                row("color", "Color", true, Type::Number, false),
            ],
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
        if kind == ObjectKind::Draw {
            let field = |name: &str, label: &str, input: bool, kind: Expect| Field {
                name: name.to_owned(),
                label: crate::t!(label).into_owned(),
                input,
                output: true,
                kind,
            };
            return vec![
                field("object", "Object", false, Expect::Object),
                field("geometry", "Geometry", true, Expect::Curve),
                field("layer", "Layer", true, Expect::Text),
                field("color", "Color", true, Expect::Any),
            ];
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
                preview_wires(&tab.graph)
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
            let geometry = get("geometry").unwrap_or(Value::Null);
            // Nested lists (an array of arrays) draw every curve they hold.
            let mut curves = Vec::new();
            collect_curves(&geometry, &mut curves);
            let document = &self.tabs[i].scene.document;
            let layer = get("layer")
                .map(|value| graph::value::text(&value).trim().to_owned())
                .filter(|name| document.layers.get(name).is_some())
                .unwrap_or_else(|| self.tabs[i].active_layer.clone());
            let color = match get("color") {
                Some(Value::Number(index)) => index
                    .as_i64()
                    .map_or(codec::types::Color::ByLayer, |index| {
                        codec::types::Color::from_index(index as i16)
                    }),
                _ => codec::types::Color::ByLayer,
            };
            let mut objects = Vec::new();
            for curve in curves {
                let Some(mut entity) = crate::entities::curve::curve_entity(&curve) else {
                    continue;
                };
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
                        entity.common_mut().layer = layer.clone();
                        entity.common_mut().color = color;
                        self.tabs[i].scene.update_entity(entity);
                        handle
                    }
                    None => {
                        entity.common_mut().handle = Handle::NULL;
                        entity.common_mut().layer = layer.clone();
                        entity.common_mut().color = color;
                        let Some(handle) = self.commit_entity_handle_preserve_style(entity) else {
                            continue;
                        };
                        handle
                    }
                };
                self.tabs[i].dirty = true;
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
            results.push(vec![
                ("geometry".to_owned(), geometry),
                ("layer".to_owned(), json!(layer)),
                (
                    "color".to_owned(),
                    match color {
                        codec::types::Color::Index(index) => json!(index),
                        _ => json!("ByLayer"),
                    },
                ),
                ("object".to_owned(), object),
            ]);
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

    fn graph_run(&mut self, i: usize) -> Task<Message> {
        self.graph_step(i, |_| Vec::new())
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
            GraphMsg::TogglePreview => self.graph_refresh_preview(),
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
            | GraphMsg::MoveNow(..) => {
                if self.tabs[i].graph.confirm.is_none() {
                    return self.graph_run(i);
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
            GraphMsg::New => {
                self.tabs[i].graph.clear();
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
                        Kind::Op(_) | Kind::Composite(_) => None,
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
        if kind == ObjectKind::Draw {
            return app.graph_draw(i, node, instances);
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
}

/// Every curve in `value`, through nested lists.
fn collect_curves(value: &Value, curves: &mut Vec<kernel::space::PlanarCurve>) {
    match value {
        Value::Array(items) => items.iter().for_each(|item| collect_curves(item, curves)),
        Value::Object(_) => curves.extend(graph::value::curve(value)),
        _ => {}
    }
}

/// Whether `kind` is the Draw Geometry node.
fn is_draw(kind: &Kind) -> bool {
    matches!(kind, Kind::Object(name) if name == ObjectKind::Draw.name())
}

/// Accent of the graph's curve preview.
const PREVIEW_COLOR: [f32; 4] = [1.0, 0.55, 0.1, 0.9];

/// The curves operation nodes compute, as overlay wires. Drawn objects show
/// themselves; unbounded curves (rays, construction lines) are left out.
fn preview_wires(graph: &crate::ui::node_graph::Graph) -> Vec<crate::scene::WireModel> {
    let mut wires = Vec::new();
    for node in &graph.engine.nodes {
        if !matches!(node.kind, Kind::Op(_)) {
            continue;
        }
        let mut curves = Vec::new();
        node.outputs
            .values()
            .for_each(|value| collect_curves(value, &mut curves));
        for (k, curve) in curves.into_iter().enumerate() {
            if curve.curve.extent() != kernel::geom2d::Extent::Bounded {
                continue;
            }
            let tolerance = (curve.length() * 1e-3).max(1e-6);
            let points = curve.tessellate_within(tolerance);
            if points.len() >= 2 {
                wires.push(crate::scene::WireModel::solid_f64(
                    format!("graph-preview-{}-{k}", node.id),
                    points,
                    PREVIEW_COLOR,
                    false,
                ));
            }
        }
    }
    wires
}
