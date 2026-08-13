const BERLIN_TIME_ZONE = "Europe/Berlin";
const FIXTURE_URL_PREFIX = "http://host.docker.internal:8010/";
const MAX_RESPONSE_BYTES = 250_000;
const REQUEST_TIMEOUT_MS = 1_500;
const FIXTURE_LABELS = {
  calendar: "Kalender",
  stop: "Beispielhaltestelle",
  direction_a: "Richtung A",
  direction_b: "Richtung B",
};

function requiredSetting(input, key) {
  const value = input?.[key];
  if (typeof value !== "string" || !value.trim()) {
    throw new Error(`missing ${key}`);
  }
  return value.trim();
}

function coordinateSetting(input, key, minimum, maximum) {
  const value = Number(requiredSetting(input, key));
  if (!Number.isFinite(value) || value < minimum || value > maximum) {
    throw new Error(`invalid ${key}`);
  }
  return value;
}

function transitSetting(input, key) {
  const value = requiredSetting(input, key);
  if (!/^\d+$/.test(value)) {
    throw new Error(`invalid ${key}`);
  }
  return value;
}

function labelSetting(input, key) {
  const value = requiredSetting(input, key);
  if (value.length > 48) {
    throw new Error(`invalid ${key}`);
  }
  return value;
}

function transitSource(stop, direction) {
  return `https://v6.bvg.transport.rest/stops/${stop}/departures?direction=${direction}&results=6&duration=60&remarks=false&language=de&suburban=false&subway=false&tram=true&bus=false&ferry=false&express=false&regional=false`;
}

function productionConfiguration(input) {
  const latitude = coordinateSetting(input, "weather_latitude", -90, 90);
  const longitude = coordinateSetting(input, "weather_longitude", -180, 180);
  const stop = transitSetting(input, "transit_stop_id");
  return {
    sources: {
      weather: `https://api.open-meteo.com/v1/forecast?latitude=${latitude}&longitude=${longitude}&current=temperature_2m,relative_humidity_2m&daily=weather_code,temperature_2m_max&timezone=Europe%2FBerlin&forecast_days=4`,
      direction_a: transitSource(stop, transitSetting(input, "transit_direction_a_id")),
      direction_b: transitSource(stop, transitSetting(input, "transit_direction_b_id")),
    },
    labels: {
      calendar: labelSetting(input, "calendar_label"),
      stop: labelSetting(input, "transit_stop_label"),
      direction_a: labelSetting(input, "transit_direction_a_label"),
      direction_b: labelSetting(input, "transit_direction_b_label"),
    },
  };
}

function finiteNumber(value, label) {
  const number = Number(value);
  if (!Number.isFinite(number)) {
    throw new Error(`invalid ${label}`);
  }
  return number;
}

function weatherState(codeValue) {
  const code = finiteNumber(codeValue, "weather code");
  if (code === 0) {
    return { key: "sunny", label: "sonnig", glyph: "sun" };
  }
  if (code === 1 || code === 2) {
    return { key: "partly_cloudy", label: "leicht bewölkt", glyph: "partly-cloudy" };
  }
  if (code === 3 || code === 45 || code === 48) {
    return { key: "cloudy", label: "bewölkt", glyph: "cloud" };
  }
  if ((code >= 71 && code <= 77) || code === 85 || code === 86) {
    return { key: "snow", label: "Schnee", glyph: "snow" };
  }
  if ((code >= 51 && code <= 67) || (code >= 80 && code <= 82) || (code >= 95 && code <= 99)) {
    return { key: "rain", label: "Regen", glyph: "rain" };
  }
  throw new Error("unsupported weather code");
}

function dateParts(date, options) {
  return new Intl.DateTimeFormat("de-DE", { timeZone: BERLIN_TIME_ZONE, ...options }).format(date);
}

function dateFromDay(day) {
  const date = new Date(`${day}T12:00:00Z`);
  if (Number.isNaN(date.getTime())) {
    throw new Error("invalid calendar date");
  }
  return date;
}

function weekday(day) {
  return new Intl.DateTimeFormat("de-DE", { weekday: "short", timeZone: "UTC" })
    .format(dateFromDay(day))
    .replace(".", "");
}

function dateLabel(day) {
  return new Intl.DateTimeFormat("de-DE", {
    day: "numeric",
    month: "short",
    timeZone: "UTC",
  })
    .format(dateFromDay(day))
    .replace(".", "");
}

