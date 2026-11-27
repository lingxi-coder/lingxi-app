import { maxCardImagePixels, rasterPixels } from './raster.js';

const root = document.querySelector('#surface');
const byId = new Map(), metadata = new WeakMap(), canvasRevisions = new WeakMap();
const selectSelections = new WeakMap();
const maxCanvases = 8, maxCanvasPixels = 8 * 1024 * 1024, maxDrawCommands = 6000;
const htmlTags = new Set('div span section article header footer aside h1 h2 h3 h4 h5 h6 p pre code strong em b i small u s mark sub sup br hr ul ol li dl dt dd table thead tbody tfoot tr th td caption colgroup col form fieldset legend label input button select option optgroup textarea output progress meter details summary canvas img figure figcaption style'.split(' '));
const svgTags = new Map('svg g path line circle rect ellipse polyline polygon text tspan title desc defs symbol use clipPath mask linearGradient radialGradient stop pattern'.split(' ').map(tag => [tag.toLowerCase(), tag]));
const namespaces = { svg: 'http://www.w3.org/2000/svg', html: 'http://www.w3.org/1999/xhtml' };
const formTypes = new Set(['text', 'search', 'number', 'range', 'checkbox', 'radio', 'date', 'time', 'color', 'email', 'tel', 'button', 'submit']);
const allowedAttributes = new Set('id class style title role tabindex name type placeholder autocomplete spellcheck min max step minlength maxlength size rows cols width height disabled hidden selected for open multiple required readonly form colspan rowspan scope align lang dir value checked alt viewbox preserveaspectratio x y x1 x2 y1 y2 cx cy r rx ry d points transform fill fill-opacity fill-rule stroke stroke-width stroke-linecap stroke-linejoin stroke-dasharray stroke-opacity opacity text-anchor dominant-baseline font-size font-family font-weight offset stop-color stop-opacity gradientunits gradienttransform fx fy clip-path mask'.split(' '));
const svgAttributes = new Map([['viewbox', 'viewBox'], ['preserveaspectratio', 'preserveAspectRatio'],
  ['gradientunits', 'gradientUnits'], ['gradienttransform', 'gradientTransform']]);
const drawMethods = new Set('clearRect fillRect strokeRect fillText strokeText beginPath closePath moveTo lineTo arc arcTo ellipse rect roundRect bezierCurveTo quadraticCurveTo fill stroke clip save restore translate rotate scale setTransform resetTransform setLineDash'.split(' '));
const drawProperties = new Set('fillStyle strokeStyle lineWidth font textAlign textBaseline globalAlpha lineCap lineJoin globalCompositeOperation'.split(' '));
let send;
let worker;
let images = {};
const imagePixels = new Map();
let admittedImagePixels = 0;

function imageSource(value, budget) {
  const source = Object.hasOwn(images, value) ? images[value] : value;
  if (!source.startsWith('data:image/') || budget.imageCount >= 16) return;
  let pixels = imagePixels.get(source);
  if (pixels === undefined) {
    if (imagePixels.size >= 32 || budget.imageProbes >= 16) return;
    budget.imageProbes++;
    pixels = rasterPixels(source);
    if (admittedImagePixels + pixels > maxCardImagePixels) pixels = 0;
    imagePixels.set(source, pixels);
    admittedImagePixels += pixels;
  }
  if (!pixels || budget.imagePixels + pixels > maxCardImagePixels) return;
  budget.imageCount++;
  budget.imagePixels += pixels;
  return source;
}
let frameSeen = false;
let lastHeight = 0;
let budgetedCanvases = new Map();
let initialization;
const nativeChunkBytes = 256 * 1024;
const packageBytes = 50 * 1024 * 1024;

