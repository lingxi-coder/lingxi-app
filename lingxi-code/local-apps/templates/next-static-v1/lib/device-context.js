// Device facts have exactly ONE derivation, and it lives in `./lingxi-bridge`
// beside every other host helper — that is the module generated code naturally
// imports from. This file used to normalize the context a SECOND time while
// `./lingxi-bridge` exported a same-named function that returned `null`, so
// which of the two an import happened to pick decided whether the app booted
// in a plain browser. Re-exporting keeps both import sites working and makes a
// silent fork impossible.
//
// The dependency runs one way (device-context -> lingxi-bridge); do not import
// this module from `./lingxi-bridge` or the two become a cycle.
export { FALLBACK_DEVICE_CONTEXT, getDeviceContext } from "./lingxi-bridge";
