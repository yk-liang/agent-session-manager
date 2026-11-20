import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { installParagraphSelection } from "./paragraphSelection";
import "./styles.css";
import "./usage.css";
import "./sessionOutline.css";

installParagraphSelection();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
