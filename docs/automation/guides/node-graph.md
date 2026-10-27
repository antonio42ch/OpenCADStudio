# Node graph

A drawing's node graph builds geometry from values. It has two areas:

- **Inputs** hold the only values typed by hand: Number, Number Slider, Integer Slider, Text and Boolean nodes.
- The **node sequence** holds every other node, run from first to last. Each input of a step takes its value from an input node or from an earlier step, never from a later one. A step input that nothing feeds keeps its own value: an operation's default, or the current property of an object node's entity.

Object nodes (Line, Circle, Arc, Ellipse, Point, Polyline, Ray, Construction Line, Text, Multiline Text) own drawing entities; their ports are the entity's Properties rows. Computed rows such as Length, Angle or Delta X are outputs only. A list arriving at a step makes it run once per element, so an object node fed a list owns one entity per element.

Curves are values too: `{"curve": "line" | "circle" | "arc" | "polyline" | "ray" | "xline", …}` in plane coordinates, angles in degrees. Vectors are written like points, and planes as `{"plane": {"origin", "x_axis", "y_axis"}}`. The Vector nodes make vectors and planes (Plane, XY Plane, Curve Frame, Frame At Distance); the Curve nodes make and edit curves (Line, Circle, Arc and Polyline Curve, Line By Direction, Arc and Circle 3 Points, Rectangle, Polygon, Offset, Explode, Join, Reverse, Close, Extend, Fillet Corners, Closest Point, Sub Curve, Split At Distances, Offset Both Sides); the Transform nodes (Move, Rotate, Scale, Mirror, Orient, Linear, Polar and Curve Array) carry points, curves, planes and lists of them alike. None of them touches the drawing, and every geometry node takes either a curve value or a drawn object. Only a **Draw Geometry** node draws: it owns one entity per curve on its `geometry` input (lists, nested or not, draw every curve they hold), on its `layer` (the current layer when unset or unknown) and `color` (an ACI index, ByLayer when unset), updating those entities in place as the curves change and erasing them when the curves go. While the graph is open, curves computed by operation nodes show in the viewport as a preview overlay that is never part of the drawing.

Nodes have unique names (case-insensitive). Address a node by name or as `#id`, and a port as `"Node.port"`, split at the last dot, or as `{"node": "...", "port": "..."}`. A port may be named by its field (`end_x`) or its label (`End X`).

## Read

- `ocs_read` `op: graph_library` lists every node type with `source` (`builtin` or `object`; `composite` and `script` are reserved), `category`, `area` (`inputs` or `sequence`), a one-line `description` of what it does, and its ports with `kind` (`number`, `text`, `bool`, `point`, `curve`, `plane`, `object` or `any`; vectors are `point`). Filter with `parameters.search` or pick one type with `parameters.name`.
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
| `composite` | list of steps, `name`: one composite node in their place (see below) |
| `expand` | composite node: its steps back in its place, named as they were, in a group |
| `save_composite` | composite node; optional `name`: saved to the user's composite library |

The result lists each change's outcome (`node`, `id`, `links_dropped`), `warnings`, the new `graph_revision` and the node names.

## Composites

A composite is a graph of operation steps used as one node: the tool a combination of generic nodes becomes. `{"composite": ["Axis", "Face", "Rounded"], "name": "Wall outline"}` replaces those steps with one node where the last of them was. Every link from outside into the steps becomes one of its inputs (named `Node port`, e.g. `Axis start`), every link out of them one of its outputs; with nothing outside reading them, the last step's outputs are exposed. Steps that sat between them without feeding them move after it. Inputs-area nodes, object nodes and Draw Geometry stay outside, so a composite computes and the graph around it draws.

`save_composite` keeps it in the user's composite library, which every drawing's palette lists under Composites and `graph_library` reports with `source: "composite"`; `{"add": {"composite": "Wall outline"}}` adds a copy. `expand` opens one back into its steps to change it. Composites are saved inside the `.ocg` file, so a graph never depends on the library.

Along a curve, **Frame At Distance** gives the plane, point, tangent, perpendicular and tangent angle at each distance, so a Range of distances (with `include end` to close on the curve's length) places marks, labels or sections. **Split At Distances** cuts a curve at a list of distances into its pieces, with the `even` (1st, 3rd…) and `odd` (2nd, 4th…) pieces apart, and **Sub Curve** takes the part between two distances: a wall's stretches and openings, on straight or bent axes alike. **Offset Both Sides** gives a curve's two parallels at half a width and the closed `outline` between them, so `Split At Distances.even` → `Offset Both Sides.outline` → Draw Geometry draws a wall with its openings. **Format Number** writes numbers with fixed decimals, zero padding and digit groups: stations such as `0+030.00` (4 digits, groups of 3, separator `+`), or thousands such as `1,234.50`.

Nested lists are the graph's trees: Graft, Flatten Once, List Depth, Cross Reference, Weave, Shift List and Cull Pattern reshape them, and a list reaching a step's single-value input runs the step once per element, recursively.

## Rules and errors

The first refused change stops the request and nothing is applied; the error carries its `index`, a `code` and often a `hint`.

- `manual_value_outside_inputs`: `set` or `values` on a step, or `link` into an input. Add an input node, set it and link it.
- `backward_link`: the source is not an input or an earlier step. Move one of them first.
- `would_break_links`: `remove` or `move` would clear inputs other nodes feed; `links` lists them. Repeat with `"force": true` to accept.
- `unknown_node`, `unknown_port`, `unknown_type`, `ambiguous_type`, `unknown_group`, `invalid_value`, `invalid_change`, `wrong_area`, `mixed_areas`.
- `entity_absent`, `entity_owned`, `unsupported_entity`, `invalid_handle` for `adopt`.
- `invalid_composite`: a composite takes operation steps only (no inputs, objects or Draw Geometry), and no other step may both read from the gathered steps and feed them.
- `not_supported`: `{"add": {"script": …}}` is reserved for script nodes.

Warnings do not stop the request: `name_taken` (the node got a numbered name), `type_mismatch` (linked kinds differ), `not_linked`.

Prefer one `graph_apply` per coherent edit, validate large ones first with `graph_validate`, and read `graph_state` afterwards to check values. The person at the screen sees every change live and can undo a whole request with the graph's Undo button.
