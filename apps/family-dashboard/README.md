# Berlin Family Dashboard

Berlin Family Dashboard is a private, full-screen TRMNL X plugin for a two-item household playlist. It combines weather for Berlin 13055, the next five events from the shared Google calendar named `Familie`, and the next two tram departures in each direction from Simon-Bolivar-Straße.

```text
Google Calendar → hidden native TRMNL instance ┐
Open-Meteo ────────────────────────────────────┼→ TRMNL Serverless → Liquid → TRMNL X
BVG transport.rest ────────────────────────────┘
```

No separately hosted application or new GitHub Pages deployment is required. The plugin's Node serverless function performs four concurrent, bounded requests, normalizes the results, and returns only the view model needed by Liquid. The existing Berlin Times publisher remains independent.

## Product and layout contract

The only supported target is TRMNL X at 1872×1404, landscape, with 16 grayscale levels. Half-horizontal, half-vertical, quadrant, portrait, and smaller-device layouts intentionally show a full-screen-only notice.

The screen uses a high-contrast Berlin wayfinding-board visual language:

- 54% left column: weather;
- upper third of weather: today's condition glyph, current temperature, humidity, and daily maximum;
- lower two thirds of weather: condition glyph and maximum temperature for each of the next three days;
- 46% right column, upper 58%: five chronologically next events, regardless of week boundary;
- right column, lower 42%: two non-interleaved departure boards labelled `Innenstadt` and `Hohenschönhausen`.

All labels and dates are German, times use 24-hour notation, and all date/time calculations use `Europe/Berlin`. Weather glyphs are inline SVG so the plugin has no font-icon or image dependency. Long event titles may use two lines and then truncate; departure destinations use a single ellipsized line so their absolute departure times stay dominant.

## Constraints and design decisions

### Monorepo isolation

The deployable root is `apps/family-dashboard/plugin`, not the repository root. `trmnlp` sees only that directory through its Docker volume. Its `.trmnlp.yml`, `src/settings.yml`, `_build`, tests, and eventual server-assigned plugin ID are therefore isolated from `apps/berlin-times/plugin`.

The GitHub workflows are path-filtered:

- `.github/workflows/ci.yml` owns Berlin Times Rust, Pages-fixture, and plugin checks;
- `.github/workflows/family-dashboard-ci.yml` owns this app's transform, fixture, lint, and rendering checks;
- `.github/workflows/edition.yml` remains the only Pages publishing workflow and still owns Berlin Times only.

Shared root files such as `Cargo.toml` trigger Berlin Times CI because they affect its Rust generator. Family Dashboard has no Rust workspace member and does not disturb the existing dependency graph or publication schedule.

### Data ownership and authentication

