import assert from "node:assert/strict";
import test from "node:test";

import {
  GIBIBYTE,
  formatGibibytes,
  requireFreeSpace,
  verificationEnvironment,
} from "./verify-support.mjs";

test("verification environment bounds Cargo cache growth by default", () => {
  assert.deepEqual(verificationEnvironment({ EXAMPLE: "value" }), {
    EXAMPLE: "value",
    CARGO_INCREMENTAL: "0",
    CARGO_BUILD_JOBS: "1",
  });
});

test("verification environment preserves explicit Cargo overrides", () => {
  const environment = verificationEnvironment({
    CARGO_INCREMENTAL: "1",
    CARGO_BUILD_JOBS: "4",
  });
  assert.equal(environment.CARGO_INCREMENTAL, "1");
  assert.equal(environment.CARGO_BUILD_JOBS, "4");
});

test("free-space preflight reports an actionable cleanup command", () => {
  assert.doesNotThrow(() => requireFreeSpace(8n * GIBIBYTE, 8n * GIBIBYTE, "Local verification"));
  assert.throws(
    () => requireFreeSpace(7n * GIBIBYTE, 8n * GIBIBYTE, "Local verification"),
    /7\.0 GiB is available.*npm run clean:rust/u,
  );
  assert.equal(formatGibibytes(3n * GIBIBYTE), "3.0 GiB");
});
