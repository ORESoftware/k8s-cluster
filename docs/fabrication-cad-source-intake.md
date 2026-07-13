# Fabrication CAD Source Intake Contract

The Rust fabrication service should treat CAD files as source evidence that can
feed planning, validation, and learning, not as automatically machine-ready
geometry. Native CAD formats often require licensed kernels, cloud APIs, or
tool-specific exporters before downstream slicer, CAM, simulation, and
controller checks can prove that the job is ready to release.

## Format Tiers

| Tier | Formats | Service posture |
| --- | --- | --- |
| Verified neutral manufacturing inputs | STEP, STEP assembly, STL, 3MF, DXF, CAM setup JSON, sheet nesting JSON, assembly graph JSON | Can be attached directly to plans and design export bundles, then validated against material, machine, workholding, slicer, CAM, and operator evidence. |
| Native professional CAD sources | SOLIDWORKS `.sldprt`/`.sldasm`, PTC Creo or Pro/Engineer `.prt`/`.asm`, Siemens NX `.prt`, CATIA `.CATPart`/`.CATProduct`, Fusion `.f3d`/`.f3z`, Onshape documents | Accepted only as source packages until a trusted exporter produces verified neutral artifacts with units, configuration, version, and tolerance evidence. |
| Open and scriptable parametric sources | FreeCAD `.FCStd`, OpenSCAD `.scad`, parametric CSG JSON | Good candidates for deterministic headless conversion workers that regenerate STEP, STL, 3MF, or DXF from parameter sets. |
| Artistic or organic mesh sources | Blender `.blend`, ZBrush `.ztl`/`.zpr`, OBJ, sculpt meshes | Require mesh repair, scale, manifoldness, wall thickness, orientation, support, and tolerance review before printing or machining. |
| Existing controller or shop instructions | G-code dialects, printer jobs, resin jobs, powder-bed jobs, router profiles, setup sheets, operator checklists | Can be analyzed directly for failure boundaries, intervention requirements, and improvement drafts, but still remain non-machine-ready until release evidence is complete. |

## Request Shape

`/fabrication/plan` accepts a `designInputs` array alongside `parts`,
`machines`, and `existingInstructions`:

```json
{
  "designInputs": [
    {
      "id": "gearbox-housing-native",
      "fileName": "gearbox-housing.sldprt",
      "sourceUri": "s3://operator-controlled-bucket/path/gearbox-housing.sldprt",
      "format": "SLDPRT",
      "sourceSystem": "SOLIDWORKS",
      "role": "editable native source CAD",
      "notes": ["configuration=machined-print-hybrid", "units=mm"]
    },
    {
      "id": "gearbox-housing-step",
      "fileName": "gearbox-housing.step",
      "format": "STEP",
      "role": "verified neutral export candidate",
      "notes": ["generatedBy=licensed-solidworks-export-worker", "toleranceMm=0.05"]
    },
    {
      "id": "gearbox-print-project",
      "fileName": "gearbox-housing.3mf",
      "format": "3MF",
      "sourceSystem": "PrusaSlicer",
      "role": "slicer project evidence",
      "notes": ["mesh manifoldness checked", "printer profile pending release"]
    }
  ]
}
```

The response returns `designInputReview`, also retained as a
`design-input-review` artifact and embedded in `parametric-design` and
`mdp-request`. The review records the normalized source system, import strategy,
preferred neutral exports, slicer targets, and blockers for each input.
Every entry must declare at least one source identity field: `fileName`,
`sourceUri`, `format`, or `sourceSystem`. `role` and `notes` are supplemental
evidence only. Review output redacts URI userinfo, query strings, and fragments
before storing artifacts or publishing MDP requests, and ambiguous native
extensions such as bare `.prt` remain blocked until the source system or a
verified neutral export is supplied.

## Release Rules

- Native CAD packages without translator or verified neutral-export evidence
  remain `supported-native-cad-translator-required` and keep `machineReady=false`.
- STEP or STEP assembly exports should carry unit, configuration, and assembly
  reference evidence before subtractive CAM or hybrid assembly release.
- STL and 3MF exports should carry mesh repair, manifoldness, scale, wall
  thickness, orientation, support, and slicer profile evidence before additive
  release.
- DXF and sheet nesting exports should carry closed-profile, kerf, pierce,
  focus, gas or jet support, stock, and fixture evidence before sheet-cutting
  release.
- Native source revisions should be tied to content hashes and exporter
  versions so learning outcomes can distinguish design changes from process
  changes.
- Cloud-native sources such as Onshape documents should record document,
  workspace or version, element, and configuration identifiers instead of
  relying only on human-readable filenames.

## Adapter Strategy

- Use licensed, isolated workers for proprietary formats such as SOLIDWORKS,
  Creo or Pro/Engineer, NX, CATIA, and Fusion native files.
- Use vendor APIs where available, especially for Onshape document export and
  configuration resolution.
- Use deterministic open-source workers for FreeCAD and OpenSCAD conversion,
  with parameter snapshots captured in the request.
- Use Blender headless workers for `.blend` mesh extraction and mesh-review
  evidence, but require engineering tolerance review before machining or
  precision-fit printing.
- Treat ZBrush and sculpting sources as artistic mesh inputs unless explicit
  dimensional inspection evidence is attached.

## Async Worker Subjects

External CAD/CAM/slicer workers should consume conversion jobs from
`dd.remote.fabrication.design.conversion.requests` with queue group
`dd-fabrication-design-converters` and publish completed neutral-export evidence
to `dd.remote.fabrication.design.conversion.results`.

Those messages use the shared-interface payloads
`FabricationDesignConversionRequest` and `FabricationDesignConversionResult`.
They carry the reviewed `designInputs`, requested neutral export targets,
sanitized source and artifact references, source hashes, translator versions,
blocker notes, and generated STEP, STL, 3MF, DXF, CAM setup JSON, sheet nesting
JSON, or slicer-project evidence. The planner can then retain those results as
design evidence without making the core Rust service depend on proprietary CAD
kernels.

This keeps the system honest: it can receive and reason about native CAD source
packages, but it does not confuse proprietary-source possession with verified,
machine-ready manufacturing geometry.
