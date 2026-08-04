# Named CNC lathe model support

Daedalus keeps generic `lathe`, `turning-center`, `mill-turn`, and `swiss-lathe`
profiles, while named profiles bind common operator language to a conservative
controller and work-envelope plan. A named profile is not machine certification:
every physical release still requires current workholding, tooling, offsets,
postprocessor, simulation, quality, and operator or automation-handoff evidence.

## Supported named models

| Model | Controller family | Planning envelope | Spindle planning range | Workholding notes |
| --- | --- | --- | --- | --- |
| Haas ST-20 | `haas-gcode` / Haas NGC | 330 mm maximum cutting diameter × 572 mm maximum cutting length | 1–4,000 rpm | 210 mm chuck, 64 mm published bar capacity, 12-station turret. The 330 mm diameter assumes the BOT turret; BMT65 and VDI configurations are smaller. |
| Tormach 15L Slant-PRO | `linuxcnc` / PathPilot | 254 mm X travel × 305 mm Z travel | 180–3,500 rpm across supported workholding | D1-4 spindle with removable 5C insert, optional 152.4 mm chuck, gang tooling/manual QCTP/8-position turret variants. Published spindle limits depend on workholding. |

The Tormach profile uses the existing `linuxcnc` controller token because
PathPilot belongs to the repository's LinuxCNC postprocessor family. The catalog
also exposes `controlName: PathPilot` so operators do not lose the product-facing
identity.

## Aliases and fleet IDs

### Haas ST-20

Accepted selection aliases include:

- `haas-st-20`
- `st-20`
- `st20`
- `haas-st20`
- `haas-st-20-series`

The default fleet entry is `haas-st-20-1`.

### Tormach 15L Slant-PRO

Accepted selection aliases include:

- `tormach-15l-slant-pro`
- `15l`
- `15l-slant-pro`
- `tormach-15l`
- `slant-pro`
- `tormach-slant-pro`

The default fleet entry is `tormach-15l-slant-pro-1`.

## Safety and release boundary

Named selection never authorizes a spindle start or physical cut. Before release,
retain evidence for at least:

1. exact machine and controller identity, firmware/control revision, and licensed options;
2. chuck, collet, jaws, turret/gang plate, tailstock, bar feeder, and stock-support configuration;
3. tool geometry, holders, stickout, offsets, wear values, coolant, and chip-clearance plan;
4. material lot, stock diameter/length, feeds, speeds, depth of cut, and surface-speed limits;
5. postprocessor identity plus static validation and simulation against the chosen profile;
6. safe bar support and exclusion-zone controls, especially for stock extending behind the spindle;
7. first-article inspection, dimensional evidence, and operator or automation handoff.

If the installed turret, chuck, collet, or workholding reduces the usable envelope,
record the smaller machine-specific limit. The catalog's published maximum is
never permission to assume that every installed configuration provides it.

## Adding another lathe model

1. Add one `LatheModelSpec` in `src/lathe_model_catalog.rs` with canonical and common aliases.
2. Use an existing reviewed controller family or add its postprocessor and preflight semantics first.
3. Keep the two-value envelope compatible with the current turning preflight path.
4. Add source notes for workholding-dependent limits rather than flattening them into one unsafe number.
5. Extend unit and HTTP catalog tests, the static contract, and this document.
6. Do not mark the machine ready until the ordinary release-gate evidence is complete.
