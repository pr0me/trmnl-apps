const BERLIN_TIME_ZONE = "Europe/Berlin";
const CONTRACT = "family_dashboard_data_v1";
const MAX_RESPONSE_BYTES = 250_000;
const REQUEST_TIMEOUT_MS = 1_500;

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
    throw new Error("invalid forecast date");
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

async function fetchJson(url, fetchImplementation) {
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

async function run(input, fetchImplementation = fetch, now = new Date()) {
  if (!(now instanceof Date) || Number.isNaN(now.getTime())) {
    throw new Error("invalid current time");
  }
  const hostedSettings = input?.trmnl?.plugin_settings?.custom_fields_values;
  const { labels, sources } = productionConfiguration(hostedSettings || input);
  const requests = [
    ["weather", sources.weather],
    ["direction_a", sources.direction_a],
    ["direction_b", sources.direction_b],
  ];
  const settled = await Promise.allSettled(
    requests.map(([, url]) => fetchJson(url, fetchImplementation)),
  );
  const payloads = Object.fromEntries(
    requests.map(([name], index) => [name, settled[index].status === "fulfilled" ? settled[index].value : null]),
  );

  let weather = {};
  const departures = { direction_a: [], direction_b: [] };
  const sourceStatus = { weather: "error", direction_a: "error", direction_b: "error" };

  try {
    weather = normalizeWeather(payloads.weather);
    sourceStatus.weather = "ok";
  } catch (_error) {
    weather = {};
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
    contract: CONTRACT,
    generated_at: now.toISOString(),
    generated_time: timeLabel(now),
    labels,
    weather,
    departures,
    source_status: sourceStatus,
  };
}

if (typeof module !== "undefined") {
  module.exports = {
    CONTRACT,
    normalizeDepartures,
    normalizeWeather,
    productionConfiguration,
    run,
    weatherState,
  };
}
