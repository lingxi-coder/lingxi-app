import React from "react";
import { createRoot } from "react-dom/client";
import AppShell from "../components/AppShell";
import "./globals.css";

const root = document.getElementById("root");
if (!root) {
  throw new Error("LingXi local app root element is missing");
}

createRoot(root).render(
  <React.StrictMode>
    <AppShell />
  </React.StrictMode>,
);
