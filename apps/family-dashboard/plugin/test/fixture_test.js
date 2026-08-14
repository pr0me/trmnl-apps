const assert = require("node:assert/strict");
const { mkdtemp, readFile, rm } = require("node:fs/promises");
const { join } = require("node:path");
const { tmpdir } = require("node:os");
const { test } = require("node:test");

const { generateFixture } = require("./generate_fixture.js");

test("fixture generator combines native calendar data and actual helper output", async () => {
  const directory = await mkdtemp(join(tmpdir(), "family-dashboard-fixture-"));
  try {
    const output = join(directory, ".trmnlp.yml");
    const result = await generateFixture("typical", output);
    const config = await readFile(output, "utf8");
    assert.equal(result.dashboardDataSource.merge_variables.contract, "family_dashboard_data_v1");
    assert.equal(result.calendarSource.events[0].summary, "Sommerfest im Innenhof");
    assert.match(config, /calendar_label: Familie/);
    assert.match(config, /family_dashboard_data_v1/);
    assert.match(config, /Gemeinsames Frühstück/);
    assert.equal(config.includes("oauth"), false);
  } finally {
    await rm(directory, { recursive: true });
  }
});

test("fixture generator emits each missing and invalid source state", async () => {
  const directory = await mkdtemp(join(tmpdir(), "family-dashboard-states-"));
  try {
    const variants = {
      "missing-helper": (result) => assert.equal(result.dashboardDataSource, null),
      "missing-calendar": (result) => assert.equal(result.calendarSource, null),
      "empty-calendar": (result) => assert.deepEqual(result.calendarSource.events, []),
      "wrong-helper-contract": (result) => assert.equal(result.dashboardDataSource.merge_variables.contract, "unrelated_contract"),
    };
    for (const [variant, assertion] of Object.entries(variants)) {
      assertion(await generateFixture(variant, join(directory, `${variant}.yml`)));
    }
  } finally {
    await rm(directory, { recursive: true });
  }
});
