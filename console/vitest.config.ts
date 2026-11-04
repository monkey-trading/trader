import { fileURLToPath } from "node:url";

import { defineConfig } from "vitest/config";

const alias = { "@": fileURLToPath(new URL("./src", import.meta.url)) };

// `.test.ts` runs in Node; `.test.tsx` runs in a DOM with Testing Library's matchers.
export default defineConfig({
  test: {
    globals: true,
    projects: [
      {
        resolve: { alias },
        test: { name: "node", environment: "node", include: ["src/**/*.test.ts"] },
      },
      {
        resolve: { alias },
        test: {
          name: "dom",
          environment: "jsdom",
          include: ["src/**/*.test.tsx"],
          setupFiles: ["./src/test-setup.ts"],
        },
      },
    ],
  },
});
