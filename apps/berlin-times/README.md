# The Berlin Times

The Berlin Times is a private, landscape-only TRMNL X newspaper front page. It publishes exactly six concise news briefs with English headlines and summaries at 06:00 and 17:00 Europe/Berlin time.

```text
GitHub Actions → Exa Search API → GitHub Pages → TRMNL polling → TRMNL X
```

The page is deterministic HTML and Liquid, not an AI-generated image. Exa returns current news results with English headlines and summaries. The Rust generator independently enforces publication windows and URL safety, removes repeated and duplicate canonical URLs, keeps a fresh story at the lead, carries stories from only the currently deployed edition when needed, fits copy to the layout, processes one source photograph, and atomically publishes a static Pages artifact. TRMNL polls `edition.json` and renders the e-ink screen.

## Contents

- `generator/`: Rust CLI, typed API client, editorial validation, safe photo fetcher, and Pages writer.
- `plugin/`: official `trmnlp` project pinned to Framework 3.2.0.
- `generator/fixtures/`: deterministic research and image inputs used by CI.

## Local fixture generation

From the repository root:

```sh
cargo run --bin berlin-times -- generate \
  --output _site \
  --public-base-url https://pr0me.github.io/trmnl-apps/ \
  --fixture apps/berlin-times/generator/fixtures/valid-research.json \
  --fixture-image apps/berlin-times/generator/fixtures/lead.ppm \
  --at 2026-08-05T04:15:00Z
```

The output contains:

- `edition.json`
- `assets/lead-<edition_id>.jpg`
- `assets/berlin-times.css` and self-hosted OFL fonts
- `index.html`
- `robots.txt` and `.nojekyll`

The writer stages the complete site beside the destination and swaps directories only after every file succeeds. Generation errors therefore cannot create a partial output. On GitHub, a failed build never reaches the Pages deployment job, so the preceding deployment stays online.

## Live generation

Read a project-scoped Exa key without placing it in a command, file, or shell history, then omit the fixture flags:

```sh
umask 077
IFS= read -r -s EXA_API_KEY
export EXA_API_KEY
trap 'unset EXA_API_KEY' EXIT HUP INT TERM
cargo run --locked --release --bin berlin-times -- generate \
  --output _site \
  --public-base-url https://pr0me.github.io/trmnl-apps/
```

`EXA_API_BASE` exists for isolated API tests and defaults to `https://api.exa.ai/`. The CLI never logs request headers, environment contents, the raw key, summaries, or raw article text.

Morning searches use `wsj.com`, `nytimes.com`, `ft.com`, and `reuters.com` with the international query. Evening searches use `handelsblatt.com`, `tagesschau.de`, `ft.com`, and `dw.com` with the German and European query. Both request consequential current reporting from up to three distinct allowed sources, avoid duplicate events, and force a live crawl with `maxAgeHours: 0`.

The morning primary window begins at the preceding Berlin date's nominal 17:00 boundary. A deployed evening edition's actual generation time replaces that boundary only when it belongs to the expected preceding evening and precedes the new run. The evening primary window begins at current Berlin midnight. Both end at the run time plus 30 minutes. A final fallback can query the adjacent half-open interval from the preceding Berlin midnight to the primary start.

When the initial result set cannot supply six novel usable stories, one primary-window retry rotates away from sources associated with previously displayed URLs, or otherwise from the most represented source. Every request keeps at least two allowed domains. A prior-day request runs only when no novel lead exists or novel stories plus valid carry candidates still cannot fill six slots. Transport retries remain separate: connection failures, timeouts, `429`, and `5xx` responses retry the identical body up to three total attempts with bounded backoff and `Retry-After` support.

Each attempt logs credential-free structured fields for edition, attempt, window kind and bounds, domains, returned/usable/novel counts, rejection counts, request ID, and cost. Final selection logs novel and carried counts, provider and category coverage, and `lead_fresh=true`.

