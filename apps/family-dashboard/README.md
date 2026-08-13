# Berlin Family Dashboard

Berlin Family Dashboard is a private, full-screen TRMNL X plugin for a household playlist. It combines weather for configurable coordinates, the next five events from a selected Google calendar, and the next two tram departures in each of two configured directions.

```text
Google Calendar API ────→ TRMNL OAuth polling ┐
Open-Meteo ───────────────────────────────────┼→ TRMNL Serverless → Liquid → TRMNL X
BVG transport.rest ───────────────────────────┘
```

No separately hosted application or new GitHub Pages deployment is required. TRMNL polls Google Calendar with its borrowed Google Calendar OAuth app, while the plugin's Node serverless function performs three concurrent, bounded requests for weather and departures. It normalizes all four sources and returns only the view model needed by Liquid. The existing Berlin Times publisher remains independent.

## Product and layout contract

The only supported target is TRMNL X at 1872×1404, landscape, with 16 grayscale levels. Half-horizontal, half-vertical, quadrant, portrait, and smaller-device layouts intentionally show a full-screen-only notice.

The screen uses a high-contrast Berlin wayfinding-board visual language:

- 54% left column: weather;
- upper third of weather: today's condition glyph, current temperature, humidity, and daily maximum;
- lower two thirds of weather: condition glyph and maximum temperature for each of the next three days;
- 46% right column, upper 58%: five chronologically next events, regardless of week boundary;
- right column, lower 42%: two non-interleaved departure boards with deployment-configured labels.

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

The dashboard uses the Polling strategy because hosted Serverless transforms run on polled responses. It borrows TRMNL's Google Calendar OAuth app and sends the access token only in the Authorization header of the Google Events API request. The subsequent Serverless requests to Open-Meteo and BVG do not receive that header.

The Family Dashboard private plugin requires deployment-specific custom fields for the Google Calendar ID and heading, weather latitude and longitude, transit origin and two direction IDs, and the three visible transit labels. No household location, calendar name, account identifier, or private plugin ID is stored in the public repository.

No TRMNL API key, Google client secret, or separately hosted service is required. Calendar event titles necessarily pass through TRMNL because TRMNL both fetches and renders the plugin; do not use this design if that trust boundary is unacceptable.

### Serverless runtime

TRMNL Serverless provides Node with network access and currently documents a 128 MB / 5 second execution budget. The implementation uses no packages, runs the three network requests concurrently, applies a 1.5 second timeout per request, caps each response at 250 KB, and avoids returning raw upstream payloads. Calendar parsing, weather, and each direction fail independently. A panel shows a local unavailable state when one source fails; the run fails only when all four sources are unavailable.

The `polling` strategy is required to invoke the hosted transform. Local `.trmnlp.yml` overrides the Google polling URL with the deterministic fixture server, so local tests do not need OAuth credentials. Do not upload `.trmnlp.yml`: `trmnlp push` uploads the files under `src`, while the dotfile remains a local development override.

### Source limits

