# Sonus Auris production hosting

The authoritative public site is `https://sonusauris.app/` and is built from the
`main` branch of this repository by `.github/workflows/deploy.yml`.

## Required GitHub Pages settings

1. Set **Build and deployment → Source** to **GitHub Actions**.
2. Set the custom domain to `sonusauris.app` and enable HTTPS enforcement after
   GitHub reports the DNS check as successful.
3. Ensure the `github-pages` environment does not require an unattended manual
   approval for normal production releases.
4. Keep `public/CNAME` set to exactly `sonusauris.app`.

The workflow builds with:

```text
SONUS_AURIS_SITE_URL=https://sonusauris.app
SONUS_AURIS_SITE_BASE=/
```

This is intentionally a custom-domain root deployment, not the historical
`sonus-auris.github.io/sonus-auris-site.web/` project path.

## Release verification

Before deployment, `npm run check` builds the exact production artifact and
verifies:

- `dist/CNAME` declares the custom domain;
- generated HTML does not contain the legacy GitHub Pages host or project path;
- canonical URLs, when present, remain under `https://sonusauris.app`;
- privacy and account-deletion pages contain the approved publisher details;
- launch placeholders and broken internal links are absent.

After deployment, run the browser and store-compliance suites in
`sonus-auris/sonus-auris-e2e` against `https://sonusauris.app`.

## Rollback

Revert the release commit on `main` through a pull request. The resulting push
starts the same Pages workflow and republishes the prior source state. Do not
change DNS or detach the custom domain for a routine content rollback.

If the Pages deployment itself is unhealthy, preserve the custom-domain DNS,
inspect the `github-pages` environment and workflow run, and redeploy the last
known-good commit with `workflow_dispatch`.
