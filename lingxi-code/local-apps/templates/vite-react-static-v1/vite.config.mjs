const runtimeModules = "/opt/lingxi/local-app-runtime/node_modules";

export default {
  base: "./",
  resolve: {
    alias: [
      { find: /^react$/, replacement: `${runtimeModules}/react/index.js` },
      {
        find: /^react\/jsx-runtime$/,
        replacement: `${runtimeModules}/react/jsx-runtime.js`,
      },
      {
        find: /^react\/jsx-dev-runtime$/,
        replacement: `${runtimeModules}/react/jsx-dev-runtime.js`,
      },
      {
        find: /^react-dom\/client$/,
        replacement: `${runtimeModules}/react-dom/client.js`,
      },
      {
        find: /^react-dom$/,
        replacement: `${runtimeModules}/react-dom/index.js`,
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
