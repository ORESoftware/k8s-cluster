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
