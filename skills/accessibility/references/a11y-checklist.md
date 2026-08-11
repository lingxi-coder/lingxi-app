# Accessibility checklist

- Use `header`, `nav`, `main`, `aside`, and `footer` only when their landmark
  meaning is true; keep a single useful page heading.
- Every input has a programmatic label and an error tied with `aria-describedby`;
  every icon-only button has an accessible name.
- Focus is visible, ordered, retained after dialogs/navigation, and never
  trapped outside the active modal.
- Dynamic changes use a polite status region where a screen reader needs to
  hear them; validation errors identify the field and recovery action.
- Text, controls, focus, and meaningful non-text states meet contrast needs;
  selected/error/success state has a non-color cue.
- Large text and zoom do not hide content or force two-dimensional scrolling;
  animation has a reduced-motion path.
