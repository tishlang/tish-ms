# Releasing tish-ms

Same flow as tish-apple:

1. **Merge** a `feat:` / `fix:` PR to `main`. `release-prerelease.yml` cuts a prerelease `vX.Y.Z`.
2. **Promote** it (Releases → Edit → uncheck **Set as a pre-release**). That runs:
   - `crates-release.yml`: `tishlang_ms_common`, then `tishlang_windows`, to crates.io
     (`CARGO_REGISTRY_TOKEN` secret).
   - `npm-release.yml`: `@tishlang/tish-windows` to npm, once the shared crate is on crates.io.

Promote in the GitHub UI: editing the release through the API or `gh` doesn't start the
workflows.

## First release: the npm package by hand

npm's trusted publishing needs the package to exist, so the first version is published from a
logged-in machine, after `crates-release.yml` has published that version's crates:

```sh
npm login
node scripts/publish-npm.mjs --version 1.0.0
```

Then on npmjs.com: `@tishlang/tish-windows` → Settings → Trusted Publisher → GitHub Actions,
organization `tishlang`, repository `tish-ms`, workflow `npm-release.yml`. Later releases publish
themselves. `node scripts/publish-npm.mjs --version 0.0.0 --dry-run` stages and packs without
publishing (`dist-npm/tish-windows/`).
