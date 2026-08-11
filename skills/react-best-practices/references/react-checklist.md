# React checklist

- Components have one reason to change and receive explicit props; shared
  behavior lives in hooks/utilities instead of copied handlers.
- Async work has cancellation/cleanup and an explicit pending/error/success
  state; bridge failures are rendered as recoverable UI.
- Lists use stable domain keys; controlled inputs retain user text across
  validation and do not reset from an unrelated render.
- Effects synchronize with external systems only; no effect is used as a
  substitute for a click/submit handler or for deriving a value.
- Platform-specific shell/navigation/tokens are selected once at the adapter
  boundary, while business state remains shared.
- Motion, images, and expensive lists have bounded work and a reduced-motion or
  no-animation fallback.
