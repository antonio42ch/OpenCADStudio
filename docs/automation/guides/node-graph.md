# Node graph

A drawing's node graph builds geometry from values. It has two areas:

- **Inputs** hold the only values typed by hand: Number, Number Slider, Integer Slider, Text, Boolean and Table nodes. A Table's `table` is a list of rows (or text with a row per line, cells split by tabs, semicolons or commas); its columns come out as lists, named by the first row when `header` is on.
- The **node sequence** holds every other node, run from first to last. Each input of a step takes its value from an input node or from an earlier step, never from a later one. A step input that nothing feeds keeps its own value: an operation's default, or the current property of an object node's entity.

Object nodes (Line, Circle, Arc, Ellipse, Point, Polyline, Ray, Construction Line, Text, Multiline Text) own drawing entities; their ports are the entity's Properties rows. Computed rows such as Length, Angle or Delta X are outputs only. A list arriving at a step makes it run once per element, so an object node fed a list owns one entity per element.

Curves are values too: `{"curve": "line" | "circle" | "arc" | "polyline" | "ray" | "xline", …}` in plane coordinates, angles in degrees. Vectors are written like points, and planes as `{"plane": {"origin", "x_axis", "y_axis"}}`. The Vector nodes make vectors and planes (Plane, XY Plane, Curve Frame, Frame At Distance); the Curve nodes make and edit curves (Line, Circle, Arc and Polyline Curve, Line By Direction, Arc and Circle 3 Points, Rectangle, Polygon, Offset, Explode, Join, Reverse, Close, Extend, Fillet Corners, Closest Point, Sub Curve, Split At Distances, Offset Both Sides); the Transform nodes (Move, Rotate, Scale, Mirror, Orient, Linear, Polar and Curve Array) carry points, curves, planes and lists of them alike. None of them touches the drawing, and every geometry node takes either a curve value or a drawn object. Only a **Draw Geometry** node draws: it owns one entity per curve on its `geometry` input (lists, nested or not, draw every curve they hold), on its `layer` (the current layer when unset; a layer it names that the drawing lacks is created), `color` (an ACI index, ByLayer when unset), `linetype` (a name; one the drawing lacks is loaded from the standard catalog) and `lineweight` (millimetres, ByLayer when unset), updating those entities in place as the curves change and erasing them when the curves go. Three more nodes draw from their inputs the same way, each with `layer`, `color`, `linetype` and `lineweight`: **Hatch** fills closed curves (`boundary`; one hatch per region, so a list of an outline and its holes fills with the holes open; `pattern` `SOLID` or a pattern name, `scale`, `angle` in degrees), **Insert Block** places a block of the drawing (`block`, `point`, `rotation` in degrees, `scale`; one reference per point of a list), and **Aligned Dimension** measures from `from` to `to`, its line `offset` to their left, with an optional `text`. While the graph is open, curves computed by operation nodes show in the viewport as a preview overlay that is never part of the drawing.

Nodes have unique names (case-insensitive). Address a node by name or as `#id`, and a port as `"Node.port"`, split at the last dot, or as `{"node": "...", "port": "..."}`. A port may be named by its field (`end_x`) or its label (`End X`).

## Read

- `ocs_read` `op: graph_library` lists every node type with `source` (`builtin` or `object`; `composite` and `script` are reserved), `category`, `area` (`inputs` or `sequence`), a one-line `description` of what it does, and its ports with `kind` (`number`, `text`, `bool`, `point`, `curve`, `plane`, `object` or `any`; vectors are `point`). Filter with `parameters.search` or pick one type with `parameters.name`.
- `ocs_read` `op: graph_state` returns `graph_revision`, the `inputs` and `sequence` nodes in order, with their names, types, groups, entities and ports (value, kind, `computed`, and `source` when linked), and the groups. A step that gave no result lists why in `issues`: `Nothing feeds <input>`, `The list on <input> is empty`, `No result with these values` (a size of zero, coincident points…), or, for a composite, the step inside and its issue. Read them after an apply and fix what they name. `parameters.detail: summary` leaves out the ports.
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

**Region Union**, **Region Difference** and **Region Intersection** combine the closed curves of `a` and `b` into `regions` (each a list of its outline, counter-clockwise, and its holes, clockwise; arcs become chords) and their net `area`. **Choose** picks one of five options by `index`.

**Expression** evaluates a formula of `a`, `b`, `c`, `d`, `x` and `y` with `+ - * / % ^`, comparisons, `and`/`or`/`not` (or `&& || !`), `c ? a : b` and functions such as `sqrt`, `sin` (degrees), `min`, `max`, `round(x, digits)`, `clamp` and `if`; a comparison gives true or false, and true counts as 1. **Filter** splits a list by a condition on each item (`x` the item or a curve's length, `px`/`py`/`pz` a point's coordinates, `i` the index, `n` the count) into `in`, `out` and the `index` of the items kept. **Sort By Key** sorts a list by a parallel list of keys, **Repeat List** repeats or cycles a whole list, and **Zip** joins up to four lists into rows. **Format Text** fills a template: `{a}`…`{d}` take the inputs, `{a:000}` pads a number to three digits and `{a:.2}` rounds it, so `P-{a:00}` over a list numbers it `P-01`, `P-02`…

**Layer Objects** reads the drawn objects on a `layer` (of a `type` such as `Line` when given) as `objects`, their `curves` and a `count`, leaving out what the graph draws. The graph runs again by itself after an edit changes what such a node reads, so hand-drawn axes or outlines on a layer drive the graph; undo and redo do not re-run it.

Results go out as drawings and files. **Draw Table** draws `rows` (lists of cells, such as **Zip** gives) as a table of lines and texts with its top left corner at `point`, `height` the text height and `width` a fixed column width (columns fit their texts when unset). **Curve Label** writes `text` on each `curve` at a `distance` along it (the middle when unset) and an `offset` to its left, turned to follow it and kept readable. **Group By** gathers `values` by `keys` into each key's sum, count and values: lengths per layer, areas per kind. **Read CSV** reads a CSV or text table (its `path` relative to the drawing's folder) as a Table does, and **Write CSV** writes `rows` as CSV to a `.csv` or `.txt` `path`; a file written and read in one graph is read on the next run.

Every drawing node takes `data`, text such as `kind=door; segment=A` it stores on its entities (XDATA under `OCS_DATA`); **Tag Objects** writes it onto drawn objects it does not own, such as those a Layer Objects node reads, and **Object Data** reads one `key` back (all pairs when empty), so data sorts objects through Filter or Group By.

For profiles, **Distance Along Curve** gives a point's station along an alignment, its signed `offset` (left positive), the foot point and the curve's angle there, and **Interpolate Values** reads a `y` at an `x` from two parallel lists, such as an elevation at a station from a profile's stations and elevations.

A formula or condition that does not read shows why in the node's issues, such as "The formula does not read: unknown name `q`".

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
