# Supported CNC turning-center models

Daedalus models turning processes structurally first: workholding, stock support,
turret/tooling, offsets, controller/postprocessor, feed and speed modes,
threading, part-off, simulation, inspection, and release evidence remain the
safety boundary for every lathe. The named-model catalog adds conservative
reference profiles for machines operators commonly identify by model name.

Named support is **not** remote-control certification. A catalog match may help
classification and planning, but execution remains blocked until the exact live
asset, controller, options, fixtures, tools, offsets, postprocessor, and first-
article evidence pass the normal turning preflight and release gates.

## Current named catalog

| Model token | Machine | Reference control | Max cut diameter × length | Chuck | Bar capacity | Spindle | Turret |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `haas-st-20` | Haas ST-20 | Haas G-code | 330 × 572 mm | 210 mm | 64 mm | 4000 rpm, 14.9 kW, 203 N·m | 12 station |
| `dn-solutions-lynx-2100b-fanuc` | DN Solutions Lynx 2100B | Fanuc G-code | 350 × 330 mm | 203.2 mm | configuration-specific | 4500 rpm, 15 kW, 169 N·m | 12 station |

### Why these two

- Haas describes the redesigned ST-20 series as its best-selling turning-center
  series. The standard ST-20 is a broadly recognizable two-axis production
  lathe with one fixed Haas control family and published envelope, chuck, bar,
  spindle, torque, and turret limits.
- DN Solutions lists the Lynx 2100/2600 series among its popular turning-center
  products and reports more than 25,000 Lynx-series sales. The 8-inch Lynx
  2100B provides a compact, high-production counterpoint to the ST-20.

Manufacturer references:

- Haas ST-20: <https://www.haascnc.com/machines/lathes/st/models/standard/st-20.html>
- DN Solutions Lynx 2100B: <https://www.dn-solutions.com/global/product/turning-center/2-axis-horizontal/lynx-2100-b.do>

## Alias and fleet behavior

`src/turning_model_catalog.rs` is the single source of truth. Each
`TurningMachineModelSpec` supplies:

- a canonical normalized model token and aliases;
- the generic machine kind used by the turning planner;
- the reference controller and possible controller families;
- published cutting envelope, chuck, spindle, power, torque, and turret limits;
- conservative materials and operations;
- release requirements and the manufacturer reference.

The service derives `<model>-1` default-fleet entries from the catalog. Model
aliases therefore flow through machine classification and appear consistently
in `/machines/catalog`, `/subtractive/catalog`, `/turning/catalog`, and
`/lathe/catalog` without copying the physical limits into several code paths.

Examples:

- `ST 20`, `st20`, and `haas-st20` resolve to `haas-st-20`.
- `lynx-2100b`, `dn-solutions-lynx-2100b`, and the legacy
  `doosan-lynx-2100b` name resolve to the Fanuc reference profile.

## Controller boundary

The Haas entry advertises `haas-gcode`; it must not be silently treated as a
Fanuc program.

The Lynx 2100 family is available with modern Fanuc and Siemens controls. The
named fleet entry is deliberately labeled
`dn-solutions-lynx-2100b-fanuc` and emits `fanuc-gcode`. Generic Lynx aliases
may select that reference for planning, but `requiresControllerConfirmation`
remains true and machine-ready release requires proving the exact installed
control. A Siemens-equipped machine must submit an explicit machine profile
with `siemens-sinumerik`; it must not execute the Fanuc reference output.

## Release evidence

At minimum, retain:

1. exact model, serial/asset identity, controller family/version, enabled options,
   parameters, and reviewed postprocessor;
2. chuck/collet/soft-jaw state, pressure, stock diameter, stick-out, runout,
   bar support, tailstock/sub-spindle/catcher state, and clearance;
3. turret station map, insert/tool geometry, tool-nose radius, geometry and wear
   offsets, tool-life state, and safe tool-change proof;
4. G50 spindle limit, CSS versus fixed-RPM state, feed-per-revolution mode,
   thread pitch/encoder synchronization, relief and spring-pass plan;
5. part-off support, catcher/ejection path, chip and coolant management, dry-run
   or simulation, first-piece inspection, and operator/automation signoff.

Optional Y-axis, live-tooling, sub-spindle, bar feeder, robot/APL, alternate
chuck/turret, or regional variants require an explicit submitted profile. The
base two-axis model must not silently inherit those capabilities.

## Adding another turning model

1. Add one `TurningMachineModelSpec` with conservative published limits and a
   manufacturer source.
2. Reuse a controller dialect already reviewed by the server, or add a separate
   controller-hardening change first.
3. Keep aliases unambiguous. When a model has multiple control families, expose
   the reference variant in the canonical token and require controller
   confirmation.
4. Extend module tests for aliases, default-fleet derivation, controller-specific
   accepted languages, catalog JSON, and physical limits.
5. Run formatting, the complete Rust test suite, HTTP/API contract checks, and
   formal release-gate checks before merging.
