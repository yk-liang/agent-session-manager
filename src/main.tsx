import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { installParagraphSelection } from "./paragraphSelection";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/shell.css";
import "./styles/catalog.css";
import "./styles/conversation.css";
import "./styles/activity.css";
import "./styles/config.css";
import "./styles/usage.css";

installParagraphSelection();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
