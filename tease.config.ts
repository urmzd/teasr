import { defineConfig } from "./src/index.js";

export default defineConfig({
  server: {
    command: "npx serve examples/demo --listen 3123",
    url: "http://localhost:3123",
    timeout: 10_000,
  },
  scenes: [
    // Web: capture the demo landing page
    {
      type: "web",
      url: "/",
      name: "demo-landing",
      formats: ["png"],
    },

    // Terminal: capture tease's own help output
    {
      type: "terminal",
      command: "node dist/cli.js --help",
      name: "cli-help",
      theme: "dracula",
      cols: 90,
      formats: ["png"],
    },

    // Terminal: colorful output demo
    {
      type: "terminal",
      command: "node -e \"console.log('\\x1b[1m\\x1b[35mtease\\x1b[0m — capture showcase assets'); console.log(); console.log('  \\x1b[36mweb\\x1b[0m       Playwright-based page capture'); console.log('  \\x1b[33mterminal\\x1b[0m  Styled CLI output as images'); console.log('  \\x1b[32mscreen\\x1b[0m    Desktop and native app capture');\"",
      name: "colorful-modes",
      theme: "dracula",
      cols: 60,
      formats: ["png"],
    },
  ],
  output: {
    dir: "./showcase",
    formats: ["png"],
  },
});
