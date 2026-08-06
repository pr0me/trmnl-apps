# Berlin Times

Berlin Times is a private, landscape-only TRMNL X newspaper front page. It publishes exactly six concise English-language news briefs at 06:30 and 17:00 Europe/Berlin time.

```text
GitHub Actions → Exa Search API → GitHub Pages → TRMNL polling → TRMNL X
```

The page is deterministic HTML and Liquid, not an AI-generated image. Exa returns ten current news results and English summaries in one search. The Rust generator independently enforces freshness and URL safety, removes duplicates, selects six stories, fits copy to the layout, processes one source photograph, and publishes a static Pages artifact. TRMNL polls `edition.json` and renders the e-ink screen.

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
  --at 2026-08-05T04:30:00Z
```

The output contains:

- `edition.json`
- `assets/lead-<edition_id>.jpg`
- `assets/berlin-times.css` and self-hosted OFL fonts
- `index.html`
- `robots.txt` and `.nojekyll`

The writer stages the complete site beside the destination and swaps directories only after every file succeeds. Generation errors therefore cannot create a partial output. On GitHub, a failed build never reaches the Pages deployment job, so the preceding deployment stays online.

## Live generation

Set a project-scoped Exa key in the environment, then omit the fixture flags:

```sh
EXA_API_KEY=... cargo run --release --bin berlin-times -- generate \
  --output _site \
  --public-base-url https://pr0me.github.io/trmnl-apps/
```

`EXA_API_BASE` exists for isolated API tests and defaults to `https://api.exa.ai/`. The CLI never logs request headers, environment contents, the raw key, or raw article text.

One live `POST /search` requests ten news results from The New York Times, Financial Times, Tagesschau, Reuters, rbb24, The Wall Street Journal, and Handelsblatt. Its publication filters span the current Europe/Berlin calendar day with DST-correct UTC boundaries. Exa summaries and filters are advisory: the generator checks every result locally. Connection failures, timeouts, `429`, and `5xx` responses are retried up to three total attempts with bounded backoff and `Retry-After` support. Other API errors fail immediately and report Exa's request ID and error tag when available.

The generator logs the request ID, returned/usable/selected counts, provider and category coverage, and Exa's reported cost. Current documented pricing implies approximately $0.017 for a ten-result search with ten summaries, but Exa's reported request cost is authoritative.

## Editorial contract

The generator rejects an edition unless it has:

- exactly six distinct events;
- publication timestamps within the current Berlin calendar day and no more than 30 minutes in the future;
- canonical, credential-free HTTPS article URLs from the seven configured publications;
- no duplicate canonical URLs or likely same-event duplicates;
- locally derived English headlines of at most 12 words;
- summaries of at most 60 lead words and 45 secondary words;
- exactly one mapped publication source per story;
- all six stories ranked exactly once for photographic suitability.

Germany, technology, global economics, and global politics coverage; provider diversity; an Exa image; and URLs absent from the prior edition are deterministic best-effort selection preferences, not publication blockers. Research pages are untrusted input. The photo fetcher tries valid Exa image URLs first and falls back to each article's Open Graph image. It permits HTTPS on port 443 only, rejects credentials and private/reserved addresses, checks every redirect, caps redirect count, download time and bytes, validates image dimensions before decoding, crops to the template's fixed 560:367 aspect ratio, converts to grayscale, and writes a compressed JPEG. If no selected story yields a usable image, no edition is published.

## Plugin preview

The project’s `.trmnlp.yml` points local previews at `_site` on port 8000 so the fixture JSON, CSS, fonts, and image stay outside the uploaded plugin definition.

```sh
python3 apps/berlin-times/plugin/test/fixture_server.py _site \
  --plugin-build apps/berlin-times/plugin/_build
docker run --rm \
  -v "$PWD/apps/berlin-times/plugin:/plugin" \
  trmnl/trmnlp:latest lint
docker run --rm \
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

It checks six unique articles, headlines, and summaries; one successfully loaded image; X viewport and story bounds; unclamped fixture copy; and a photo area no greater than 18% of the screen. The assertion writes the final 1872×1404 screenshot and quantizes it to 4-bit grayscale after the browser reaches a stable layout. CI repeats this with short, typical, and maximum-budget copy and uploads each result beside its checked-in golden PNG for visual diff review. Golden files are updated intentionally after review, never by CI.

## GitHub setup

1. Make `pr0me/trmnl-apps` public.
2. In **Settings → Pages**, choose **GitHub Actions** as the source.
3. In **Settings → Secrets and variables → Actions → Repository secrets**, create `EXA_API_KEY`. Do not create it as a repository variable.
4. Use a project-scoped key, set project budget alerts, and rotate it after any suspected exposure.
5. Run **Publish Berlin Times edition** manually and review all six claims, source links, and the photo before relying on the schedule.

Only the generator step receives `EXA_API_KEY`. Pull-request CI has no secret access, fork pull requests never invoke edition publication, and the repository does not use `pull_request_target`. Pages deployment permissions are isolated to the deployment job. Publication may take several minutes after a successful run.

The secret-free monthly keepalive workflow updates `.github/schedule-heartbeat` on `main`, preventing GitHub from disabling scheduled workflows after prolonged repository inactivity. Standard GitHub Actions failure notifications and manual dispatch are the recovery path.

## TRMNL account and device setup

1. Confirm Developer perks are enabled for the X.
2. Wait for the first successful Pages deployment, then verify `https://pr0me.github.io/trmnl-apps/edition.json`.
3. From `apps/berlin-times/plugin`, run `trmnlp login`, then `trmnlp push`. No TRMNL credential belongs in GitHub.
4. Confirm the polling URL is `https://pr0me.github.io/trmnl-apps/edition.json` with a 60-minute refresh interval.
5. Add Berlin Times to the playlist as a full-screen item.
6. Set the X to landscape and 16 grayscale levels, then force a refresh.

Half and quadrant mashup templates intentionally show a full-screen-only notice. Portrait rendering is not supported in v1.

## Operational acceptance

Before declaring the installation stable:

- review one manually triggered real edition end to end;
- inspect the 1872×1404 4-bit artifact and physical X for clipping and source legibility;
- verify the credited image corresponds to its `lead_image.story_id`;
- observe successful morning and evening editions for seven consecutive days.

The public Pages JSON and current processed photograph are reachable by anyone with the URL. This is acceptable only for the private, personal prototype. A public Recipe or third-party app needs licensed/open imagery or a commercial image feed.
