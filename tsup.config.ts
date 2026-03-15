import { defineConfig } from "tsup";

export default defineConfig([
  {
    entry: ["src/index.ts"],
    format: ["esm"],
    dts: true,
    splitting: true,
    clean: true,
    target: "node18",
  },
  {
    entry: ["src/cli.ts"],
    format: ["esm"],
    dts: false,
    splitting: true,
    clean: false,
    target: "node18",
    banner: {
      js: "#!/usr/bin/env node",
    },
  },
]);
