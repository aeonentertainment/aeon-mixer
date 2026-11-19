import { defineConfig } from "vite-plus";

export default defineConfig({
  pack: {
    entry: ["src/index.ts", "src/contract/index.ts"],
    dts: { generator: "tsgo" },
    exports: true,
  },
  lint: {
    options: {
      typeAware: true,
      typeCheck: true,
    },
    plugins: ["typescript", "oxc", "unicorn"],
    categories: {
      correctness: "error",
      suspicious: "warn",
      perf: "warn",
    },
    rules: {
      "typescript/no-explicit-any": "error",
    },
  },
  fmt: {
    printWidth: 100,
  },
  test: {
    environment: "node",
  },
  staged: {
    "*": "vp check --fix --no-error-on-unmatched-pattern",
  },
});
