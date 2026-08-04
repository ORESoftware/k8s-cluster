use std::{fs, path::PathBuf};

fn fixture() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/msgint-operator-config.yml");
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[test]
fn messaging_intel_negative_mutation_anchors_are_unique_and_effective() {
    let workflow = fixture();
    let anchors = [
        (
            "operator audit command",
            "          npm audit --audit-level=high\n",
            1,
        ),
        (
            "operator install/check order",
            "          npm ci --ignore-scripts\n          npm run check\n",
            1,
        ),
        (
            "immutable checkout action",
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            2,
        ),
        ("credential persistence control", "          persist-credentials: false\n", 2),
        (
            "operator job runner",
            "  operator_config:\n    runs-on: ubuntu-latest\n",
            1,
        ),
    ];

    for (label, anchor, expected_count) in anchors {
        assert_eq!(
            workflow.matches(anchor).count(),
            expected_count,
            "{label} fixture anchor drifted"
        );
    }

    let mutations = [
        (
            "extra command",
            "          npm audit --audit-level=high\n",
            "          npm audit --audit-level=high\n          npm publish\n",
            "npm publish",
        ),
        (
            "reordered commands",
            "          npm ci --ignore-scripts\n          npm run check\n",
            "          npm run check\n          npm ci --ignore-scripts\n",
            "npm run check\n          npm ci --ignore-scripts",
        ),
        (
            "mutable action",
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "actions/checkout@main",
            "actions/checkout@main",
        ),
        (
            "bracket secret",
            "          persist-credentials: false\n",
            "          persist-credentials: false\n        env:\n          MSGINT_META_ACCESS_TOKEN: ${{ secrets['PROD_TOKEN'] }}\n",
            "PROD_TOKEN",
        ),
        (
            "plain job environment",
            "  operator_config:\n    runs-on: ubuntu-latest\n",
            "  operator_config:\n    runs-on: ubuntu-latest\n    env:\n      NODE_ENV: test\n",
            "NODE_ENV: test",
        ),
    ];

    for (label, anchor, replacement, evidence) in mutations {
        let mutated = workflow.replacen(anchor, replacement, 1);
        assert_ne!(mutated, workflow, "{label} mutation became a no-op");
        assert!(
            mutated.contains(evidence),
            "{label} mutation did not produce its evidence marker"
        );
    }
}
