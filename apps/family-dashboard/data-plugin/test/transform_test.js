const assert = require("node:assert/strict");
const { readFile } = require("node:fs/promises");
const { join } = require("node:path");
const { test } = require("node:test");

const {
  CONTRACT,
  normalizeDepartures,
  normalizeWeather,
  productionConfiguration,
  run,
  weatherState,
} = require("../src/transform.js");

const fixtureDirectory = join(__dirname, "../../plugin/test/fixtures");
const settings = {
  weather_latitude: "52.5200",
  weather_longitude: "13.4050",
  transit_stop_id: "900000100003",
  transit_stop_label: "Beispielhaltestelle",
  transit_direction_a_id: "900000100001",
  transit_direction_a_label: "Richtung A",
  transit_direction_b_id: "900000100002",
  transit_direction_b_label: "Richtung B",
};

async function fixture(name) {
  return JSON.parse(await readFile(join(fixtureDirectory, `${name}.json`), "utf8"));
}

function fixtureFetch(input) {
  return async (url) => {
    let source = "weather";
    if (url.includes("transport.rest")) {
      const direction = new URL(url).searchParams.get("direction");
      source = direction === settings.transit_direction_a_id ? "direction_a" : "direction_b";
    }
    if ((input.errors || []).includes(source)) {
      return new Response("unavailable", { status: 503 });
    }
    return new Response(JSON.stringify(input[source]), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  };
}

test("configuration validates coordinates, identifiers, and labels", () => {
  const configuration = productionConfiguration(settings);
  assert.match(configuration.sources.weather, /latitude=52\.52&longitude=13\.405/);
  assert.match(configuration.sources.direction_a, /stops\/900000100003\/departures/);
  assert.match(configuration.sources.direction_a, /direction=900000100001/);
  assert.deepEqual(configuration.labels, {
    stop: "Beispielhaltestelle",
    direction_a: "Richtung A",
    direction_b: "Richtung B",
  });
  assert.throws(() => productionConfiguration({}), /missing weather_latitude/);
  assert.throws(
    () => productionConfiguration({ ...settings, weather_latitude: "91" }),
    /invalid weather_latitude/,
  );
  assert.throws(
    () => productionConfiguration({ ...settings, transit_stop_id: "not-a-stop" }),
    /invalid transit_stop_id/,
  );
  assert.throws(
    () => productionConfiguration({ ...settings, transit_stop_label: "x".repeat(49) }),
    /invalid transit_stop_label/,
  );
});

test("weather codes map to stable glyph states", () => {
  assert.equal(weatherState(0).glyph, "sun");
  assert.equal(weatherState(2).glyph, "partly-cloudy");
  assert.equal(weatherState(45).glyph, "cloud");
  assert.equal(weatherState(63).glyph, "rain");
  assert.equal(weatherState(86).glyph, "snow");
  assert.throws(() => weatherState(255), /unsupported weather code/);
});

test("weather normalization keeps current readings and four daily forecasts", async () => {
  const input = await fixture("typical");
  const weather = normalizeWeather(input.weather);
  assert.equal(weather.current_c, 23);
  assert.equal(weather.humidity_pct, 39);
  assert.deepEqual(weather.days.map((day) => day.glyph), ["partly-cloudy", "sun", "rain", "cloud"]);
  assert.deepEqual(weather.days.map((day) => day.max_c), [23, 32, 27, 21]);
});

test("departures reject cancelled and stale entries, sort, and keep two", async () => {
  const input = await fixture("typical");
  input.direction_a.departures.unshift({
    when: "2026-08-08T15:59:00+02:00",
    plannedWhen: "2026-08-08T15:59:00+02:00",
    delay: 0,
    direction: "Schon weg",
    line: { name: "M5" },
  });
  input.direction_a.departures.unshift({
    when: "2026-08-08T16:05:00+02:00",
    plannedWhen: "2026-08-08T16:05:00+02:00",
    cancelled: true,
    direction: "Fällt aus",
    line: { name: "18" },
  });
  input.direction_a.departures.reverse();
  input.direction_a.departures[1].delay = null;
  const departures = normalizeDepartures(input.direction_a, new Date(input.now));
  assert.equal(departures.length, 2);
  assert.deepEqual(departures.map((departure) => departure.time), ["16:12", "16:26"]);
  assert.equal(departures[0].delay_minutes, 2);
  assert.equal(departures[0].delay_label, "+2 min");
  assert.equal(departures[1].delay_label, "ohne Echtzeit");
});

test("run preserves partial upstream failures", async () => {
  const input = await fixture("degraded");
  const result = await run(
    settings,
    fixtureFetch(input),
    new Date(input.now),
  );
  assert.equal(result.source_status.weather, "error");
  assert.equal(result.source_status.direction_a, "ok");
  assert.equal(result.source_status.direction_b, "error");
  assert.deepEqual(result.weather, {});
  assert.equal(result.departures.direction_a.length, 1);
  assert.deepEqual(result.departures.direction_b, []);
});

test("run marks successful empty sources without failing the contract", async () => {
  const input = await fixture("typical");
  input.direction_b.departures = [];
  const result = await run(settings, fixtureFetch(input), new Date(input.now));
  assert.equal(result.source_status.direction_b, "empty");
  assert.deepEqual(result.departures.direction_b, []);
});

test("run reads custom fields from the hosted Serverless input namespace", async () => {
  const input = await fixture("typical");
  const hostedInput = {
    trmnl: {
      plugin_settings: {
        custom_fields_values: settings,
      },
    },
  };
  const result = await run(hostedInput, fixtureFetch(input), new Date(input.now));
  assert.equal(result.contract, CONTRACT);
  assert.equal(result.labels.stop, settings.transit_stop_label);
  assert.equal(result.source_status.weather, "ok");
  assert.equal(result.source_status.direction_a, "ok");
  assert.equal(result.source_status.direction_b, "ok");
});

test("run throws only when all upstream sources fail", async () => {
  const input = await fixture("typical");
  input.errors = ["weather", "direction_a", "direction_b"];
  await assert.rejects(
    run(settings, fixtureFetch(input), new Date(input.now)),
    /all data sources unavailable/,
  );
});

test("run returns the exact family dashboard data contract without calendar or oauth data", async () => {
  const input = await fixture("typical");
  const result = await run(
    { ...settings, calendar: input.calendar, oauth_access_token: "secret" },
    fixtureFetch(input),
    new Date(input.now),
  );
  assert.equal(result.contract, CONTRACT);
  assert.deepEqual(Object.keys(result), [
    "contract",
    "generated_at",
    "generated_time",
    "labels",
    "weather",
    "departures",
    "source_status",
  ]);
  assert.deepEqual(Object.keys(result.labels), ["stop", "direction_a", "direction_b"]);
  assert.deepEqual(Object.keys(result.departures), ["direction_a", "direction_b"]);
  assert.deepEqual(Object.keys(result.source_status), ["weather", "direction_a", "direction_b"]);
  assert.equal(result.generated_at, new Date(input.now).toISOString());
  assert.equal(result.generated_time, "16:00");
  assert.equal(JSON.stringify(result).includes("Sommerfest im Innenhof"), false);
  assert.equal(JSON.stringify(result).includes("secret"), false);
  assert.equal("calendar" in result, false);
  assert.equal("oauth_access_token" in result, false);
});
