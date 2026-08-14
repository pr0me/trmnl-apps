#!/usr/bin/env node

const { readFile, writeFile } = require("node:fs/promises");
const { resolve } = require("node:path");

const { run } = require("../../data-plugin/src/transform.js");

const helperSettings = {
  weather_latitude: "52.5200",
  weather_longitude: "13.4050",
  transit_stop_id: "900000100003",
  transit_stop_label: "Beispielhaltestelle",
  transit_direction_a_id: "900000100001",
  transit_direction_a_label: "Richtung A",
  transit_direction_b_id: "900000100002",
  transit_direction_b_label: "Richtung B",
};

function fixtureFetch(input) {
  return async (url) => {
    let source = "weather";
    if (url.includes("transport.rest")) {
      const direction = new URL(url).searchParams.get("direction");
      source = direction === helperSettings.transit_direction_a_id ? "direction_a" : "direction_b";
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

async function generateFixture(variant, outputPath) {
  const baseVariant = ["maximum", "degraded"].includes(variant) ? variant : "typical";
  const fixturePath = resolve(__dirname, "fixtures", `${baseVariant}.json`);
  const input = JSON.parse(await readFile(fixturePath, "utf8"));
  const helper = await run(helperSettings, fixtureFetch(input), new Date(input.now));

  let calendarSource = input.calendar;
  let dashboardDataSource = { merge_variables: helper };
  if (variant === "missing-helper") {
    dashboardDataSource = null;
  } else if (variant === "missing-calendar") {
    calendarSource = null;
  } else if (variant === "empty-calendar") {
    calendarSource = { events: [] };
  } else if (variant === "wrong-helper-contract") {
    dashboardDataSource = { merge_variables: { ...helper, contract: "unrelated_contract" } };
  }

  const config = [
    "---",
    "watch: false",
    "time_zone: Europe/Berlin",
    "custom_fields:",
    "  calendar_label: Familie",
    "variables:",
    `  calendar_source: ${JSON.stringify(calendarSource)}`,
    `  dashboard_data_source: ${JSON.stringify(dashboardDataSource)}`,
    "",
  ].join("\n");
  await writeFile(outputPath, config, "utf8");
  return { calendarSource, dashboardDataSource };
}

async function main() {
  const variantIndex = process.argv.indexOf("--variant");
  const outputIndex = process.argv.indexOf("--output");
  const variant = variantIndex >= 0 ? process.argv[variantIndex + 1] : "typical";
  const outputPath = outputIndex >= 0 ? process.argv[outputIndex + 1] : null;
  const variants = [
    "typical",
    "maximum",
    "degraded",
    "missing-helper",
    "missing-calendar",
    "empty-calendar",
    "wrong-helper-contract",
  ];
  if (!variants.includes(variant)) {
    throw new Error("invalid fixture variant");
  }
  if (!outputPath) {
    throw new Error("missing fixture output path");
  }
  await generateFixture(variant, outputPath);
}

if (require.main === module) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}

module.exports = { generateFixture };
