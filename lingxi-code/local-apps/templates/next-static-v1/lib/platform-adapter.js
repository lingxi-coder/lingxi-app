import {
  FALLBACK_DEVICE_CONTEXT,
  getDeviceContext,
  normalizeDeviceContext,
} from "./lingxi-bridge";

const DEFAULT_ADAPTER_KEY = "desktop:desktop";

const PLATFORM_ADAPTERS = {
  "ios:iphone": {
    navigation: "tabs",
    controlDensity: "44pt",
    surfaceRadius: 18,
    navigationPlacement: "bottom",
    stateLayer: "none",
    fontFamily: "-apple-system, BlinkMacSystemFont, sans-serif",
    supportsSidebar: false,
  },
  "android:phone": {
    navigation: "top-bar-and-bottom-nav",
    controlDensity: "48dp",
    surfaceRadius: 16,
    navigationPlacement: "bottom",
    stateLayer: "ripple",
    fontFamily: "Roboto, sans-serif",
    supportsSidebar: false,
  },
  "ios:tablet": {
    navigation: "sidebar-or-split-view",
    controlDensity: "44pt",
    surfaceRadius: 20,
    navigationPlacement: "sidebar",
    stateLayer: "none",
    fontFamily: "-apple-system, BlinkMacSystemFont, sans-serif",
    supportsSidebar: true,
  },
  "android:tablet": {
    navigation: "rail-and-adaptive-pane",
    controlDensity: "48dp",
    surfaceRadius: 16,
    navigationPlacement: "rail",
    stateLayer: "ripple",
    fontFamily: "Roboto, sans-serif",
    supportsSidebar: true,
  },
  "desktop:desktop": {
    navigation: "sidebar",
    controlDensity: "40px",
    surfaceRadius: 20,
    navigationPlacement: "sidebar",
    stateLayer: "hover",
    fontFamily: "Inter, system-ui, sans-serif",
    supportsSidebar: true,
  },
};

/**
 * Keep platform differences in one adapter layer, not page-wide branches.
 *
 * Tolerates a null/partial `context`: this runs on the first render in every
 * environment, including a plain desktop browser with no host bridge, and a
 * layout helper must never be the thing that throws.
 */
export function getPlatformAdapter(context = getDeviceContext()) {
  const safeContext = normalizeDeviceContext(context);
  const key = `${safeContext.os}:${safeContext.formFactor}`;
  return {
    ...PLATFORM_ADAPTERS[key === "ios:ipad" ? "ios:tablet" : key] ??
      PLATFORM_ADAPTERS[DEFAULT_ADAPTER_KEY],
    key,
    context: safeContext,
  };
}

export function platformStyle(adapter) {
  const fallback = PLATFORM_ADAPTERS[DEFAULT_ADAPTER_KEY];
  const safe = adapter && typeof adapter === "object" ? adapter : fallback;
  const safeArea =
    safe.context?.safeArea ?? FALLBACK_DEVICE_CONTEXT.safeArea;
  return {
    "--safe-area-top": `${safeArea.top}px`,
    "--safe-area-right": `${safeArea.right}px`,
    "--safe-area-bottom": `${safeArea.bottom}px`,
    "--safe-area-left": `${safeArea.left}px`,
    "--platform-control-min": safe.controlDensity === "44pt" ? "44px" : "48px",
    "--platform-radius": `${safe.surfaceRadius ?? fallback.surfaceRadius}px`,
    "--platform-font": safe.fontFamily ?? fallback.fontFamily,
    "--platform-navigation-placement":
      safe.navigationPlacement ?? fallback.navigationPlacement,
  };
}

export const platformNavigationItems = [
  { id: "home", label: "概览" },
  { id: "activity", label: "动态" },
  { id: "saved", label: "收藏" },
  { id: "settings", label: "设置" },
];
