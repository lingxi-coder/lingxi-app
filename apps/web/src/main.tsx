import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './app/App';
import './theme.css';
import './styles.css';
import './home.css';

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
