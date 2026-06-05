import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/dd-next-runtime/kustomization.yaml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('fabrication CAD source intake contract separates native CAD from verified neutral exports', async () => {
  const contract = await readRepoFile('docs/fabrication-cad-source-intake.md');
  const remoteReadme = await readRepoFile('remote/readme.md');
  const source = await readRepoFile('remote/deployments/fabrication-server-rs/src/main.rs');
  const fabricationReadme = await readRepoFile('remote/deployments/fabrication-server-rs/readme.md');
  const apiDocs = JSON.parse(
    await readRepoFile('remote/deployments/fabrication-server-rs/generated/api-docs.json'),
  );
  const natsSchema = await readRepoFile('remote/libs/nats/subject-defs/schema/fabrication.schema.json');
  const natsTypeScript = await readRepoFile(
    'remote/libs/nats/subject-defs/generated/typescript/index.ts',
  );
  const natsRust = await readRepoFile('remote/libs/nats/subject-defs/generated/rust/src/lib.rs');
  const natsReadme = await readRepoFile('remote/libs/nats/subject-defs/readme.md');
  const sharedSchema = await readRepoFile(
    'remote/libs/interfaces/shared/schema/fabrication-cad-conversion.schema.json',
  );
  const sharedSchemaIndex = await readRepoFile('remote/libs/interfaces/shared/schema/index.json');
  const sharedPackageJson = await readRepoFile('remote/libs/interfaces/shared/package.json');
  const sharedTypeScript = await readRepoFile(
    'remote/libs/interfaces/shared/generated/typescript/index.ts',
  );
  const sharedRust = await readRepoFile('remote/libs/interfaces/shared/generated/rust/src/lib.rs');
  const sharedReadme = await readRepoFile('remote/libs/interfaces/shared/readme.md');
  const sharedRequestExample = await readRepoFile(
    'remote/libs/interfaces/shared/examples/fabrication-design-conversion-request.json',
  );
  const sharedResultExample = await readRepoFile(
    'remote/libs/interfaces/shared/examples/fabrication-design-conversion-result.json',
  );

  assert.match(contract, /# Fabrication CAD Source Intake Contract/);
  assert.match(contract, /not as automatically machine-ready\s+geometry/);
  assert.match(contract, /STEP, STEP assembly, STL, 3MF, DXF, CAM setup JSON/);
  assert.match(contract, /sheet nesting JSON, assembly graph JSON/);
  assert.match(contract, /SOLIDWORKS `\.sldprt`\/`\.sldasm`/);
  assert.match(contract, /PTC Creo or Pro\/Engineer `\.prt`\/`\.asm`/);
  assert.match(contract, /Siemens NX `\.prt`/);
  assert.match(contract, /CATIA `\.CATPart`\/`\.CATProduct`/);
  assert.match(contract, /Fusion `\.f3d`\/`\.f3z`/);
  assert.match(contract, /Onshape documents/);
  assert.match(contract, /FreeCAD `\.FCStd`/);
  assert.match(contract, /OpenSCAD `\.scad`/);
  assert.match(contract, /parametric CSG JSON/);
  assert.match(contract, /Blender `\.blend`/);
  assert.match(contract, /ZBrush `\.ztl`\/`\.zpr`/);
  assert.match(contract, /G-code dialects, printer jobs, resin jobs, powder-bed jobs/);

  assert.match(contract, /"designInputs": \[/);
  assert.match(contract, /"fileName": "gearbox-housing\.sldprt"/);
  assert.match(contract, /"format": "SLDPRT"/);
  assert.match(contract, /"sourceSystem": "SOLIDWORKS"/);
  assert.match(contract, /"format": "STEP"/);
  assert.match(contract, /"format": "3MF"/);
  assert.match(contract, /"sourceSystem": "PrusaSlicer"/);
  assert.match(contract, /`designInputReview`/);
  assert.match(contract, /`design-input-review` artifact/);
  assert.match(contract, /Every entry must declare at least one source identity field/);
  assert.match(contract, /role` and `notes` are supplemental/);
  assert.match(contract, /redacts URI userinfo, query strings, and fragments/);
  assert.match(contract, /ambiguous native\s+extensions such as bare `\.prt` remain blocked/);
  assert.match(contract, /"units=mm"/);
  assert.match(contract, /"toleranceMm=0\.05"/);

  assert.match(contract, /`supported-native-cad-translator-required`/);
  assert.match(contract, /keep `machineReady=false`/);
  assert.match(contract, /STEP or STEP assembly exports should carry unit/);
  assert.match(contract, /STL and 3MF exports should carry mesh repair/);
  assert.match(contract, /DXF and sheet nesting exports should carry closed-profile/);
  assert.match(contract, /content hashes and exporter\s+versions/);
  assert.match(contract, /Onshape documents should record document/);

  assert.match(contract, /licensed, isolated workers for proprietary formats/);
  assert.match(contract, /vendor APIs where available/);
  assert.match(contract, /deterministic open-source workers for FreeCAD and OpenSCAD/);
  assert.match(contract, /Blender headless workers/);
  assert.match(contract, /ZBrush and sculpting sources as artistic mesh inputs/);
  assert.match(contract, /dd\.remote\.fabrication\.design\.conversion\.requests/);
  assert.match(contract, /dd-fabrication-design-converters/);
  assert.match(contract, /dd\.remote\.fabrication\.design\.conversion\.results/);
  assert.match(contract, /`FabricationDesignConversionRequest`/);
  assert.match(contract, /`FabricationDesignConversionResult`/);
  assert.match(contract, /sanitized source and artifact references/);
  assert.match(contract, /translator versions/);

  assert.match(remoteReadme, /deployments\/fabrication-server-rs/);
  assert.match(remoteReadme, /Native CAD\/source package intake rules/);
  assert.match(remoteReadme, /SOLIDWORKS, Creo\/Pro\/Engineer/);
  assert.match(remoteReadme, /NX, CATIA, Fusion, Onshape, FreeCAD, OpenSCAD, Blender, ZBrush/);
  assert.match(remoteReadme, /neutral STEP\/STL\/3MF\/DXF/);
  assert.match(remoteReadme, /\.\.\/docs\/fabrication-cad-source-intake\.md/);

  assert.match(source, /design_inputs: Option<Vec<DesignInputFile>>/);
  assert.match(source, /struct DesignInputFile/);
  assert.match(source, /file_name: Option<String>/);
  assert.match(source, /source_uri: Option<String>/);
  assert.match(source, /source_system: Option<String>/);
  assert.match(source, /struct DesignInputReview/);
  assert.match(source, /struct ReviewedDesignInput/);
  assert.match(source, /fn sanitize_design_source_uri/);
  assert.match(source, /fn token_parts_contain/);
  assert.match(source, /fn design_source_extension/);
  assert.match(source, /fn review_design_inputs/);
  assert.match(source, /dd\.fabrication\.design-input-review\.v1/);
  assert.match(source, /supported-native-cad-translator-required/);
  assert.match(source, /native CAD extension is ambiguous across CAD systems/);
  assert.match(source, /must include fileName, sourceUri, format, or sourceSystem/);
  assert.match(source, /sourceUri must include a path or identifier/);
  assert.match(source, /https:\/\/redacted@cad\.onshape\.com\/documents\/demo/);
  assert.match(source, /ptc-creo-pro-engineer-native/);
  assert.match(source, /solidworks-native/);
  assert.match(source, /autodesk-fusion-native/);
  assert.match(source, /siemens-nx-native/);
  assert.match(source, /catia-native/);
  assert.match(source, /onshape-cloud-document/);
  assert.match(source, /freecad-native/);
  assert.match(source, /openscad-source/);
  assert.match(source, /blender-native/);
  assert.match(source, /zbrush-native/);
  assert.match(source, /"designInputReview": response\.design_input_review/);
  assert.match(source, /"design-input-review"/);
  assert.match(source, /"designInputs"/);
  assert.match(source, /"designInputs may name native CAD, neutral geometry/);
  assert.match(source, /"optional": \["id", "fileName", "sourceUri", "format", "sourceSystem", "role", "notes"\]/);
  assert.match(source, /"format": "SLDPRT"/);
  assert.match(source, /"sourceSystem": "SOLIDWORKS"/);
  assert.match(source, /"sourceSystem": "PTC Creo"/);
  assert.match(source, /"sourceSystem": "PrusaSlicer"/);
  assert.match(source, /plan_reviews_professional_open_artistic_and_slicer_design_inputs/);
  assert.match(source, /design_input_review_hardens_ambiguous_extensions_and_redacts_uris/);

  assert.match(fabricationReadme, /Submitted `designInputs` are classified/);
  assert.match(fabricationReadme, /A `designInputReview` that recognizes Creo\/Pro\/ENGINEER/);
  assert.match(fabricationReadme, /Siemens NX, CATIA, Onshape/);
  assert.match(fabricationReadme, /FreeCAD, OpenSCAD, Blender, ZBrush/);
  assert.match(fabricationReadme, /PrusaSlicer\/OrcaSlicer\/Cura\/Bambu Studio/);
  assert.match(fabricationReadme, /retaining translator, topology, scale, slicer-profile/);
  assert.match(fabricationReadme, /source identity field/);
  assert.match(fabricationReadme, /Source URIs are stored without\s+userinfo, query strings, or fragments/);
  assert.match(fabricationReadme, /bare `\.prt` stay release-blocked/);
  assert.match(fabricationReadme, /`design-input-review`/);
  assert.match(fabricationReadme, /`mdp-request` artifacts include `designInputReview`/);

  assert.equal(apiDocs.service, 'fabrication-server-rs');
  assert.ok(
    apiDocs.routes.some(
      (route: { path: string; methods: string[]; handlers: string[] }) =>
        route.path === '/fabrication/plan' &&
        route.methods.includes('POST') &&
        route.handlers.includes('plan_http'),
    ),
    'generated API docs should expose the fabrication planning route',
  );
  assert.ok(
    apiDocs.routes.some(
      (route: { path: string; methods: string[]; handlers: string[] }) =>
        route.path === '/fabrication/schema' &&
        route.methods.includes('GET') &&
        route.handlers.includes('request_schema'),
    ),
    'generated API docs should expose the request schema route clients use to discover designInputs',
  );
  assert.ok(
    apiDocs.routes.some(
      (route: { path: string; methods: string[]; handlers: string[] }) =>
        route.path === '/fabrication/examples' &&
        route.methods.includes('GET') &&
        route.handlers.includes('examples'),
    ),
    'generated API docs should expose examples with designInputs payloads',
  );

  assert.match(natsSchema, /FabricationDesignConversionRequests/);
  assert.match(natsSchema, /dd\.remote\.fabrication\.design\.conversion\.requests/);
  assert.match(natsSchema, /dd-fabrication-design-converters/);
  assert.match(natsSchema, /FabricationDesignConversionResults/);
  assert.match(natsSchema, /dd\.remote\.fabrication\.design\.conversion\.results/);
  assert.match(
    natsTypeScript,
    /FABRICATION_DESIGN_CONVERSION_REQUESTS_SUBJECT = "dd\.remote\.fabrication\.design\.conversion\.requests"/,
  );
  assert.match(
    natsTypeScript,
    /FABRICATION_DESIGN_CONVERSION_REQUESTS_QUEUE_GROUP = "dd-fabrication-design-converters"/,
  );
  assert.match(
    natsTypeScript,
    /FABRICATION_DESIGN_CONVERSION_RESULTS_SUBJECT = "dd\.remote\.fabrication\.design\.conversion\.results"/,
  );
  assert.match(
    natsRust,
    /FABRICATION_DESIGN_CONVERSION_REQUESTS_SUBJECT: &str = "dd\.remote\.fabrication\.design\.conversion\.requests"/,
  );
  assert.match(
    natsRust,
    /FABRICATION_DESIGN_CONVERSION_REQUESTS_QUEUE_GROUP: &str = "dd-fabrication-design-converters"/,
  );
  assert.match(
    natsRust,
    /FABRICATION_DESIGN_CONVERSION_RESULTS_SUBJECT: &str = "dd\.remote\.fabrication\.design\.conversion\.results"/,
  );
  assert.match(natsReadme, /fabrication\.schema\.json/);
  assert.match(natsReadme, /### Fabrication CAD Conversion/);
  assert.match(natsReadme, /FABRICATION_DESIGN_CONVERSION_REQUESTS_SUBJECT/);
  assert.match(natsReadme, /dd\.remote\.fabrication\.design\.conversion\.requests/);
  assert.match(natsReadme, /FABRICATION_DESIGN_CONVERSION_REQUESTS_QUEUE_GROUP/);
  assert.match(natsReadme, /dd-fabrication-design-converters/);
  assert.match(natsReadme, /FABRICATION_DESIGN_CONVERSION_RESULTS_SUBJECT/);
  assert.match(natsReadme, /dd\.remote\.fabrication\.design\.conversion\.results/);
  assert.match(natsReadme, /SOLIDWORKS, Creo\/Pro\/Engineer, NX/);
  assert.match(natsReadme, /the core Rust planner/);

  assert.match(sharedSchemaIndex, /fabrication-cad-conversion\.schema\.json/);
  assert.match(sharedSchema, /FabricationDesignConversionRequest/);
  assert.match(sharedSchema, /dd\.remote\.fabrication\.design\.conversion\.requests/);
  assert.match(sharedSchema, /FabricationDesignConversionResult/);
  assert.match(sharedSchema, /dd\.remote\.fabrication\.design\.conversion\.results/);
  assert.match(sharedSchema, /FabricationDesignInputRef/);
  assert.match(sharedSchema, /FabricationDesignConversionTarget/);
  assert.match(sharedSchema, /FabricationNeutralExportArtifact/);
  assert.match(sharedSchema, /sourceUri/);
  assert.match(sharedSchema, /Userinfo, query strings, and fragments must be removed/);
  assert.match(sharedSchema, /SLDPRT, SLDASM, PRT, ASM, STEP, STL, 3MF, DXF/);
  assert.match(sharedSchema, /SOLIDWORKS, PTC Creo, Siemens NX, CATIA, Fusion, Onshape/);
  assert.match(sharedSchema, /FreeCAD, OpenSCAD, Blender, ZBrush/);
  assert.match(sharedSchema, /[Rr]equested STEP\/STL\/3MF\/DXF\/CAM setup\/sheet nesting/);
  assert.match(sharedSchema, /supported-native-cad-translator-required/);
  assert.match(sharedSchema, /translatorVersion/);
  assert.match(sharedSchema, /reviewMetadata/);

  assert.match(sharedTypeScript, /export type FabricationDesignConversionRequest = \{/);
  assert.match(sharedTypeScript, /designInputs: FabricationDesignInputRef\[\];/);
  assert.match(sharedTypeScript, /targets: FabricationDesignConversionTarget\[\];/);
  assert.match(sharedTypeScript, /resultSubject\?: string \| null;/);
  assert.match(sharedTypeScript, /export type FabricationDesignConversionResult = \{/);
  assert.match(sharedTypeScript, /machineReady: boolean;/);
  assert.match(sharedTypeScript, /translatorVersion\?: string \| null;/);
  assert.match(sharedTypeScript, /artifacts: FabricationNeutralExportArtifact\[\];/);
  assert.match(sharedRust, /pub struct FabricationDesignConversionRequest \{/);
  assert.match(sharedRust, /pub design_inputs: Vec<FabricationDesignInputRef>,/);
  assert.match(sharedRust, /pub targets: Vec<FabricationDesignConversionTarget>,/);
  assert.match(sharedRust, /pub result_subject: Option<String>,/);
  assert.match(sharedRust, /pub struct FabricationDesignConversionResult \{/);
  assert.match(sharedRust, /pub machine_ready: bool,/);
  assert.match(sharedRust, /pub translator_version: Option<String>,/);
  assert.match(sharedRust, /pub artifacts: Vec<FabricationNeutralExportArtifact>,/);
  assert.match(sharedReadme, /###? Fabrication CAD Conversion/);
  assert.match(sharedReadme, /FabricationDesignConversionRequest/);
  assert.match(sharedReadme, /FabricationDesignConversionResult/);
  assert.match(sharedReadme, /`@dd\/nats-subject-defs`/);
  assert.match(sharedReadme, /reviewed `designInputs`/);
  assert.match(sharedReadme, /examples\/fabrication-design-conversion-request\.json/);
  assert.match(sharedReadme, /examples\/fabrication-design-conversion-result\.json/);
  assert.match(sharedReadme, /pnpm --filter @dd\/shared-interfaces run validate:examples/);
  assert.match(sharedReadme, /rejects credential-bearing URIs/);
  assert.match(sharedPackageJson, /"validate:examples": "node src\/validate-examples\.mjs"/);
  assert.match(sharedPackageJson, /"test": "pnpm run validate:examples && node --test/);
  assert.match(sharedRequestExample, /dd\.fabrication\.design-conversion\.request\.v1/);
  assert.match(sharedRequestExample, /dd\.remote\.fabrication\.design\.conversion\.results/);
  assert.match(sharedRequestExample, /gearbox-housing\.sldprt/);
  assert.match(sharedRequestExample, /"format": "SLDPRT"/);
  assert.match(sharedRequestExample, /"sourceSystem": "SOLIDWORKS"/);
  assert.match(sharedRequestExample, /"format": "STEP"/);
  assert.match(sharedRequestExample, /"format": "3MF"/);
  assert.match(sharedRequestExample, /supported-native-cad-translator-required/);
  assert.match(sharedResultExample, /dd\.fabrication\.design-conversion\.result\.v1/);
  assert.match(sharedResultExample, /licensed-solidworks-step-exporter/);
  assert.match(sharedResultExample, /"translatorVersion": "2026\.2\.0\+adapter\.4"/);
  assert.match(sharedResultExample, /"artifactId": "housing-step-mm-artifact"/);
  assert.match(sharedResultExample, /"artifactId": "housing-3mf-verified-artifact"/);
  assert.match(sharedResultExample, /"machineReady": false/);
  assert.match(sharedResultExample, /machine-profile-release-required/);
});
