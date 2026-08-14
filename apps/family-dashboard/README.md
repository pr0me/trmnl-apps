# Berlin Family Dashboard

Berlin Family Dashboard is a private, full-screen TRMNL X view assembled from three cooperating plugin instances:

```text
Hidden native Google Calendar ─┐
                               ├─→ Visible Berlin Family Dashboard (Plugin Merge)
Hidden Dashboard Data helper ──┘
       │
       ├─ Open-Meteo
       └─ transport.rest / BVG
```

The native Google Calendar instance owns Google OAuth, calendar choice, event filtering, ordering, timezone, time format, and event horizon. `data-plugin` is a private Polling plus Serverless helper that concurrently fetches and normalizes weather and two transit directions. `plugin` is the presentation-only Plugin Merge view. It has no polling endpoint, OAuth configuration, external requests, or Serverless transform.

Plugin Merge is required because TRMNL does not expose one plugin instance's cached data to an ordinary Polling or Static plugin. Its `plugin_instance_select` fields bind active source instances to stable Liquid aliases. Only instances in a playlist or mashup appear in those selectors, and only active instances continue refreshing. Both sources must therefore stay in the target playlist and be hidden with the eye control, not removed.

## Deployable roots and ownership

The two `trmnlp` roots are deliberately isolated:

- `apps/family-dashboard/data-plugin` deploys **Berlin Family Dashboard Data**, ID `415366`. It owns weather coordinates, the transit origin ID and label, and both direction IDs and labels.
- `apps/family-dashboard/plugin` deploys **Berlin Family Dashboard**, ID `410021`. It owns the selected native calendar instance, selected helper instance, and calendar heading.
- The native Google Calendar plugin owns all calendar and Google account configuration. No Google client credentials, calendar ID, TRMNL API key, public calendar URL, or household deployment values belong in this repository.

The unrelated Berlin Times app, publisher, and workflows are independent.

## Helper contract

The view accepts helper data only when `contract` is exactly `family_dashboard_data_v1`:

```json
{
  "contract": "family_dashboard_data_v1",
  "generated_at": "2026-08-08T14:00:00.000Z",
  "generated_time": "16:00",
  "labels": {
    "stop": "Example stop",
    "direction_a": "Direction A",
    "direction_b": "Direction B"
  },
  "weather": {},
  "departures": {
    "direction_a": [],
    "direction_b": []
  },
  "source_status": {
    "weather": "ok|empty|error",
    "direction_a": "ok|empty|error",
    "direction_b": "ok|empty|error"
  }
}
```

The helper uses TRMNL's documented example JSON endpoint only as a reliable Polling trigger. Its Node transform makes the three real requests concurrently, applies a 1.5-second timeout and 250 KB response limit to each, and never returns raw upstream payloads. Weather and each direction fail independently. Empty departure arrays are successful `empty` sources; request or normalization failures are `error`. The transform throws only when all three sources are `error`.

Weather comes from Open-Meteo and is collapsed into five stable SVG glyph states. Each transport.rest request asks for six tram departures in one direction, rejects cancelled and elapsed records, orders by effective realtime departure, and keeps two. Absolute times are used because countdowns become misleading between e-ink refreshes.

The helper's layouts are diagnostic only. If it is accidentally visible, it identifies itself as a data-only source and shows the three source states.

## View behavior

The full-screen target remains TRMNL X landscape at 1872×1404 and 16 grayscale levels. Smaller layouts show the existing full-screen-only notice.

The view reads `calendar_source.events` without re-filtering or reordering it and renders at most five events. The native event's `summary` is the title, `date_time` supplies German weekday and numeric date labels, `start` supplies a local timed-event value, and all-day events display `ganztägig`. Private-plugin Serverless output is accepted directly or from the `dashboard_data_source.merge_variables` and nested `data` wrappers used by hosted merge payloads.

Source states are distinct:

- absent calendar selector: `Kalenderquelle fehlt`;
- selected calendar with no events: `Keine anstehenden Termine`;
- absent helper selector: `Dashboard-Datenquelle fehlt`;
- selected private plugin with the wrong contract: `Falsche Dashboard-Datenquelle`;
- weather request failure: weather unavailable while calendar and transit remain visible;
- one direction request failure: that direction is unavailable while the other remains visible;
- successful direction with no departures: `Keine Abfahrten`.

The footer timestamp belongs to the helper refresh. Calendar and helper caches refresh independently; refreshing the merge view does not force either source to fetch fresh upstream data.

## Account setup

