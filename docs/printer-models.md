# Supported FDM printer models

Most of the fabrication server describes machines **structurally** (build
volume, nozzle, flow, controller) rather than by brand. The named-model catalog
in [`src/printer_model_catalog.rs`](../src/printer_model_catalog.rs) is the
exception: it pins specific vendor models the service commits to supporting so a
caller can reference a machine by name (`"creality-k1"`, `"bambu-x1c"`) and the
FDM catalog can advertise each model's real envelope, thermals, and firmware
dialect.

## How a named model is wired in

Each `FdmPrinterModelSpec` flows through three places:

1. **Classification** — `machine_class()` resolves a model token (or alias) to
   its canonical `machine_kind`, so `"creality-k2"` classifies as additive/
   multi-material without the caller spelling out the kind.
2. **Default fleet** — `printer_model_fleet_machines()` turns every spec into a
   `MachineProfile` (`<model>-1`), so the models appear in `/machines/catalog`,
   `/printers/catalog`, and `/fdm-printer/catalog` and can be planned against.
3. **Instruction dialect** — `firmware_gcode_dialect()` maps the controller to
   the G-code flavor the model actually consumes:

   | controller | dialect        | example models |
   |------------|----------------|----------------|
   | `klipper`  | `klipper-gcode`| Creality K1/K2/K1C, Elegoo Centauri Carbon |
   | `bambu`    | `bambu-gcode`  | Bambu A1 family, P1S, X1C, H2D |
   | `marlin`   | `marlin-gcode` | Prusa CORE One (Buddy firmware) |
   | `reprap`   | `reprap-gcode` | — |
   | `flashforge` | `flashforge-gcode` | FlashForge Adventurer 5M family |

## Catalog (as of 2026-07)

Envelope is X×Y×Z mm; "flow" is the max volumetric flow (mm³/s) used as a
preflight ceiling; "mats" is max simultaneous materials.

| Model | Vendor | Kind | Ctrl | Kinematics | Envelope | Nozzle°C | Bed°C | Flow | Encl. | Mats |
|-------|--------|------|------|-----------|----------|---------|------|------|-------|------|
| creality-k1 | Creality | fdm | klipper | CoreXY | 220×220×250 | 300 | 100 | 32 | yes | 1 |
| creality-k2 | Creality | multi | klipper | CoreXY | 260×260×260 | 300 | 100 | 32 | yes | 16 |
| creality-k1c | Creality | fdm | klipper | CoreXY | 220×220×250 | 300 | 100 | 32 | yes | 1 |
| bambu-a1 | Bambu Lab | fdm | bambu | Cartesian | 256×256×256 | 300 | 100 | 28 | no | 1 |
| bambu-a1-combo | Bambu Lab | multi | bambu | Cartesian | 256×256×256 | 300 | 100 | 28 | no | 4 |
| bambu-a1-mini | Bambu Lab | fdm | bambu | Cartesian | 180×180×180 | 300 | 80 | 28 | no | 1 |
| bambu-a1-mini-combo | Bambu Lab | multi | bambu | Cartesian | 180×180×180 | 300 | 80 | 28 | no | 4 |
| bambu-p1s | Bambu Lab | multi | bambu | CoreXY | 256×256×256 | 300 | 100 | 32 | yes | 16 |
| bambu-x1-carbon | Bambu Lab | multi | bambu | CoreXY | 256×256×256 | 300 | 110 | 32 | yes | 16 |
| bambu-h2d | Bambu Lab | multi | bambu | CoreXY | 325×320×325 | 350 | 120 | 40 | yes | 16 |
| prusa-core-one | Prusa Research | fdm | marlin | CoreXY | 250×220×270 | 290 | 120 | 24 | yes | 1 |
| elegoo-centauri-carbon | Elegoo | fdm | klipper | CoreXY | 256×256×256 | 320 | 110 | 25 | yes | 1 |
| anycubic-kobra-3-combo | Anycubic | multi | klipper | Cartesian | 250×250×260 | 300 | 110 | 18 | no | 8 |
| anycubic-kobra-s1-combo | Anycubic | multi | klipper | CoreXY | 250×250×250 | 320 | 120 | 18 | yes | 8 |
| qidi-q1-pro | QIDI Tech | fdm | klipper | CoreXY | 245×245×240 | 350 | 120 | 24 | yes | 1 |
| qidi-plus4 | QIDI Tech | fdm | klipper | CoreXY | 305×305×280 | 370 | 120 | 24 | yes | 1 |
| flashforge-adventurer-5m | FlashForge | fdm | flashforge | CoreXY | 220×220×220 | 280 | 100 | 28 | no | 1 |
| flashforge-adventurer-5m-pro | FlashForge | fdm | flashforge | CoreXY | 220×220×220 | 280 | 100 | 32 | yes | 1 |

Notable capabilities:

- **Bambu H2D** — 2025 dual-nozzle flagship (two hotends, soluble support or two
  materials per layer), 65 °C active chamber, 350 °C hotend for CF/GF.
- **Prusa CORE One** — Prusa's first fully-enclosed CoreXY; Buddy firmware is
  Marlin-derived, so it advertises `marlin-gcode`, not `klipper`. Base unit is
  single-material; MMU3 (5) / INDX (8) add multi-material.
- **Creality K1C / Elegoo Centauri Carbon** — enclosed CoreXY with hardened
  nozzles for carbon-fiber/PA out of the box.
- **Anycubic Kobra 3 Combo / Kobra S1 Combo** — Kobra OS
  (GoKlipper-family) systems with one ACE Pro for four-color or two units for
  eight-color printing. The Kobra 3 is an open Cartesian machine; the S1 is an
  enclosed CoreXY with a 320 °C hotend and 120 °C bed.
- **QIDI Q1 Pro / Plus4** — open-source Klipper CoreXY machines with active
  chamber heat. Plus4 expands the envelope to 305×305×280 mm and reaches
  370 °C; optional QIDI Box support is not folded into the single-material base
  profile.
- **FlashForge Adventurer 5M / 5M Pro** — 220 mm-cube CoreXY machines using
  FlashPrint/Orca-Flashforge G-code. The Pro adds the enclosure, camera, air
  filtration, ABS/ASA support, and a published 32 mm³/s flow ceiling.

Multi-material models advertise the AMS/MMU job languages
(`ams-mmu-job`, `multi-material-fdm-job`) in addition to their G-code dialect.

## Where these surface over HTTP

- `GET /fdm-printer/catalog` → `supportedPrinterModels` (full specs) +
  `supportedPrinterModelCount`, and each `<model>-1` under `fdmPrinters`.
- `GET /printers/catalog` and `GET /machines/catalog` include the fleet entries.
- `POST /printing/preflight` validates a submitted machine/material/profile; the
  models here are planning profiles, not a bypass for preflight or operator
  release.

## Adding a model

Append an `FdmPrinterModelSpec` to `FDM_PRINTER_MODEL_SPECS`. Nothing else needs
editing — classification, the default fleet, and the catalog derive from it.
Add the model to the unit tests in `src/tests.rs`
(`*_models_resolve_and_join_the_default_fleet` /
`fdm_printer_catalog_advertises_*`) and, for full-stack coverage, to
`src/e2e_tests.rs`. Use the model's real published envelope/thermals; the
volumetric-flow figure is the hotend ceiling used to gate preflight, so prefer a
slightly conservative value over an optimistic one. `supportedPrinterModelCount`
is derived (`.len()`), so no count constant needs bumping.
