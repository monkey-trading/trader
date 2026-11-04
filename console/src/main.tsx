/** Mounts the console into the page. */
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "@/App";

import "@/styles.css";

const root = document.getElementById("root");
if (root === null) {
  // index.html always carries the mount point; its absence is a build defect.
  throw new Error("index.html has no #root element");
}

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
