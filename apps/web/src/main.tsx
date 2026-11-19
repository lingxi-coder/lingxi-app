import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './app/App';
import { hasSessionHint, landingRedirect } from './app/sessionHint';
import './theme.css';
import './styles.css';
import './home.css';

// A returning, signed-in person goes straight to Chat; decided before anything renders, so the landing page never flashes.
const redirect = landingRedirect(location.pathname, location.search, hasSessionHint());
if (redirect) location.replace(redirect);
else ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
