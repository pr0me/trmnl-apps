const assert = require("node:assert/strict");
const { readFile } = require("node:fs/promises");
const { join } = require("node:path");
const { test } = require("node:test");

const {
  calendarPayload,
  normalizeDepartures,
  normalizeEvents,
  normalizeWeather,
  run,
  weatherState,
} = require("../src/transform.js");

const fixtureDirectory = join(__dirname, "fixtures");

async function fixture(name) {
  return JSON.parse(await readFile(join(fixtureDirectory, `${name}.json`), "utf8"));
}

test("calendar payload accepts hosted polling and local fixture shapes", () => {
  const direct = { items: [{ summary: "Direct" }] };
  const indexed = { items: [{ summary: "Indexed" }] };
  const fixturePayload = { events: [{ summary: "Fixture" }] };
  assert.equal(calendarPayload(direct), direct);
  assert.equal(calendarPayload({ IDX_0: indexed }), indexed);
  assert.equal(calendarPayload({ calendar_source: fixturePayload }), fixturePayload);
  assert.equal(calendarPayload({}), null);
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

test("calendar normalization removes elapsed events and limits the ordered result", async () => {
  const input = await fixture("typical");
  const events = normalizeEvents(input.calendar, new Date(input.now));
  assert.equal(events.length, 5);
  assert.equal(events[0].title, "Sommerfest im Innenhof");
  assert.equal(events[0].time, "ganztägig");
  assert.equal(events[1].time, "10:30");
  assert.ok(events.every((event) => event.title !== "Schon vorbei"));
});

test("Google Calendar events normalize timed and all-day entries", () => {
  const payload = {
    items: [
      {
        status: "confirmed",
        summary: "Abendessen",
        start: { dateTime: "2026-08-08T18:30:00+02:00" },
        end: { dateTime: "2026-08-08T20:00:00+02:00" },
      },
      {
        status: "confirmed",
        summary: "Familientag",
        start: { date: "2026-08-09" },
        end: { date: "2026-08-10" },
      },
      {
        status: "cancelled",
        summary: "Abgesagt",
        start: { dateTime: "2026-08-08T17:00:00+02:00" },
        end: { dateTime: "2026-08-08T18:00:00+02:00" },
      },
    ],
  };
  const events = normalizeEvents(payload, new Date("2026-08-08T14:00:00Z"));
  assert.deepEqual(events.map((event) => event.title), ["Abendessen", "Familientag"]);
  assert.equal(events[0].time, "18:30");
  assert.equal(events[1].time, "ganztägig");
});

test("ongoing multi-day all-day events remain visible", async () => {
  const input = await fixture("maximum");
  const events = normalizeEvents(input.calendar, new Date(input.now));
  assert.equal(events[0].title, "Mehr­tägiger Familienurlaub zwischen den Jahren");
  assert.equal(events[0].date, "2026-12-28");
});

test("departures use realtime values, reject stale and cancelled entries, and keep two", async () => {
  const input = await fixture("typical");
  input.city.departures.unshift({
    when: "2026-08-08T15:59:00+02:00",
    plannedWhen: "2026-08-08T15:59:00+02:00",
    delay: 0,
    direction: "Schon weg",
    line: { name: "M5" },
  });
  input.city.departures.unshift({
    when: "2026-08-08T16:05:00+02:00",
    plannedWhen: "2026-08-08T16:05:00+02:00",
    cancelled: true,
    direction: "Fällt aus",
    line: { name: "18" },
  });
  input.city.departures[3].delay = null;
  const departures = normalizeDepartures(input.city, new Date(input.now));
  assert.equal(departures.length, 2);
  assert.deepEqual(departures.map((departure) => departure.time), ["16:12", "16:26"]);
  assert.equal(departures[0].delay_minutes, 2);
  assert.equal(departures[0].delay_label, "+2 min");
  assert.equal(departures[1].delay_label, "ohne Echtzeit");
});

test("run isolates source failures and never forwards raw source data", async () => {
  const input = await fixture("degraded");
  const names = ["weather", "city", "hohenschoenhausen"];
  input.calendar_source = input.calendar;
  input.fixture_now = input.now;
  input.fixture_sources = Object.fromEntries(
    names.map((name) => [name, `http://host.docker.internal:8010/${name}/degraded`]),
  );
  const fetchImplementation = async (url) => {
    const source = new URL(url).pathname.split("/")[1];
    if (input.errors.includes(source)) {
      return new Response("unavailable", { status: 503 });
    }
    return new Response(JSON.stringify(input[source]), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  };
  const result = await run(input, fetchImplementation);
  assert.equal(result.source_status.weather, "error");
  assert.equal(result.source_status.hohenschoenhausen, "error");
  assert.equal(result.source_status.calendar, "ok");
  assert.equal(result.events.length, 2);
  assert.equal(result.departures.city.length, 1);
  assert.equal("calendar" in result, false);
});
