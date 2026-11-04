import js from "@eslint/js";
import { defineConfig } from "eslint/config";
import jsxA11y from "eslint-plugin-jsx-a11y";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

export default defineConfig(
  { ignores: ["dist"] },
  js.configs.recommended,
  tseslint.configs.strictTypeChecked,
  reactHooks.configs.flat.recommended,
  jsxA11y.flatConfigs.strict,
  {
    languageOptions: { parserOptions: { projectService: true } },
    rules: {
      "max-lines": ["error", 300],
      "@typescript-eslint/no-non-null-assertion": "error",
      "@typescript-eslint/consistent-type-imports": "error",
      "@typescript-eslint/consistent-type-assertions": ["error", { assertionStyle: "as", objectLiteralTypeAssertions: "never" }],
      "no-console": "error",
    },
  },
  // Config files are plain JavaScript outside the TypeScript project.
  { files: ["**/*.mjs"], extends: [tseslint.configs.disableTypeChecked] },
  // The stub server and the browser test run in Node 24, which has these as globals.
  {
    files: ["stub/**/*.mjs", "browser-test/**/*.mjs"],
    languageOptions: { globals: { URL: "readonly", fetch: "readonly", setInterval: "readonly", setTimeout: "readonly", document: "readonly", window: "readonly" } },
  },
);
