export default {
  base: "./",
  build: {
    // The host materializes the verified dependency tree at local
    // `node_modules/`, matching Node and Vite's normal project layout.
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: false,
    target: "safari17",
    reportCompressedSize: false,
  },
};