function berlinDay(date) {
  const parts = new Intl.DateTimeFormat("en-CA", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    timeZone: BERLIN_TIME_ZONE,
  }).formatToParts(date);
  const values = Object.fromEntries(parts.map(({ type, value }) => [type, value]));
  return `${values.year}-${values.month}-${values.day}`;
}

function timeLabel(date) {
  return dateParts(date, { hour: "2-digit", minute: "2-digit", hour12: false });
}

function normalizeWeather(payload) {
  const current = payload?.current;
  const daily = payload?.daily;
  const times = daily?.time;
  const codes = daily?.weather_code;
  const maximums = daily?.temperature_2m_max;
  if (!current || !Array.isArray(times) || !Array.isArray(codes) || !Array.isArray(maximums)) {
    throw new Error("invalid weather response");
  }
  if (times.length < 4 || codes.length < 4 || maximums.length < 4) {
    throw new Error("weather response has fewer than four days");
  }

  const days = times.slice(0, 4).map((day, index) => {
    const state = weatherState(codes[index]);
    return {
      date: day,
      weekday: weekday(day),
      date_label: dateLabel(day),
      state: state.key,
      state_label: state.label,
      glyph: state.glyph,
      max_c: Math.round(finiteNumber(maximums[index], "maximum temperature")),
    };
  });

  return {
    current_c: Math.round(finiteNumber(current.temperature_2m, "current temperature")),
    humidity_pct: Math.round(finiteNumber(current.relative_humidity_2m, "humidity")),
    days,
  };
}

function eventStart(event) {
  return event?.start_full || event?.date_time || event?.start;
}

function eventEndDay(event) {
  const value = event?.end_full || event?.end;
  return typeof value === "string" ? value.slice(0, 10) : "";
}

function calendarEvents(payload) {
  const normalizedEvents = payload?.data?.events || payload?.events;
  if (Array.isArray(normalizedEvents)) {
    return normalizedEvents;
  }
  if (!Array.isArray(payload?.items)) {
    throw new Error("invalid calendar response");
  }

  return payload.items
    .filter((event) => event?.status !== "cancelled")
    .map((event) => {
      const start = event?.start?.dateTime || event?.start?.date;
      const end = event?.end?.dateTime || event?.end?.date;
      return {
        summary: event?.summary,
        start_full: start,
        end_full: end,
        all_day: typeof event?.start?.date === "string",
      };
    });
}

function normalizeEvents(payload, now) {
  const events = calendarEvents(payload);
  const today = berlinDay(now);

  return events
    .filter((event) => {
      const start = eventStart(event);
      if (typeof start !== "string" || start.length < 10) {
        return false;
      }
      if (event.all_day === true) {
        const startDay = start.slice(0, 10);
        const endDay = eventEndDay(event);
        return startDay >= today || (startDay < today && endDay > today);
      }
      const parsed = new Date(start);
      return !Number.isNaN(parsed.getTime()) && parsed >= now;
    })
    .sort((left, right) => {
      const leftStart = eventStart(left);
      const rightStart = eventStart(right);
      return leftStart.localeCompare(rightStart);
    })
    .slice(0, 5)
    .map((event) => {
      const start = eventStart(event);
      const day = start.slice(0, 10);
      const parsed = new Date(start);
      const allDay = event.all_day === true;
      return {
        date: day,
        weekday: weekday(day),
        date_label: dateLabel(day),
        title: String(event.summary || "Ohne Titel").trim() || "Ohne Titel",
        time: allDay ? "ganztägig" : timeLabel(parsed),
        all_day: allDay,
      };
    });
}

function normalizeDepartures(payload, now) {
  if (!Array.isArray(payload?.departures)) {
    throw new Error("invalid departure response");
  }

  return payload.departures
    .filter((departure) => departure?.cancelled !== true)
    .map((departure) => {
      const effective = departure?.when || departure?.plannedWhen;
      const parsed = new Date(effective);
      if (!effective || Number.isNaN(parsed.getTime()) || parsed < now) {
        return null;
      }
      const delaySeconds = departure.delay === null || departure.delay === undefined
        ? Number.NaN
        : Number(departure.delay);
      const delayMinutes = Number.isFinite(delaySeconds) ? Math.round(delaySeconds / 60) : null;
      return {
        line: String(departure?.line?.name || "–"),
        destination: String(departure?.direction || "Ziel unbekannt"),
        time: timeLabel(parsed),
        delay_minutes: delayMinutes,
        delay_label: delayMinutes === null
          ? "ohne Echtzeit"
          : delayMinutes > 0
            ? `+${delayMinutes} min`
            : delayMinutes < 0
              ? `${delayMinutes} min`
              : "planmäßig",
        timestamp: parsed.getTime(),
      };
    })
    .filter(Boolean)
    .sort((left, right) => left.timestamp - right.timestamp)
    .slice(0, 2)
    .map(({ timestamp: _timestamp, ...departure }) => departure);
}