function decode64(data) {
  if (typeof Uint8Array.fromBase64 === 'function') return Uint8Array.fromBase64(data);
  const binary = atob(data), bytes = new Uint8Array(binary.length);
  for (let index = 0; index < bytes.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

function nativeInitialization(message) {
  const value = message.value;
  if (message.type === 'initializeStart') {
    const assets = Object.entries(value.bundle.assets ?? {});
    if (assets.length > 32) throw new Error('Invalid initialization');
    const source = () => ({ decoder: new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }), chunks: [] });
    initialization = { load: value, size: 0, html: source(), parts: new Map(assets.map(([name]) => [name, source()])) };
  } else if (initialization && message.type === 'initializePart') {
    const target = value.asset === undefined ? initialization.html : initialization.parts.get(value.asset);
    if (!target || typeof value.data !== 'string' || value.data.length > Math.ceil(nativeChunkBytes / 3) * 4) {
      throw new Error('Invalid initialization');
    }
    const bytes = decode64(value.data);
    initialization.size += bytes.length;
    if (bytes.length > nativeChunkBytes || initialization.size > packageBytes) throw new Error('Invalid initialization');
    target.chunks.push(target.decoder.decode(bytes, { stream: true }));
  } else if (initialization && message.type === 'initializeEnd') {
    const { load, html, parts } = initialization;
    initialization = undefined;
    const text = source => { source.chunks.push(source.decoder.decode()); return source.chunks.join(''); };
    load.bundle.html = text(html);
    for (const [name, source] of parts) load.bundle.assets[name].data = text(source);
    receive({ type: 'initialize', value: load });
  }
}

