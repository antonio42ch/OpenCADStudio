# Node graph

A drawing's node graph builds geometry from values. It has two areas:

- **Inputs** hold the only values typed by hand: Number, Number Slider, Integer Slider, Text and Boolean nodes.
- The **node sequence** holds every other node, run from first to last. Each input of a step takes its value from an input node or from an earlier step, never from a later one. A step input that nothing feeds keeps its own value: an operation's default, or the current property of an object node's entity.

Object nodes (Line, Circle, Arc, Ellipse, Point, Polyline, Ray, Construction Line, Text, Multiline Text) own drawing entities; their ports are the entity's Properties rows. Computed rows such as Length, Angle or Delta X are outputs only. A list arriving at a step makes it run once per element, so an object node fed a list owns one entity per element.

Curves are values too: `{"curve": "line" | "circle" | "arc" | "polyline" | "ray" | "xline", …}` in plane coordinates, angles in degrees. The Curve nodes (Line Curve, Circle Curve, Arc Curve, Polyline Curve, Offset Curve) make and change them without touching the drawing, and every geometry node takes either a curve value or a drawn object. Only a **Draw Geometry** node draws: it owns one entity per curve on its `geometry` input, on its `layer` (the current layer when unset or unknown) and `color` (an ACI index, ByLayer when unset), updating those entities in place as the curves change and erasing them when the curves go. While the graph is open, curves computed by operation nodes show in the viewport as a preview overlay that is never part of the drawing.

Nodes have unique names (case-insensitive). Address a node by name or as `#id`, and a port as `"Node.port"`, split at the last dot, or as `{"node": "...", "port": "..."}`. A port may be named by its field (`end_x`) or its label (`End X`).

## Read

- `ocs_read` `op: graph_library` lists every node type with `source` (`builtin` or `object`; `composite` and `script` are reserved), `category`, `area` (`inputs` or `sequence`) and its ports with `kind` (`number`, `text`, `bool`, `point`, `curve`, `object` or `any`). Filter with `parameters.search` or pick one type with `parameters.name`.
- `ocs_read` `op: graph_state` returns `graph_revision`, the `inputs` and `sequence` nodes in order, with their names, types, groups, entities and ports (value, kind, `computed`, and `source` when linked), and the groups. `parameters.detail: summary` leaves out the ports.
- `ocs_read` `op: graph_validate` with `parameters.request` holding a `graph_apply` request applies it to a copy of the live graph and reports `valid`, per-change results, warnings and the links it would drop, without changing anything.

## Edit

`ocs_execute` `op: graph_apply` takes `changes`, applied in order and all-or-nothing as one graph undo step, then evaluated once. Later changes may name nodes earlier ones added. Pass `graph_revision` from `graph_state` to refuse the request if the graph changed meanwhile (`stale_graph`).

```json
{"op":"graph_apply","request_id":"wall-1","changes":[
  {"add":"Number","name":"Length","values":{"value":12}},
  {"add":"Number","name":"Thickness","values":{"value":0.3}},
  {"add":"Point","source":"builtin","name":"Start"},
  {"add":"Point","source":"builtin","name":"End"},
  {"link":"Length.value","to":"End.x"},
  {"add":"Line Curve","name":"Axis","group":"Wall"},
  {"link":"Start.point","to":"Axis.start"},
  {"link":"End.point","to":"Axis.end"},
  {"add":"Offset Curve","name":"Face","group":"Wall"},
  {"link":"Axis.curve","to":"Face.curve"},
  {"link":"Thickness.value","to":"Face.distance"},
  {"add":"Draw Geometry","name":"Draw face","group":"Wall"},
  {"link":"Face.curve","to":"Draw face.geometry"}
]}
```

| Change | Fields |
| --- | --- |
| `add` | node type; optional `name`, `values` (inputs only), `after` (`"start"` or a node of the same area), `group`, `source` (`object` or `builtin`, for names both use: Point, Text) |
| `adopt` | entity handle: an object node owning that drawn entity as it is; optional `name`, `after`, `group` |
| `set` | input port, `value` (inputs only) |
| `link` | output port, `to`: step input port |
| `unlink` | step input port |
| `move` | node, `after`: `"start"` or a node of the same area; optional `force` |
| `remove` | node; optional `force` |
| `rename` | node, `to` |
| `duplicate` | node (copied right after itself with its values and sources); optional `name` |
| `group` | list of nodes of one area, `name` (joins or creates); optional `color` |
| `ungroup` | group name |
| `clear` | `true`: empties the graph; drawn entities stay |

The result lists each change's outcome (`node`, `id`, `links_dropped`), `warnings`, the new `graph_revision` and the node names.

## Rules and errors

The first refused change stops the request and nothing is applied; the error carries its `index`, a `code` and often a `hint`.

- `manual_value_outside_inputs`: `set` or `values` on a step, or `link` into an input. Add an input node, set it and link it.
- `backward_link`: the source is not an input or an earlier step. Move one of them first.
- `would_break_links`: `remove` or `move` would clear inputs other nodes feed; `links` lists them. Repeat with `"force": true` to accept.
- `unknown_node`, `unknown_port`, `unknown_type`, `ambiguous_type`, `unknown_group`, `invalid_value`, `invalid_change`, `wrong_area`, `mixed_areas`.
- `entity_absent`, `entity_owned`, `unsupported_entity`, `invalid_handle` for `adopt`.
- `not_supported`: `{"add": {"script": …}}` and `{"add": {"composite": …}}` are reserved for script and composite nodes.

Warnings do not stop the request: `name_taken` (the node got a numbered name), `type_mismatch` (linked kinds differ), `not_linked`.

Prefer one `graph_apply` per coherent edit, validate large ones first with `graph_validate`, and read `graph_state` afterwards to check values. The person at the screen sees every change live and can undo a whole request with the graph's Undo button.