## Editorial contract

The generator rejects an edition unless it has:

- exactly six unique stories and canonical source URLs;
- at least one novel usable story, kept at index 0 as the non-carried textual lead;
- non-carried timestamps within the primary or explicit prior-day windows and no timestamp more than 30 minutes in the future;
- credential-free HTTPS article URLs from the globally recognized provider union, with live results restricted to the active morning or evening profile;
- English headlines, including translations requested from Exa for German titles;
- summaries fitted without story reordering to at most 36 lead words and 30 supporting words;
- exactly one correctly mapped publication source per story;
- all six stories ranked exactly once for photographic suitability.

Novel stories always precede and displace carried stories. When at least one novel story survives, any remaining slots are filled in published order from the single currently deployed `edition.json`; there is no archive or second history source and carried stories have no age limit. If no novel lead survives the primary, rotated, and prior-day searches, or novel plus valid carry content cannot total six, the deployed edition remains untouched.

Category diversity and direct Exa images are deterministic best-effort selection preferences. Provider diversity is a publication requirement: the dominant provider may supply at most four of the six stories. Primary-window candidates outrank prior-day candidates before optional preferences. Research pages are untrusted input. The photo fetcher tries valid Exa image URLs first and then each article's Open Graph image. It permits HTTPS on port 443 only, rejects credentials and private/reserved addresses, checks every redirect, caps redirect count, download time and bytes, validates image dimensions before decoding, crops to the template's fixed 560:367 aspect ratio, converts to grayscale, and writes a compressed JPEG. Photo selection never reorders the fresh textual lead; `lead_image.story_id` and its caption may identify another selected story. If no new image is usable, the preceding JPEG is reused only when its photographed story remains among the final six; otherwise no edition is published.

## Plugin preview

