const root = document.querySelector('#surface');
const byId = new Map(), metadata = new WeakMap(), canvasRevisions = new WeakMap();
const htmlTags = new Set('div span section article header footer aside h1 h2 h3 h4 h5 h6 p pre code strong em b i small u s mark sub sup br hr ul ol li dl dt dd table thead tbody tfoot tr th td caption colgroup col form fieldset legend label input button select option optgroup textarea output progress meter details summary canvas img figure figcaption style'.split(' '));
const svgTags = new Map('svg g path line circle rect ellipse polyline polygon text tspan title desc defs symbol use clipPath mask linearGradient radialGradient stop pattern'.split(' ').map(tag => [tag.toLowerCase(), tag]));
const namespaces = { svg: 'http://www.w3.org/2000/svg', html: 'http://www.w3.org/1999/xhtml' };
const formTypes = new Set(['text', 'search', 'number', 'range', 'checkbox', 'radio', 'date', 'time', 'color', 'email', 'tel', 'button', 'submit']);
const allowedAttributes = new Set('id class style title role tabindex name type placeholder autocomplete spellcheck min max step minlength maxlength size rows cols width height disabled hidden selected for open multiple required readonly colspan rowspan scope align lang dir value checked alt viewbox preserveaspectratio x y x1 x2 y1 y2 cx cy r rx ry d points transform fill fill-opacity fill-rule stroke stroke-width stroke-linecap stroke-linejoin stroke-dasharray stroke-opacity opacity text-anchor dominant-baseline font-size font-family font-weight offset stop-color stop-opacity gradientunits gradienttransform fx fy clip-path mask'.split(' '));
const svgAttributes = new Map([['viewbox', 'viewBox'], ['preserveaspectratio', 'preserveAspectRatio'],
  ['gradientunits', 'gradientUnits'], ['gradienttransform', 'gradientTransform']]);
const drawMethods = new Set('clearRect fillRect strokeRect fillText strokeText beginPath closePath moveTo lineTo arc arcTo ellipse rect roundRect bezierCurveTo quadraticCurveTo fill stroke clip save restore translate rotate scale setTransform resetTransform setLineDash'.split(' '));
const drawProperties = new Set('fillStyle strokeStyle lineWidth font textAlign textBaseline globalAlpha lineCap lineJoin globalCompositeOperation'.split(' '));
let send;
let worker;
let images = {};
let frameSeen = false;
let lastHeight = 0;

