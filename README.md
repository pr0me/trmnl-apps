# TRMNL Apps

Private TRMNL plugins and their supporting generators, maintained in one repository with app-scoped builds, deployment paths, and GitHub Actions.

- [The Berlin Times](apps/berlin-times/README.md) is a twice-daily news front page backed by a Rust generator and GitHub Pages.
- [Berlin Family Dashboard](apps/family-dashboard/README.md) combines configurable local weather, a shared calendar, and two public-transport directions in one serverless plugin.

Each app owns its code, plugin definition, fixtures, tests, and generated output beneath `apps/<app>`. Host-specific deployment bundles live beneath `ops/<app>`. The Berlin Times publication workflow remains the repository's only Pages deployer. Family Dashboard fetches its sources inside TRMNL Serverless and neither reads nor writes `_site`.

## Repository checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
node --test apps/family-dashboard/plugin/test/transform_test.js
```

Both pull-request workflows are secret-free and path-filtered. A change contained in one app runs only that app's workflow. Each plugin is mounted from its own directory for `trmnlp lint`, `build`, and `push`, so its `settings.yml`, server-assigned plugin ID, cache, and build output cannot affect the other app.

## License

Project code is MIT licensed. Bundled UnifrakturCook and Source Serif 4 font files are licensed under the SIL Open Font License; their license is distributed with the generator assets and every Pages artifact.