function fixtureSources(input) {
  const sources = input?.fixture_sources;
  if (!sources || typeof sources !== "object") {
    return null;
  }
  const values = [sources.weather, sources.direction_a, sources.direction_b];
  const allowed = values.every(
    (value) => typeof value === "string" && value.startsWith(FIXTURE_URL_PREFIX),
  );
  return allowed ? sources : null;
}

function calendarPayload(input) {
  const candidates = [input?.calendar_source, input?.IDX_0, input?.data, input];
  return candidates.find((candidate) => (
    Array.isArray(candidate?.items)
      || Array.isArray(candidate?.events)
      || Array.isArray(candidate?.data?.events)
  )) || null;
}

async function fetchJson(url, fetchImplementation) {
  if (!url) {
    throw new Error("source is not configured");
  }
  const response = await fetchImplementation(url, {
    signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
  });
  if (!response.ok) {
    throw new Error("source request failed");
  }
  const body = await response.text();
  if (body.length > MAX_RESPONSE_BYTES) {
    throw new Error("source response is too large");
  }
  try {
    return JSON.parse(body);
  } catch (_error) {
    throw new Error("source returned invalid json");
  }
}

async function run(input, fetchImplementation = fetch) {
  const fixtureUrl = input?.fixture_url;
  if (fixtureUrl && (typeof fixtureUrl !== "string" || !fixtureUrl.startsWith(FIXTURE_URL_PREFIX))) {
    throw new Error("invalid fixture url");
  }
  const runtimeInput = fixtureUrl ? await fetchJson(fixtureUrl, fetchImplementation) : input;
  const fixture = fixtureSources(runtimeInput);
  const configuration = fixture
    ? { sources: fixture, labels: FIXTURE_LABELS }
    : productionConfiguration(runtimeInput);
  const { labels, sources } = configuration;
  const now = fixture && runtimeInput.fixture_now ? new Date(runtimeInput.fixture_now) : new Date();
  if (Number.isNaN(now.getTime())) {
    throw new Error("invalid fixture time");
  }
  const requests = [
    ["weather", sources.weather],
    ["direction_a", sources.direction_a],
    ["direction_b", sources.direction_b],
  ];
  const settled = await Promise.allSettled(
    requests.map(([, url]) => fetchJson(url, fetchImplementation)),
  );
  const payloads = {
    calendar: calendarPayload(runtimeInput),
    ...Object.fromEntries(
      requests.map(([name], index) => [name, settled[index].status === "fulfilled" ? settled[index].value : null]),
    ),
  };

  let weather = null;
  let events = [];
  const departures = { direction_a: [], direction_b: [] };
  const sourceStatus = {
    calendar: "error",
    weather: "error",
    direction_a: "error",
    direction_b: "error",
  };

  try {
    weather = normalizeWeather(payloads.weather);
    sourceStatus.weather = "ok";
  } catch (_error) {
    weather = null;
  }
  try {
    events = normalizeEvents(payloads.calendar, now);
    sourceStatus.calendar = events.length === 0 ? "empty" : "ok";
  } catch (_error) {
    events = [];
  }
  for (const direction of ["direction_a", "direction_b"]) {
    try {
      departures[direction] = normalizeDepartures(payloads[direction], now);
      sourceStatus[direction] = departures[direction].length === 0 ? "empty" : "ok";
    } catch (_error) {
      departures[direction] = [];
    }
  }

  if (Object.values(sourceStatus).every((status) => status === "error")) {
    throw new Error("all data sources unavailable");
  }

  return {
    generated_at: now.toISOString(),
    generated_time: timeLabel(now),
    labels,
    weather,
    events,
    departures,
    source_status: sourceStatus,
  };
}

if (typeof module !== "undefined") {
  module.exports = {
    calendarPayload,
    normalizeDepartures,
    normalizeEvents,
    normalizeWeather,
    productionConfiguration,
    run,
    weatherState,
  };
}
