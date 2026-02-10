module.exports = [
  {
    ignores: ["node_modules/**", "dist/**", "build/**", "target/**"],
  },
  {
    files: ["frontend/**/*.js"],
    languageOptions: {
      ecmaVersion: "latest",
      sourceType: "script",
      globals: {
        window: "readonly",
        document: "readonly",
        navigator: "readonly",
        fetch: "readonly",
        localStorage: "readonly",
        requestAnimationFrame: "readonly",
        setTimeout: "readonly",
        clearTimeout: "readonly",
        location: "readonly",
        ResizeObserver: "readonly",
        Promise: "readonly",
        I18N: "readonly",
      },
    },
    rules: {
      ...require("@eslint/js").configs.recommended.rules,
    },
  },
];
