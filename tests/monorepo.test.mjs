import test from 'node:test';
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';

test('monorepo structural integrity', () => {
    // Check config
    assert.ok(fs.existsSync('monorepo.config.json'), 'monorepo.config.json is missing');
    const config = JSON.parse(fs.readFileSync('monorepo.config.json', 'utf8'));
    
    // Check apps folder
    assert.ok(fs.existsSync('apps'), 'apps directory is missing');
    
    // Check all apps in config exist in apps folder
    for (const app of config.apps) {
        assert.ok(fs.existsSync(path.join('apps', app)), `App ${app} is missing from apps/`);
    }
    
    // Check flake.nix
    assert.ok(fs.existsSync('flake.nix'), 'flake.nix is missing');
});

test('production deploy publishes and pins the complete runtime matrix', () => {
    const workflow = fs.readFileSync('.github/workflows/deploy.yml', 'utf8');
    const matrix = JSON.parse(fs.readFileSync('apps/gleam-lambda-runner/runtime-images/matrix.json', 'utf8'));
    assert.match(workflow, /SCINTILLA_K8S_BUILD_CONTEXT/);
    assert.match(workflow, /scintilla-runner/);
    assert.match(workflow, /runtime-images-docker\.e2e\.mjs/);
    assert.doesNotMatch(workflow, /imagetools inspect ghcr\.io\/gleam-lang\/gleam/);
    for (const runtime of matrix.runtimes) {
        const output = `runtime_${runtime.name}`;
        assert.ok(workflow.includes(output), `deploy output ${output} is missing`);
        assert.ok(
            workflow.includes(`SCINTILLA_RUNTIME_${runtime.name.toUpperCase()}_IMAGE`),
            `renderer input for ${runtime.name} is missing`,
        );
    }
});