1. Configure the native Google Calendar instance with the intended calendar, `Europe/Berlin`, 24-hour time, and an event horizon that can provide at least five upcoming rows. Save and force-refresh it.
2. Add that exact calendar instance to the target playlist. Hide it with the eye control, but do not remove it.
3. Open Berlin Family Dashboard Data. Enter weather latitude/longitude, transit stop ID and label, and both direction IDs and labels. Enable Serverless Debug Logs while commissioning, save, add it to the same playlist, hide it, and force-refresh it.
4. Confirm both hidden sources are active. They cannot appear in the visible dashboard's selectors until they belong to a playlist or mashup.
5. Open Berlin Family Dashboard and confirm its strategy is Plugin Merge. Select the exact Google Calendar instance for **Calendar data source**, the exact helper for **Dashboard data source**, enter the calendar heading, save, and force-refresh.

Use this refresh order whenever testing changed configuration: native calendar first, helper second, visible dashboard last.

The target playlist should show only the visible dashboard (and any unrelated desired screens). The calendar and helper entries remain active with hidden eye icons. Removing either silently stops its synchronization and eventually leaves the merge view absent or stale.

## Local development

Requirements are Node.js, Python 3, and Docker. Run the unit and lint suite from the repository root:

```sh
node --test \
  apps/family-dashboard/data-plugin/test/transform_test.js \
  apps/family-dashboard/plugin/test/fixture_test.js

docker run --rm --pull always \
  --volume "$PWD/apps/family-dashboard/data-plugin:/plugin" \
  trmnl/trmnlp:latest lint

docker run --rm \
  --volume "$PWD/apps/family-dashboard/plugin:/plugin" \
  trmnl/trmnlp:latest lint
```

Local view builds use a generated temporary `.trmnlp.yml`. The generator combines raw native-calendar fixture data, the actual helper transform output, and view settings without changing the tracked project config:

```sh
fixture_root="$(mktemp -d)"
cp -R apps/family-dashboard/plugin "$fixture_root/plugin"
node apps/family-dashboard/plugin/test/generate_fixture.js \
  --variant typical \
  --output "$fixture_root/plugin/.trmnlp.yml"
docker run --rm --pull always \
  --volume "$fixture_root/plugin:/plugin" \
  trmnl/trmnlp:latest build --png \
  --width 1872 --height 1404 --color-depth 4
```

Fixture variants are `typical`, `maximum`, `degraded`, `missing-helper`, `missing-calendar`, `empty-calendar`, and `wrong-helper-contract`. CI renders the first three as 1872×1404 device PNGs with at most 16 colors. It serves built HTML with `python3 -m http.server`, checks geometry and cardinality in Firefox, verifies representative calendar/transit text, and separately exercises all four missing/invalid merge states.

## Deployment

Authenticate once if needed:

```sh
mkdir -p "$HOME/.config/trmnlp"
docker run --rm -it --pull always \
  --volume "$HOME/.config/trmnlp:/root/.config/trmnlp" \
  --volume "$PWD/apps/family-dashboard/data-plugin:/plugin" \
  trmnl/trmnlp:latest login
```

Always push the helper before the view so a helper instance can be activated before configuring the merge selector:

```sh
docker run --rm -it --pull always \
  --volume "$HOME/.config/trmnlp:/root/.config/trmnlp" \
  --volume "$PWD/apps/family-dashboard/data-plugin:/plugin" \
  trmnl/trmnlp:latest push

docker run --rm -it --pull always \
  --volume "$HOME/.config/trmnlp:/root/.config/trmnlp" \
  --volume "$PWD/apps/family-dashboard/plugin:/plugin" \
  trmnl/trmnlp:latest push
```

The helper's first push writes its stable ID into `data-plugin/src/settings.yml`; retain that ID. The view must retain ID `410021`. When converting an older view deployment, approve deletion of its obsolete hosted transform. After pushing, pull or clone both live plugins into temporary directories and inspect their archives: helper strategy `polling` with `transform.js`; view strategy `plugin_merge` with both selectors and no transform, polling, OAuth, Google Calendar ID, weather, or transit connection fields.

## Troubleshooting

- Selector absent: put the source instance in a playlist or mashup, save it, force-refresh it, then reopen the dashboard settings. Hidden is valid; removed is not.
- Helper source `error`: check required field values and Serverless Debug Logs for timeout, non-JSON, oversized response, or upstream schema errors. Correct configuration, save, force-refresh helper, then refresh the view.
- Helper marked degraded: correct the failing source first, use TRMNL's plugin-health reset if offered, and follow the three-step refresh order.
- Wrong helper: select the Berlin Family Dashboard Data instance whose payload contract is `family_dashboard_data_v1`, not another private plugin.
- Stale timestamp: the footer shows the helper cache time. Refreshing only the view cannot update it.
- Missing or stale events: inspect and force-refresh the native calendar first, including timezone, time format, horizon, and chosen calendars.
- Physical output differs from preview: verify landscape TRMNL X, 16 grayscale levels, full layout, and one complete device refresh after the three sources have refreshed in order.