[`trmnlp`](https://github.com/usetrmnl/trmnlp) can serve the plugin entirely through Docker. From the repository root, run:

```sh
docker run --rm \
  --pull always \
  --publish 4567:4567 \
  --volume "$PWD/apps/berlin-times/plugin:/plugin" \
  --mount type=bind,source=/dev/null,target=/plugin/.trmnlp.yml,readonly \
  trmnl/trmnlp:latest serve --bind 0.0.0.0
```

Open `http://localhost:4567`, select the full layout, TRMNL X, landscape orientation, and a 4-bit palette. `trmnlp` fetches `https://pr0me.github.io/trmnl-apps/edition.json`; its startup log should report that URL with a `200` status. Use the preview's **Poll** button to fetch the latest published edition again. The read-only `/dev/null` mount hides the local `.trmnlp.yml`, so the server uses the production polling URL in `src/settings.yml` and the template's production CSS, fonts, and lead image URLs. Stop the server with `Ctrl-C`.

### Fixture and layout preview

The checked-in `.trmnlp.yml` instead points local previews at `_site` on port 8000 so deterministic fixture JSON, CSS, fonts, and images stay outside the uploaded plugin definition. Generate the fixture first, then run the following commands from the repository root. Keep the fixture server running in its own terminal:

```sh
python3 apps/berlin-times/plugin/test/fixture_server.py _site \
  --plugin-build apps/berlin-times/plugin/_build
```

In a second terminal:

```sh
docker run --rm \
  --pull always \
  -v "$PWD/apps/berlin-times/plugin:/plugin" \
  trmnl/trmnlp:latest lint
docker run --rm \
  --pull always \
  -v "$PWD/apps/berlin-times/plugin:/plugin" \
  trmnl/trmnlp:latest build --png --width 1872 --height 1404 --color-depth 4
```

On Linux, add `--add-host=host.docker.internal:host-gateway` to the render command. The browser assertion is:

```sh
docker run --rm --entrypoint /usr/local/bin/ruby \
  -e LAYOUT_URL=http://host.docker.internal:8000/__plugin__/full.html \
  -v "$PWD/apps/berlin-times/plugin:/plugin" \
  trmnl/trmnlp:latest test/layout_test.rb
```

It checks six unique articles, headlines, and summaries; one successfully loaded image; X viewport, masthead, dateline, and story bounds; unclamped summaries of at least 34px; borderless story modules; unclamped fixture copy; and a photo area no greater than 18% of the screen. The assertion writes the browser's final 1872×1404 PNG after the layout reaches a stable state; the preceding `trmnlp build` separately verifies 4-bit device rendering. CI repeats this with short, typical, and maximum-budget copy and uploads each result beside its checked-in golden PNG for visual diff review. Golden files are updated intentionally after review, never by CI.

## GitHub setup

1. Make `pr0me/trmnl-apps` public.
2. In **Settings → Pages**, choose **GitHub Actions** as the source.
3. In **Settings → Secrets and variables → Actions → Repository secrets**, create `EXA_API_KEY`. Do not create it as a repository variable.
4. Use a project-scoped key, set project budget alerts, and rotate it after any suspected exposure.
5. Run **Publish The Berlin Times edition** manually and review all six claims, source links, and the photo before relying on the schedule.

Only the generator step receives `EXA_API_KEY`. Pull-request CI has no secret access, fork pull requests never invoke edition publication, and the repository does not use `pull_request_target`. Pages deployment permissions are isolated to the deployment job. Publication may take several minutes after a successful run.

The secret-free monthly keepalive workflow updates `.github/schedule-heartbeat` on `main`, preventing GitHub from disabling scheduled workflows after prolonged repository inactivity. Standard GitHub Actions failure notifications and manual dispatch are the recovery path.

## TRMNL account and device setup

1. Confirm Developer perks are enabled for the X.
2. Wait for the first successful Pages deployment, then verify `https://pr0me.github.io/trmnl-apps/edition.json`.
3. From the repository root, create the host-side configuration directory and log in interactively through Docker:

   ```sh
   mkdir -p "$HOME/.config/trmnlp"
   docker run --rm -it \
     --pull always \
     --volume "$HOME/.config/trmnlp:/root/.config/trmnlp" \
     --volume "$PWD/apps/berlin-times/plugin:/plugin" \
     trmnl/trmnlp:latest login
   ```

   Visit the account URL printed by `trmnlp`, copy the API key, and paste it at the prompt. Login works through Docker because `-it` exposes the interactive prompt and the configuration mount persists the validated key at `~/.config/trmnlp/config.yml` after the container exits. No TRMNL credential belongs in the repository or GitHub.
4. Push the plugin with the same configuration mount:

   ```sh
   docker run --rm -it \
     --pull always \
     --volume "$HOME/.config/trmnlp:/root/.config/trmnlp" \
     --volume "$PWD/apps/berlin-times/plugin:/plugin" \
     trmnl/trmnlp:latest push
   ```

   The first push creates a private plugin and writes its server-assigned `id` back to `plugin/src/settings.yml`; keep that `id` so later pushes update the same plugin. Subsequent pushes ask before overwriting the server copy.
5. Confirm the polling URL is `https://pr0me.github.io/trmnl-apps/edition.json` with a 60-minute refresh interval.
6. Add The Berlin Times to the playlist as a full-screen item.
7. Set the X to landscape and 16 grayscale levels, then force a refresh.

Half and quadrant mashup templates intentionally show a full-screen-only notice. Portrait rendering is not supported in v1.

## Operational acceptance

Before declaring the installation stable:

- review one manually triggered real edition end to end;
- inspect the 1872×1404 4-bit artifact and physical X for clipping and source legibility;
- verify the credited image corresponds to its `lead_image.story_id`;
- observe successful morning and evening editions for seven consecutive days.

The public Pages JSON and current processed photograph are reachable by anyone with the URL. This is acceptable only for the private, personal prototype. A public Recipe or third-party app needs licensed/open imagery or a commercial image feed.