function styleSafe(text) {
  return !/@import|@font-face|\\|url\(\s*['"]?\s*(?!#)/iu.test(text);
}
function safeAttribute(element, name, value, budget) {
  const lower = name.toLowerCase();
  if (typeof value !== 'string' || value.length > 32_768 || lower.startsWith('on')) return;
  if (lower === 'style' && !styleSafe(value)) return;
  if (lower === 'type' && element.localName === 'input' && !formTypes.has(value.toLowerCase())) return;
  if (lower === 'autocomplete') return;
  if (element.localName === 'canvas' && ['width', 'height'].includes(lower)) return;
  if (lower === 'src' && element.localName === 'img') {
    const source = imageSource(value, budget);
    if (!source) element.removeAttribute('src');
    else if (element.getAttribute('src') !== source) element.setAttribute('src', source);
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

function canvasSize(attributes) {
  const dimension = (value, fallback) => value === undefined ? fallback :
    Math.max(1, Math.min(2048, Number.parseInt(value, 10) || 1));
  return [dimension(attributes.width, 300), dimension(attributes.height, 150)];
}
function planCanvases(nodes, sizes, budget) {
  for (const item of nodes) {
    if (!item) continue;
    if (item.tag === 'canvas' && !sizes.has(item.id) && sizes.size < maxCanvases) {
      const size = canvasSize(item.attributes ?? {});
      const pixels = size[0] * size[1];
      if (budget.pixels + pixels <= maxCanvasPixels) {
        sizes.set(item.id, size);
        budget.pixels += pixels;
      }
    }
    if (item.children) planCanvases(item.children, sizes, budget);
  }
}
function paint(canvas, item, budget) {
  const size = budget.sizes.get(item.id);
  if (!size) {
    if (canvas.width !== 1) canvas.width = 1;
    if (canvas.height !== 1) canvas.height = 1;
    canvasRevisions.delete(canvas);
    return;
  }
  budget.canvases.set(item.id, canvas);
  const [width, height] = size, drawing = item.canvas, commands = drawing?.commands;
  const sized = canvas.width === width && canvas.height === height;
  const current = sized && canvasRevisions.get(canvas) === drawing?.revision;
  const redraw = Array.isArray(commands) && !current;
  const work = redraw ? commands.length : 0;
  if (!sized) {
    canvas.width = 1;
    canvas.height = height;
    canvas.width = width;
  }
  if (!redraw || budget.commands + work > maxDrawCommands) return;
  budget.commands += work;
  canvasRevisions.set(canvas, drawing.revision);
  canvas.width = width; // Reset the accumulated transform and clip.
  const ctx = canvas.getContext('2d');
  if (!ctx) return;
  for (const item of commands) {
    if (!Array.isArray(item)) continue;
    const [method, ...args] = item;
    try {
      if (method === 'set' && drawProperties.has(args[0]) &&
          (Number.isFinite(args[1]) || typeof args[1] === 'string' && args[1].length <= 2048)) {
        ctx[args[0]] = args[1];
      } else if (drawMethods.has(method) && args.length <= 9 && args.every(value =>
        typeof value === 'number' || typeof value === 'boolean' || typeof value === 'string' && value.length <= 2048
        || Array.isArray(value) && value.length <= 32 && value.every(n => typeof n === 'number'))) {
        ctx[method](...args);
      }
    } catch { /* A drawing command that the browser does not support is ignored. */ }
  }
}

function reconcile(parent, nodes, seen, budget, svg = false) {
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
      const attributes = item.attributes ?? {};
      for (const attribute of Array.from(element.attributes)) {
        if (!(tag === 'canvas' && ['width', 'height'].includes(attribute.name)) && !Object.keys(attributes).some(key => key.toLowerCase() === attribute.name.toLowerCase())
            && !['autocomplete', 'href'].includes(attribute.name)) element.removeAttribute(attribute.name);
      }
      for (const [key, value] of Object.entries(attributes)) safeAttribute(element, key, value, budget);
      if (tag === 'input') element.autocomplete = 'off';
      reconcile(element, item.children ?? [], seen, budget, childSvg);
      if (tag === 'select' && Array.isArray(item.selected)) {
        const options = Array.from(element.options);
        const selection = JSON.stringify([item.selected, options.map(option => metadata.get(option))]);
        if (selectSelections.get(element) !== selection) {
          if (element.multiple) {
            const selected = new Set(item.selected);
            options.forEach((option, index) => { option.selected = selected.has(index); });
          } else element.selectedIndex = item.selected[0] ?? -1;
          selectSelections.set(element, selection);
        }
      }
      if (item.value !== undefined && element.value !== item.value && element !== document.activeElement) element.value = item.value;
      if (item.checked !== undefined && element.checked !== item.checked) element.checked = item.checked;
      if (tag === 'canvas') paint(element, item, budget);
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
  if (!worker && ['initializeStart', 'initializePart', 'initializeEnd'].includes(message.type)) {
    nativeInitialization(message);
    return;
  }
  if (message.type === 'initialize' && !worker) {
    const { bundle, aliases, state, theme, locale, expanded } = message.value;
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
        const nextCanvasSizes = new Map();
        planCanvases(data.nodes, nextCanvasSizes, { pixels: 0 });
        // Release the previous frame's backing stores before allocating new ones.
        for (const [id, canvas] of budgetedCanvases) {
          const size = nextCanvasSizes.get(id);
          if (size && size[0] === canvas.width && size[1] === canvas.height) continue;
          canvas.width = 1;
          canvas.height = 1;
          canvasRevisions.delete(canvas);
        }
        const canvases = new Map();
        reconcile(root, data.nodes, seen, {
          sizes: nextCanvasSizes, canvases, commands: 0, imageCount: 0, imagePixels: 0, imageProbes: 0,
        });
        budgetedCanvases = canvases;
        for (const id of byId.keys()) if (!seen.has(id)) byId.delete(id);
        if (!frameSeen) { frameSeen = true; send?.({ type: 'rendered' }); }
        measure();
      } else if (data.type === 'request' || data.type === 'error') send?.(data);
    };
    document.documentElement.dataset.theme = theme.mode;
    worker.postMessage({ type: 'initialize', value: { html: bundle.html, css: styleSafe(css) ? css : '',
      aliases, scripts, state, theme, locale, expanded, width: root.clientWidth } });
  } else if (worker && ['result', 'data', 'layout', 'suspend'].includes(message.type)) {
    if (message.type === 'layout' && message.value.theme) document.documentElement.dataset.theme = message.value.theme.mode;
    worker.postMessage(message);
  }
}

function textAsset(asset) { return new TextDecoder().decode(decode64(asset.data)); }

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
    const selected = node.localName === 'select' && ['input', 'change'].includes(kind) ?
      Array.from(node.options).flatMap((option, index) => option.selected ? [index] : []) : undefined;
    const rect = node.getBoundingClientRect();
    worker?.postMessage({ type: 'event', value: { id, kind, value, selected, checked: node.checked,
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