function styleSafe(text) {
  return !/@import|@font-face|\\|url\(\s*['"]?\s*(?!#)/iu.test(text);
}
function safeAttribute(element, name, value) {
  const lower = name.toLowerCase();
  if (value.length > 32_768 || lower.startsWith('on')) return;
  if (lower === 'style' && !styleSafe(value)) return;
  if (lower === 'type' && element.localName === 'input' && !formTypes.has(value.toLowerCase())) return;
  if (lower === 'autocomplete') return;
  if (element.localName === 'canvas' && ['width', 'height'].includes(lower)) {
    value = String(Math.max(1, Math.min(2048, Number.parseInt(value, 10) || 1)));
  }
  if (lower === 'src' && element.localName === 'img') {
    if (Object.hasOwn(images, value)) element.setAttribute('src', images[value]);
    else if (/^data:image\/(png|jpeg|webp);base64,[A-Za-z0-9+/]+=*$/u.test(value)) element.setAttribute('src', value);
    return;
  }
  if (element.localName === 'use' && (lower === 'href' || lower === 'xlink:href') && /^#[A-Za-z0-9_-]+$/u.test(value)) {
    element.setAttribute('href', value);
    return;
  }
  if (allowedAttributes.has(lower) || /^(?:aria|data)-[a-z0-9_-]+$/u.test(lower)) {
    const safeName = element.namespaceURI === namespaces.svg ? svgAttributes.get(lower) ?? lower : lower;
    if (/url\s*\(/iu.test(value) && !/^url\(#[A-Za-z0-9_-]+\)$/u.test(value) && lower !== 'style') return;
    if (element.getAttribute(safeName) !== value) element.setAttribute(safeName, value);
  }
}

function paint(canvas, drawing) {
  if (!drawing || canvasRevisions.get(canvas) === drawing.revision || !Array.isArray(drawing.commands)) return;
  canvasRevisions.set(canvas, drawing.revision);
  const width = Math.min(2048, canvas.width), height = Math.min(2048, canvas.height);
  if (width !== canvas.width || height !== canvas.height) { canvas.width = width; canvas.height = height; }
  const ctx = canvas.getContext('2d');
  if (!ctx) return;
  canvas.width = width; // Reset the accumulated transform and clip.
  for (const item of drawing.commands.slice(0, 6000)) {
    if (!Array.isArray(item)) continue;
    const [method, ...args] = item;
    try {
      if (method === 'set' && drawProperties.has(args[0]) && ['number', 'string'].includes(typeof args[1])) {
        ctx[args[0]] = args[1];
      } else if (drawMethods.has(method) && args.length <= 9 && args.every(value =>
        typeof value === 'number' || typeof value === 'boolean' || typeof value === 'string' && value.length <= 2048
        || Array.isArray(value) && value.length <= 32 && value.every(n => typeof n === 'number'))) {
        ctx[method](...args);
      }
    } catch { /* A drawing command that the browser does not support is ignored. */ }
  }
}

function reconcile(parent, nodes, seen, svg = false) {
  let next = parent.firstChild;
  for (const item of nodes) {
    if (!item || !Number.isSafeInteger(item.id) || seen.has(item.id)) continue;
    const tag = item.tag?.toLowerCase(), childSvg = (svg || tag === 'svg') && svgTags.has(tag);
    const text = typeof item.text === 'string';
    if (!text && (svg ? !childSvg : !childSvg && !htmlTags.has(tag))) continue;
    const namespace = childSvg ? namespaces.svg : namespaces.html;
    const name = childSvg ? svgTags.get(tag) : tag;
    let element = byId.get(item.id);
    if (!element || (text ? element.nodeType !== 3 : element.localName !== name || element.namespaceURI !== namespace)) {
      element = text ? document.createTextNode(item.text) : document.createElementNS(namespace, name);
      byId.set(item.id, element);
      metadata.set(element, item.id);
    }
    seen.add(item.id);
    if (text) {
      if (element.data !== item.text) element.data = item.text;
    } else if (tag === 'style') {
      const css = (item.children ?? []).map(child => child.text ?? '').join('');
      if (element.textContent !== css) element.textContent = styleSafe(css) ? css : '';
    } else {
      for (const attribute of Array.from(element.attributes)) {
        if (!Object.keys(item.attributes ?? {}).some(key => key.toLowerCase() === attribute.name.toLowerCase())
            && !['autocomplete', 'href'].includes(attribute.name)) element.removeAttribute(attribute.name);
      }
      for (const [key, value] of Object.entries(item.attributes ?? {})) safeAttribute(element, key, value);
      if (tag === 'input') element.autocomplete = 'off';
      reconcile(element, item.children ?? [], seen, childSvg);
      if (item.value !== undefined && element.value !== item.value && element !== document.activeElement) element.value = item.value;
      if (item.checked !== undefined && element.checked !== item.checked) element.checked = item.checked;
      if (tag === 'canvas') paint(element, item.canvas);
    }
    if (element === next) next = next.nextSibling;
    else parent.insertBefore(element, next);
  }
  while (next) { const sibling = next.nextSibling; next.remove(); next = sibling; }
}

function measure() {
  const height = Math.min(4096, Math.max(32, Math.ceil(root.getBoundingClientRect().height)));
  if (lastHeight === height) return;
  lastHeight = height;
  send?.({ type: 'height', height });
}

function receive(message) {
  if (!message || typeof message !== 'object') return;
  if (message.type === 'initialize' && !worker) {
    const { bundle, aliases, state, theme, locale } = message.value;
    const assets = Object.entries(bundle.assets ?? {});
    const scripts = Object.fromEntries(assets.filter(([, asset]) => asset.mime === 'text/javascript')
      .map(([key, asset]) => [key, textAsset(asset)]));
    images = Object.fromEntries(assets.filter(([, asset]) => ['image/png', 'image/jpeg', 'image/webp'].includes(asset.mime))
      .map(([key, asset]) => [key, `data:${asset.mime};base64,${asset.data}`]));
    const css = assets.filter(([, asset]) => asset.mime === 'text/css').map(([, asset]) => textAsset(asset)).join('\n');
    const source = URL.createObjectURL(new Blob([LINGXI_WORKER], { type: 'text/javascript' }));
    worker = new Worker(source);
    URL.revokeObjectURL(source);
    worker.onerror = () => send?.({ type: 'error', code: 'VISUALIZATION_SCRIPT_FAILED' });
    worker.onmessage = ({ data }) => {
      if (data.type === 'frame' && Array.isArray(data.nodes)) {
        const seen = new Set();
        reconcile(root, data.nodes, seen);
        for (const id of byId.keys()) if (!seen.has(id)) byId.delete(id);
        if (!frameSeen) { frameSeen = true; send?.({ type: 'rendered' }); }
        measure();
      } else if (data.type === 'request' || data.type === 'error') send?.(data);
    };
    document.documentElement.dataset.theme = theme.mode;
    worker.postMessage({ type: 'initialize', value: { html: (css && styleSafe(css) ? '<style>' + css + '</style>' : '') + bundle.html,
      aliases, scripts, state, theme, locale, width: root.clientWidth } });
  } else if (worker && ['result', 'data', 'layout', 'suspend'].includes(message.type)) {
    if (message.type === 'layout' && message.value.theme) document.documentElement.dataset.theme = message.value.theme.mode;
    worker.postMessage(message);
  }
}

function textAsset(asset) {
  return new TextDecoder().decode(Uint8Array.from(atob(asset.data), char => char.charCodeAt(0)));
}

for (const kind of ['click', 'input', 'change', 'submit', 'keydown', 'keyup', 'pointerdown', 'pointerup', 'pointermove']) {
  root.addEventListener(kind, event => {
    const node = event.target;
    const id = metadata.get(node);
    if (!id) return;
    if (kind === 'submit' || (kind === 'click' &&
        node.closest('button[type="submit"],button:not([type]),input[type="submit"]')?.closest('form'))
        || (kind === 'keydown' && event.key === 'Enter' && node.localName === 'input' && node.closest('form'))) {
      event.preventDefault();
    }
    const value = typeof node.value === 'string' ? node.value.slice(0, 32_768) : undefined;
    const rect = node.getBoundingClientRect();
    worker?.postMessage({ type: 'event', value: { id, kind, value, checked: node.checked,
      key: event.key, code: event.code, button: event.button, buttons: event.buttons,
      clientX: event.clientX, clientY: event.clientY, offsetX: event.clientX - rect.left, offsetY: event.clientY - rect.top } });
  });
}
new ResizeObserver(() => {
  measure();
  worker?.postMessage({ type: 'layout', value: { width: root.clientWidth } });
}).observe(root);

const native = globalThis.webkit?.messageHandlers?.teamVisualization ?? globalThis.teamVisualization;
if (native) {
  send = value => native.postMessage(JSON.stringify(value));
  const accept = json => { try { receive(JSON.parse(json)); } catch { send({ type: 'error', code: 'VISUALIZATION_SCRIPT_FAILED' }); } };
  Object.defineProperty(globalThis, 'receiveTeamVisualization', {
    value: accept,
  });
  if (globalThis.teamVisualization) native.onmessage = event => accept(event.data);
  send({ type: 'ready' });
} else if (window.parent !== window) {
  const connect = event => {
    if (event.source !== window.parent || event.data?.type !== 'connectTeamVisualization' || event.ports.length !== 1) return;
    window.removeEventListener('message', connect);
    const port = event.ports[0];
    send = value => port.postMessage(value);
    port.onmessage = event => {
      try { receive(event.data); } catch { send({ type: 'error', code: 'VISUALIZATION_SCRIPT_FAILED' }); }
    };
    send({ type: 'ready' });
  };
  window.addEventListener('message', connect);
  window.parent.postMessage({ type: 'teamVisualizationReady' }, '*');
}
