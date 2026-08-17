import path from "node:path";
import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";

const projectRoot = path.dirname(fileURLToPath(import.meta.url));

export default {
  base: "./",
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": projectRoot,
    },
  },
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
