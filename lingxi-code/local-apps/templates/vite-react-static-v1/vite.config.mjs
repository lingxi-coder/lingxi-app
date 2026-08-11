import fs from "node:fs";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

const runtimeModules = "/opt/lingxi/local-app-runtime/node_modules";
const appModules = process.env.LINGXI_APP_NODE_MODULES || `${process.cwd()}/node_modules`;
const hasTailwind = fs.existsSync(`${appModules}/@tailwindcss/vite`);
const require = createRequire(import.meta.url);

const plugins = [];
if (hasTailwind) {
  // The app dependency tree is projected outside the build source tree and
  // takes precedence over the shared runtime. Resolve the optional plugin
  // from that exact tree rather than relying on ESM NODE_PATH behavior.
  const tailwindPath = require.resolve("@tailwindcss/vite", { paths: [appModules] });
  const tailwind = await import(pathToFileURL(tailwindPath).href);
  plugins.push((tailwind.default ?? tailwind)());
}

const modulePath = (packageName, fallback) =>
  fs.existsSync(`${appModules}/${packageName}`) ? `${appModules}/${packageName}` : fallback;

export default {
  base: "./",
  plugins,
  resolve: {
    alias: [
      { find: /^react$/, replacement: modulePath("react/index.js", `${runtimeModules}/react/index.js`) },
      {
        find: /^react\/jsx-runtime$/,
        replacement: modulePath("react/jsx-runtime.js", `${runtimeModules}/react/jsx-runtime.js`),
      },
      {
        find: /^react\/jsx-dev-runtime$/,
        replacement: modulePath("react/jsx-dev-runtime.js", `${runtimeModules}/react/jsx-dev-runtime.js`),
      },
      {
        find: /^react-dom\/client$/,
        replacement: modulePath("react-dom/client.js", `${runtimeModules}/react-dom/client.js`),
      },
      {
        find: /^react-dom$/,
        replacement: modulePath("react-dom/index.js", `${runtimeModules}/react-dom/index.js`),
      },
    ],
  },
  build: {
    outDir: "out",
    emptyOutDir: true,
    sourcemap: false,
    target: "safari17",
    reportCompressedSize: false,
  },
};
