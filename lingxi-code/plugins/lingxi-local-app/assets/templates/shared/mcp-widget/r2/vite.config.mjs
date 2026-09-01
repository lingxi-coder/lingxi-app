import react from "@vitejs/plugin-react";
import { viteSingleFile } from "vite-plugin-singlefile";

export default {
  base: "./",
  plugins: [react(), viteSingleFile()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    reportCompressedSize: false,
    sourcemap: false,
    target: "es2022",
  },
};