TRMNL's native Google Calendar integration remains the OAuth owner. This plugin reads that native instance through TRMNL's authenticated [data-only endpoint](https://docs.usetrmnl.com/go/private-api/fetch-plugin-content). The source calendar instance must stay on an active playlist but can be hidden; TRMNL documents that hidden state as the way to keep data-only sources refreshing without showing them.

The Family Dashboard private plugin has two required custom fields:

- `calendar_plugin_setting_id`: integer from the hidden calendar instance URL, preconfigured as `409973`;
- `trmnl_user_api_key`: user API key from the TRMNL Account page.

The key is supplied to `https://trmnl.com/api/plugin_settings/<id>/data` only. It is never logged, checked in, sent to Open-Meteo or transport.rest, or returned in the serverless result. Calendar event titles necessarily pass through TRMNL because TRMNL both synchronizes and renders the plugin; do not use this design if that trust boundary is unacceptable.

### Serverless runtime

TRMNL Serverless provides Node with network access and currently documents a 128 MB / 5 second execution budget. The implementation uses no packages, runs all four requests concurrently, applies a 1.5 second timeout per request, caps each response at 250 KB, and avoids returning raw upstream payloads. Calendar, weather, and each direction fail independently. A panel shows a local unavailable state when one source fails; the run fails only when all four sources are unavailable.

The `polling` strategy remains necessary to trigger serverless execution. Its production polling URL is TRMNL's small example JSON payload; the returned data is not used. Local `.trmnlp.yml` replaces this with the deterministic fixture server. Do not upload `.trmnlp.yml`: `trmnlp push` uploads the files under `src`, while the dotfile remains a local development override.

### Source limits

- Weather uses [Open-Meteo's forecast API](https://open-meteo.com/en/docs) at fixed coordinates `52.54,13.50`, representing ZIP 13055. It requests current `temperature_2m` and `relative_humidity_2m`, plus four days of daily `weather_code` and `temperature_2m_max`. WMO codes collapse into the five stable display states sunny, partly cloudy, cloudy/fog, rain/thunderstorm, and snow. Open-Meteo requires attribution, which appears in the footer.
- Departures use [`v6.bvg.transport.rest`](https://v6.bvg.transport.rest/), which exposes BVG realtime data without an API key and documents a limit of 100 requests per minute. The origin is `900150511`; direction filters are `900150510` toward Innenstadt and `900150512` toward Hohenschönhausen. Each request is tram-only, asks for six candidates in the next 60 minutes, drops cancelled or elapsed results, sorts by effective realtime `when` with `plannedWhen` fallback, and keeps two. The display uses absolute times because a 15-minute e-ink refresh makes countdowns misleading.
- Google Calendar data comes from the configured native TRMNL instance. Use `Rolling Month`: the current official implementation fetches 30 days ahead and supports multi-day events. The dashboard can only select from events that native instance provides, so an unusually sparse calendar with fewer than five events in that horizon will show fewer than five. Ongoing all-day and multi-day events remain eligible; elapsed timed events do not.

None of the unauthenticated upstream endpoints carries an availability SLA. At a 15-minute plugin refresh, the normal load is four serverless source requests per refresh, including two BVG requests—well below the published transport.rest limit for one household installation.

## TRMNL account setup

### 1. Create the hidden calendar source

1. Open **Plugins → Google Calendar** and complete Google's authorization flow.
2. Select only the shared calendar named `Familie`. Do not select the personal or holiday calendars.
3. Select **Rolling Month**, **24 hrs**, timezone **Europe/Berlin**, include event time **Yes**, include past events **No**, and any event-status filter appropriate for the family.
4. Save the instance. Its browser URL has the form `https://trmnl.com/plugin_settings/<integer>/edit`; for this installation the integer is `409973`.
5. In **Playlists**, keep the calendar instance on the active device playlist but click its eye icon to hide it. Hidden is different from removing it: a removed source may stop refreshing.
6. Force-refresh the calendar instance once. The authenticated `/api/plugin_settings/<integer>/data` response must contain `data.events` before configuring the dashboard.

The name `Familie` is enforced operationally by selecting only that calendar in the native instance. The data-only response identifies the selected calendar internally but does not provide a safe second authorization boundary for this plugin to discover and switch calendars.

### 2. Push and configure the dashboard

Create the host-side config directory once, then authenticate `trmnlp` if this machine has not already been logged in:

```sh
mkdir -p "$HOME/.config/trmnlp"
docker run --rm -it \
  --pull always \
  --volume "$HOME/.config/trmnlp:/root/.config/trmnlp" \
  --volume "$PWD/apps/family-dashboard/plugin:/plugin" \
  trmnl/trmnlp:latest login
```

Push only this plugin directory:

```sh
docker run --rm -it \
  --pull always \
  --volume "$HOME/.config/trmnlp:/root/.config/trmnlp" \
  --volume "$PWD/apps/family-dashboard/plugin:/plugin" \
  trmnl/trmnlp:latest push
```

The first push creates the private plugin and writes its server-assigned `id` into `plugin/src/settings.yml`. Commit that ID: it is the stable identity that makes later pushes update this plugin rather than create another. It is not a secret. Never copy the Berlin Times ID into this file.

In the new plugin's settings, confirm the prefilled calendar plugin setting ID is `409973`, enter the TRMNL user API key, enable **Debug Logs** while commissioning, save, and force-refresh. Confirm that all three panels contain live data and no credentials appear in returned variables or logs.

### 3. Configure the device playlist

Keep exactly these visible full-screen items in the target device playlist:

1. The Berlin Times
2. Berlin Family Dashboard

Keep the Google Calendar source hidden. Remove or hide the separate visible weather and public-transport plugins. Select landscape and 16 grayscale levels for the X. With two visible items, the device alternates between the newspaper and dashboard; how often the dashboard appears is the device playlist interval, while how fresh its source snapshot is depends on the plugin's 15-minute refresh interval and the upstream caches.

## Local development

Requirements are Docker, Node.js for the fast transform tests, and Python 3 for the fixture server. From the repository root:

```sh
node --test apps/family-dashboard/plugin/test/transform_test.js
docker run --rm --pull always \
  --volume "$PWD/apps/family-dashboard/plugin:/plugin" \
  trmnl/trmnlp:latest lint
```

Start the deterministic source server in a separate terminal:

```sh
python3 apps/family-dashboard/plugin/test/fixture_server.py \
  apps/family-dashboard/plugin/test/fixtures \
  --plugin-build apps/family-dashboard/plugin/_build \
  --variant typical
```

Valid variants are `typical`, `maximum`, and `degraded`. Then build the X render:

```sh
docker run --rm --pull always \
  --add-host=host.docker.internal:host-gateway \
  --volume "$PWD/apps/family-dashboard/plugin:/plugin" \
  trmnl/trmnlp:latest build --png \
  --width 1872 --height 1404 --color-depth 4
```

The `--add-host` option is required on Linux and harmless with current Docker Desktop. To run the browser geometry assertion:

```sh
docker run --rm \
  --add-host=host.docker.internal:host-gateway \
  --env LAYOUT_URL=http://host.docker.internal:8010/__plugin__/full.html \
  --env EXPECTED_WEATHER_DAYS=3 \
  --env EXPECTED_EVENTS=5 \
  --env EXPECTED_CITY=2 \
  --env EXPECTED_HOHENSCHOENHAUSEN=2 \
  --entrypoint /usr/local/bin/ruby \
  --volume "$PWD/apps/family-dashboard/plugin:/plugin" \
  trmnl/trmnlp:latest test/layout_test.rb
```

The assertion verifies X viewport geometry, the 54/46 columns, both vertical splits, fixture cardinalities, direction-label bounds, and page overflow. CI repeats it for normal, maximum-copy/winter, and partial-outage fixtures, verifies the device PNG is exactly 1872×1404 with at most 16 colors, and uploads both browser and device renders beside reviewed golden images.

Local builds are intentionally fixture-only so tests never need account credentials. Validate live production data after pushing by enabling Serverless Debug Logs and using **Force Refresh** on the private plugin.

## Failure behavior and troubleshooting

- **Calendar unavailable:** confirm the native instance is hidden, not removed; force-refresh it; verify the integer ID and API key; re-authorize Google if the native plugin itself is degraded.
- **Fewer than five events:** confirm only `Familie` is selected, use Rolling Month, disable ignored phrases that match desired events, and remember that the native integration currently supplies a 30-day horizon.
- **Weather unavailable:** inspect Serverless Debug Logs for timeout, non-JSON, or schema errors. The fixed coordinates and requested fields live in `src/transform.js`.
- **One BVG direction empty:** confirm the source API is reachable and direction IDs still resolve. A legitimately empty next-hour result displays `Keine Abfahrten`; a failed request displays `Derzeit nicht verfügbar`.
- **Plugin degraded:** fix the source/configuration first, then use TRMNL's plugin-health reset control. Repeated polling failures can cause TRMNL to pause refreshes.
- **Local build cannot reach fixtures:** ensure port 8010 is free, the fixture server is still running, and Docker can resolve `host.docker.internal`; add the host-gateway option on Linux.
- **Push targets the wrong plugin:** stop before confirming overwrite and inspect the `id` in this app's `src/settings.yml` plus the mounted host configuration. Each app must retain its own ID.

## Acceptance checklist

Before relying on the installation:

- inspect all three CI renders and one physical 16-level X refresh;
- confirm the current temperature differs from the daily maximum when expected;
- compare all five event rows with the `Familie` calendar, including one all-day event;
- compare both departure groups with the BVG app and verify they are not interleaved;
- simulate a source outage or inspect the degraded golden render;
- observe at least one daylight-saving transition for calendar and departure times;
- verify the visible playlist alternates only Berlin Times and Family Dashboard while the calendar source remains hidden.

Upstream interfaces and TRMNL Serverless are external contracts. Re-run the fixture/unit suite after adapting any payload schema, and validate against live Debug Logs before pushing such a change.