- Weather uses [Open-Meteo's forecast API](https://open-meteo.com/en/docs) at coordinates supplied through private plugin settings. It requests current `temperature_2m` and `relative_humidity_2m`, plus four days of daily `weather_code` and `temperature_2m_max`. WMO codes collapse into the five stable display states sunny, partly cloudy, cloudy/fog, rain/thunderstorm, and snow. Open-Meteo requires attribution, which appears in the footer.
- Departures use [`v6.bvg.transport.rest`](https://v6.bvg.transport.rest/), which exposes BVG realtime data without an API key and documents a limit of 100 requests per minute. Origin and direction stop identifiers come from private plugin settings. Each request is tram-only, asks for six candidates in the next 60 minutes, drops cancelled or elapsed results, sorts by effective realtime `when` with `plannedWhen` fallback, and keeps two. The display uses absolute times because a 15-minute e-ink refresh makes countdowns misleading.
- Google Calendar uses the official [Events `list` endpoint](https://developers.google.com/workspace/calendar/api/v3/reference/events/list) with recurring events expanded, deleted events excluded, and results ordered by start time. Its dynamic `timeMin` is the current UTC time. Google applies this lower bound to event end times, so ongoing timed, all-day, and multi-day events remain eligible while elapsed events do not. The API page size is capped at 250 and the dashboard keeps the next five.

None of the unauthenticated upstream endpoints carries an availability SLA. At a 15-minute plugin refresh, the normal load is three serverless source requests per refresh, including two BVG requests—well below the published transport.rest limit for one household installation.

## TRMNL account setup

### 1. Get the shared calendar ID

1. Open Google Calendar in a browser.
2. Open **Settings → Settings for my calendars → selected calendar → Integrate calendar**.
3. Copy **Calendar ID**. It is usually an email address or a value ending in `group.calendar.google.com`.

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

The first push creates the private plugin and writes its server-assigned `id` into `plugin/src/settings.yml`. Keep that deployment identity only in the private checkout; do not commit it to a public repository. Never copy another plugin's ID into this file.

After the push completes, open the plugin's settings and finish the live-only configuration:

1. Confirm the strategy is **Polling**.
2. Enable OAuth and choose **Use a TRMNL OAuth app → Google Calendar**.
3. Connect the Google account that can read the selected calendar and approve the requested calendar access.
4. Paste the copied value into **Google Calendar ID**.
5. Enter a short calendar heading, weather coordinates, the transport origin ID, both direction IDs, and their visible labels.
6. Enable **Debug Logs** while commissioning, save, and use **Force Refresh**.
7. Confirm that weather, calendar, and both departure directions contain live data.

Configure OAuth only after the final `trmnlp push`; a later push of stale blank OAuth settings can replace the live provider selection. Pull the plugin before future settings edits so its current OAuth metadata round-trips locally. Tokens and OAuth client credentials are not stored in this repository.

### 3. Configure the device playlist

Keep exactly these visible full-screen items in the target device playlist:

1. The Berlin Times
2. Berlin Family Dashboard

The previous native Google Calendar source is no longer a dependency. After the dashboard succeeds once, remove it from the playlist or delete it if it is otherwise unused. Remove or hide the separate visible weather and public-transport plugins. Select landscape and 16 grayscale levels for the X. With two visible items, the device alternates between the newspaper and dashboard; how often the dashboard appears is the device playlist interval, while data is fetched on the dashboard's 15-minute refresh interval.

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
  --env EXPECTED_DIRECTION_A=2 \
  --env EXPECTED_DIRECTION_B=2 \
  --entrypoint /usr/local/bin/ruby \
  --volume "$PWD/apps/family-dashboard/plugin:/plugin" \
  trmnl/trmnlp:latest test/layout_test.rb
```

The assertion verifies X viewport geometry, the 54/46 columns, both vertical splits, fixture cardinalities, direction-label bounds, and page overflow. CI repeats it for normal, maximum-copy/winter, and partial-outage fixtures, verifies the device PNG is exactly 1872×1404 with at most 16 colors, and uploads both browser and device renders beside reviewed golden images.

Local builds are intentionally fixture-only so tests never need account credentials. Unit tests cover both the fixture schema and the Google Calendar Events API schema. Validate live production data after pushing by enabling Serverless Debug Logs and using **Force Refresh** on the private plugin.

## Failure behavior and troubleshooting

- **Calendar unavailable:** confirm the dashboard uses Polling, OAuth shows a connected Google Calendar account, and **Google Calendar ID** is exact. Reconnect OAuth if the polling log reports 401/403; recheck the ID if it reports 404.
- **Fewer than five events:** compare the selected calendar directly and confirm the connected Google account can see its event details. The dashboard displays at most five ongoing or future events.
- **Weather unavailable:** inspect Serverless Debug Logs for missing or invalid coordinates, timeout, non-JSON, or schema errors.
- **One BVG direction empty:** confirm the source API is reachable and direction IDs still resolve. A legitimately empty next-hour result displays `Keine Abfahrten`; a failed request displays `Derzeit nicht verfügbar`.
- **Plugin degraded:** fix the source/configuration first, then use TRMNL's plugin-health reset control.
- **Local build cannot reach fixtures:** ensure port 8010 is free, the fixture server is still running, and Docker can resolve `host.docker.internal`; add the host-gateway option on Linux.
- **Push targets the wrong plugin:** stop before confirming overwrite and inspect the `id` in this app's `src/settings.yml` plus the mounted host configuration. Each app must retain its own ID.

## Acceptance checklist

Before relying on the installation:

- inspect all three CI renders and one physical 16-level X refresh;
- confirm the current temperature differs from the daily maximum when expected;
- compare all five event rows with the selected calendar, including one all-day event;
- compare both departure groups with the BVG app and verify they are not interleaved;
- simulate a source outage or inspect the degraded golden render;
- observe at least one daylight-saving transition for calendar and departure times;
- verify the visible playlist alternates only Berlin Times and Family Dashboard, with no native calendar source required.

Upstream interfaces and TRMNL Serverless are external contracts. Re-run the fixture/unit suite after adapting any payload schema, and validate against live Debug Logs before pushing such a change.
