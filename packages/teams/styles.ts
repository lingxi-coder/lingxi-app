// Import once from the host's entry (a bundler resolves the stylesheet). The components themselves
// never import CSS, so they load in Node for tests and server rendering.
import './workspace.css';
