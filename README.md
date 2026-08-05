# TRMNL Apps

Private TRMNL plugins and their supporting generators.

The first app is [Berlin Times](apps/berlin-times/README.md), a twice-daily English news front page designed exclusively for TRMNL X in landscape mode.

## Repository checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
```

The pull-request workflow is secret-free. It also generates a fixed edition, lints the private plugin, renders a 1872×1404 4-bit PNG, and verifies the layout in a headless browser.

## License

Project code is MIT licensed. Bundled UnifrakturCook and Source Serif 4 font files are licensed under the SIL Open Font License; their license is distributed with the generator assets and every Pages artifact.

