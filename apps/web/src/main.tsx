import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './app/App';
import './theme.css';
import './styles.css';
import './home.css';

// A returning, signed-in person was already sent to Chat by index.html's inline script, before this module loaded.
ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
