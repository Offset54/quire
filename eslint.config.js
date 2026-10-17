import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
  {
    ignores: [
      "**/dist/**",
      "**/node_modules/**",
      "**/target/**",
      "**/src-tauri/gen/**",
      ".cache/**",
      "third_party/**",
    ],
  },
  js.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    extends: [...tseslint.configs.strictTypeChecked],
    languageOptions: {
      globals: globals.browser,
      parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname },
    },
    plugins: { "react-hooks": reactHooks },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // Numbers in template strings are fine.
      "@typescript-eslint/restrict-template-expressions": ["error", { allowNumber: true }],
      // With noUncheckedIndexedAccess, `!` is the idiomatic way to index
      // typed arrays in pixel and buffer code.
      "@typescript-eslint/no-non-null-assertion": "off",
    },
  },
  {
    files: ["**/*.{js,mjs}"],
    languageOptions: { globals: globals.node },
  },
);
