// ores-lint house config for the k8s-cluster repo.
//
// Shape: 91 Rust crates and 21 JS packages spread across catalog/, config/ and
// docs/, plus generated artifacts. The baseline would happily lint the
// generated output, so the ignore list is the important part here.
import oresConfig from './.ores-lint/eslint/base.mjs';

export default await oresConfig({
  ignores: [
    'artifacts/**',      // build output, regenerated
    'catalog/**/vendor/**',
    'docs/**/*.min.js',
    'flake/**',
    '**/testdata/**',    // fixtures are intentionally malformed
    '**/*.generated.js',
  ],
  rules: {
    // Cluster tooling shells out and handles process exits; these two are the
    // failure modes that actually bite here.
    'no-process-exit': 'off',
    'require-atomic-updates': 'warn',
  },
});
